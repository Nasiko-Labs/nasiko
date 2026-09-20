//! Cell store — the learned-quality memory behind Thompson-sampling tier selection.
//!
//! A [`Cell`] is a running estimate of how well a `(tier, request_type)` performs for a
//! provider; [`crate::routing::classifier::pick_model_thompson`] reads these to bias
//! selection toward what has worked. Feedback flows the other way: when a user's next turn
//! carries a [`signal`](crate::routing::classifier::signal), `route_model` calls
//! [`CellStore::observe`] to fold that reward into the relevant cell.
//!
//! Cells are scoped **per provider** (tiers/costs are provider-specific) and shared across
//! conversations, so learning from one conversation improves the next one's cold-start pick.
//!
//! Two impls behind the [`CellStore`] seam, mirroring the [tier registry](super::registry):
//! - [`InMemoryCellStore`] — process-local; the single-node default and the test double.
//! - [`PgCellStore`] — durable, cross-instance; the `router_quality_cells` table (migration
//!   026). Like the decision cache, it is **not** load-bearing: a DB error on `load` degrades
//!   to an empty map (cold start) and a failed `observe` is dropped, so learning can stall but
//!   routing never breaks.

use async_trait::async_trait;
use dashmap::DashMap;
use sqlx::PgPool;

use super::classifier::{Cell, CellMap, MAX_SAMPLES, RequestType, Tier, tier_prior, update_cell};

/// Reads and writes the learned [`Cell`]s for a provider.
#[async_trait]
pub trait CellStore: Send + Sync {
    /// All learned cells for `provider`, keyed by `(tier, request_type)`. Returns an empty
    /// map on any backend failure — the classifier then falls back to cold-start priors.
    async fn load(&self, provider: &str) -> CellMap;

    /// Fold one `observation` (`0.0` = bad, `1.0` = good) into the cell for
    /// `(provider, tier, request_type)`, creating it if absent. Best-effort — failures are
    /// swallowed.
    async fn observe(&self, provider: &str, tier: Tier, rt: RequestType, observation: f64);
}

/// Process-local cell store. Fine for a single node; learning is lost on restart and not
/// shared across instances (use [`PgCellStore`] for those). Also the test double.
#[derive(Default)]
pub struct InMemoryCellStore {
    cells: DashMap<(String, Tier, RequestType), Cell>,
}

impl InMemoryCellStore {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl CellStore for InMemoryCellStore {
    async fn load(&self, provider: &str) -> CellMap {
        self.cells
            .iter()
            .filter(|e| e.key().0 == provider)
            .map(|e| ((e.key().1, e.key().2), *e.value()))
            .collect()
    }

    async fn observe(&self, provider: &str, tier: Tier, rt: RequestType, observation: f64) {
        let mut entry = self
            .cells
            .entry((provider.to_string(), tier, rt))
            .or_insert_with(|| Cell {
                quality_mean: tier_prior(tier, rt),
                samples: 0,
            });
        *entry = update_cell(*entry, observation);
        tracing::info!(
            target: "nasiko::llm_router::cells",
            store = "in-memory", %provider, ?tier, request_type = %rt.as_str(),
            observation, quality_mean = entry.quality_mean, samples = entry.samples,
            "cell store observe — folded reward into learned quality"
        );
    }
}

/// Postgres-backed cell store reading/writing `router_quality_cells` (migration 026).
///
/// `observe` performs the running-mean update **atomically in SQL** (`INSERT … ON CONFLICT
/// DO UPDATE`), so concurrent gateway instances can learn without a read-modify-write race.
/// The update is the exact port of
/// [`update_cell`](crate::routing::classifier::update_cell): the first observation seeds the
/// mean directly, later ones blend with the effective sample count capped at
/// [`MAX_SAMPLES`].
pub struct PgCellStore {
    db: PgPool,
}

impl PgCellStore {
    pub fn new(db: PgPool) -> Self {
        Self { db }
    }
}

#[async_trait]
impl CellStore for PgCellStore {
    async fn load(&self, provider: &str) -> CellMap {
        let key = provider.trim().to_ascii_lowercase();
        let rows: Result<Vec<(i16, String, f64, i64)>, sqlx::Error> = sqlx::query_as(
            "SELECT tier, request_type, quality_mean, samples \
             FROM router_quality_cells WHERE provider = $1",
        )
        .bind(&key)
        .fetch_all(&self.db)
        .await;
        match rows {
            Ok(rows) => {
                let mut map = CellMap::new();
                for (tier, rt, quality_mean, samples) in rows {
                    if let (Some(tier), Some(rt)) =
                        (Tier::from_level(tier), RequestType::from_wire(&rt))
                    {
                        map.insert(
                            (tier, rt),
                            Cell {
                                quality_mean,
                                samples,
                            },
                        );
                    }
                }
                tracing::info!(
                    target: "nasiko::llm_router::cells",
                    store = "postgres:router_quality_cells", provider = %key, cells = map.len(),
                    "cell store load — loaded learned quality cells for classification"
                );
                map
            }
            Err(e) => {
                tracing::warn!(
                    target: "nasiko::llm_router::cells",
                    error = %e, provider = %key,
                    "router_quality_cells read failed; degrading to cold-start priors (empty cell map)"
                );
                CellMap::new()
            }
        }
    }

    async fn observe(&self, provider: &str, tier: Tier, rt: RequestType, observation: f64) {
        let key = provider.trim().to_ascii_lowercase();
        // First observation (INSERT) seeds the mean to the observation with samples=1 — which
        // is exactly what update_cell yields from a zero-sample cell. On conflict, apply the
        // running-mean blend with the effective sample count capped at MAX_SAMPLES.
        let res = sqlx::query(
            "INSERT INTO router_quality_cells (provider, tier, request_type, quality_mean, samples) \
             VALUES ($1, $2, $3, $4, 1) \
             ON CONFLICT (provider, tier, request_type) DO UPDATE SET \
               quality_mean = router_quality_cells.quality_mean \
                 + ($4 - router_quality_cells.quality_mean) \
                   / (LEAST(router_quality_cells.samples, $5) + 1), \
               samples = LEAST(router_quality_cells.samples + 1, $5), \
               updated_at = now()",
        )
        .bind(&key)
        .bind(tier.as_level())
        .bind(rt.as_str())
        .bind(observation)
        .bind(MAX_SAMPLES)
        .execute(&self.db)
        .await;
        match res {
            Ok(_) => tracing::info!(
                target: "nasiko::llm_router::cells",
                store = "postgres:router_quality_cells", provider = %key, ?tier,
                request_type = %rt.as_str(), observation,
                "cell store observe — folded reward into learned quality (atomic upsert)"
            ),
            Err(e) => tracing::warn!(
                target: "nasiko::llm_router::cells",
                error = %e, provider = %key, ?tier, request_type = %rt.as_str(),
                "router_quality_cells write failed; observation dropped (learning stalls, routing unaffected)"
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn in_memory_learns_and_scopes_by_provider() {
        let store = InMemoryCellStore::new();
        // First observation seeds the mean directly (like update_cell from zero samples).
        store
            .observe("anthropic", Tier::Tier1, RequestType::CodeGeneration, 1.0)
            .await;
        let cells = store.load("anthropic").await;
        let cell = cells
            .get(&(Tier::Tier1, RequestType::CodeGeneration))
            .unwrap();
        assert_eq!(cell.quality_mean, 1.0);
        assert_eq!(cell.samples, 1);

        // A second, contradictory observation pulls the mean toward it.
        store
            .observe("anthropic", Tier::Tier1, RequestType::CodeGeneration, 0.0)
            .await;
        let cells = store.load("anthropic").await;
        let cell = cells
            .get(&(Tier::Tier1, RequestType::CodeGeneration))
            .unwrap();
        assert!((cell.quality_mean - 0.5).abs() < 1e-9);
        assert_eq!(cell.samples, 2);

        // Scoping: a different provider sees none of anthropic's learning.
        assert!(store.load("openai").await.is_empty());
    }
}

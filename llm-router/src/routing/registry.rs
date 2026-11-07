//! Tier registry — maps `(provider, tier)` to a concrete model id.
//!
//! Once the classifier picks a [`Tier`] and the resolver has fixed the destination
//! provider, this is where the two combine into the actual model to call.
//!
//! [`PgTierRegistry`] resolves a tier in two steps, most-explicit first:
//!
//! 1. **Operator override** — a `model_registry` row for `(provider, tier)`
//!    (`PUT /api/model-registry`). Always honored: it is explicit operator config.
//! 2. **Catalog-derived** — the provider's live model catalog (`provider_models`,
//!    synced from the provider's `GET /models` by [`super::catalog`]) ranked by price
//!    (`model_pricing`) as the strength signal: Tier 1 = priciest, Tier 3 = cheapest,
//!    Tier 2 = the median. Only models with a pricing row participate — without a
//!    price there is no strength signal and no way to meter cost.
//!
//! There is deliberately **no hardcoded model list**: a catalog miss (no rows, DB
//! error, unpriced models) returns `None`, which makes the router fall through to the
//! request's own model. Routing to a model the provider doesn't serve would be
//! strictly worse than not routing at all.

use async_trait::async_trait;
use sqlx::PgPool;

use super::classifier::Tier;

impl Tier {
    /// The `model_registry.tier` SMALLINT value (1 = strongest … 3 = smallest).
    pub fn as_level(self) -> i16 {
        match self {
            Tier::Tier1 => 1,
            Tier::Tier2 => 2,
            Tier::Tier3 => 3,
        }
    }

    /// Inverse of [`Tier::as_level`]; `None` for out-of-range values.
    pub fn from_level(level: i16) -> Option<Tier> {
        match level {
            1 => Some(Tier::Tier1),
            2 => Some(Tier::Tier2),
            3 => Some(Tier::Tier3),
            _ => None,
        }
    }
}

/// Looks up the model for a `(provider, tier)` pair.
#[async_trait]
pub trait TierRegistry: Send + Sync {
    /// The model id for `(provider, tier)`, or `None` if no mapping can be derived
    /// (caller falls through to the configured/default model).
    async fn model_for(&self, provider: &str, tier: Tier) -> Option<String>;
}

/// Model names that are never chat-completion models — excluded from tier candidacy.
/// Provider catalogs mix modalities (embeddings, images, audio); without this filter a
/// cheap embedding model would win Tier 3 and break every classified chat request.
fn is_chat_model(model: &str) -> bool {
    const NON_CHAT: &[&str] = &[
        "embed",
        "whisper",
        "tts",
        "dall-e",
        "moderation",
        "realtime",
        "audio",
        "image",
        "transcribe",
        "babbage",
        "davinci",
        "sora",
    ];
    let m = model.to_ascii_lowercase();
    !NON_CHAT.iter().any(|s| m.contains(s))
}

/// Pick tier models from a price-ranked catalog (sorted by price DESC).
///
/// Price is the strength signal: within one provider, a more expensive model is the
/// stronger one. Tier 1 takes the priciest, Tier 3 the cheapest, Tier 2 the median;
/// with fewer than three models the cheapest tiers collapse onto what exists.
fn tiers_from_priced_catalog(mut priced: Vec<(String, f64)>) -> Option<(String, String, String)> {
    priced.retain(|(m, _)| is_chat_model(m));
    // Stable sort keeps a deterministic order for equal prices.
    priced.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    match priced.len() {
        0 => None,
        1 => {
            let only = priced[0].0.clone();
            Some((only.clone(), only.clone(), only))
        }
        2 => {
            let (strong, cheap) = (priced[0].0.clone(), priced[1].0.clone());
            Some((strong, cheap.clone(), cheap))
        }
        n => {
            let t1 = priced[0].0.clone();
            let t2 = priced[n / 2].0.clone();
            let t3 = priced[n - 1].0.clone();
            Some((t1, t2, t3))
        }
    }
}

/// Postgres-backed registry: operator `model_registry` overrides first, then the
/// price-ranked live catalog.
pub struct PgTierRegistry {
    db: PgPool,
}

impl PgTierRegistry {
    pub fn new(db: PgPool) -> Self {
        Self { db }
    }

    /// Operator override for `(provider, tier)`, if any.
    async fn operator_override(&self, provider: &str, tier: Tier) -> Option<String> {
        sqlx::query_scalar("SELECT model FROM model_registry WHERE provider = $1 AND tier = $2")
            .bind(provider)
            .bind(tier.as_level())
            .fetch_optional(&self.db)
            .await
            .map_err(|e| {
                tracing::warn!(
                    target: "nasiko::llm_router::registry",
                    error = %e, provider = %provider, tier = ?tier,
                    "model_registry read failed; trying catalog-derived mapping"
                );
                e
            })
            .ok()
            .flatten()
    }

    /// The catalog-derived tier mapping for `provider`: live `provider_models` joined
    /// to their current prices (model-name match, mirroring `DbPricing` semantics),
    /// price-ranked and reduced to three tiers.
    async fn catalog_derived(&self, provider: &str, tier: Tier) -> Option<String> {
        let rows: Vec<(String, f64)> = sqlx::query_as(
            r#"SELECT pm.model,
                      (mp.input_price_per_1m + mp.output_price_per_1m)::float8 AS price
               FROM provider_models pm
               JOIN LATERAL (
                   SELECT input_price_per_1m, output_price_per_1m
                   FROM model_pricing mp
                   WHERE mp.model = pm.model
                     AND mp.effective_from <= now()
                     AND (mp.effective_until IS NULL OR mp.effective_until > now())
                   ORDER BY mp.effective_from DESC
                   LIMIT 1
               ) mp ON true
               WHERE pm.provider = $1"#,
        )
        .bind(provider)
        .fetch_all(&self.db)
        .await
        .map_err(|e| {
            tracing::warn!(
                target: "nasiko::llm_router::registry",
                error = %e, provider = %provider,
                "provider_models read failed; no tier mapping available"
            );
            e
        })
        .ok()?;
        let (t1, t2, t3) = tiers_from_priced_catalog(rows)?;
        let model = match tier {
            Tier::Tier1 => t1,
            Tier::Tier2 => t2,
            Tier::Tier3 => t3,
        };
        tracing::info!(
            target: "nasiko::llm_router::registry",
            provider = %provider, tier = ?tier, tier_level = tier.as_level(), model = %model,
            "tier registry lookup — derived from live provider catalog (price-ranked)"
        );
        Some(model)
    }
}

#[async_trait]
impl TierRegistry for PgTierRegistry {
    async fn model_for(&self, provider: &str, tier: Tier) -> Option<String> {
        let key = provider.trim().to_ascii_lowercase();
        if let Some(model) = self.operator_override(&key, tier).await {
            tracing::info!(
                target: "nasiko::llm_router::registry",
                provider = %key, tier = ?tier, tier_level = tier.as_level(), model = %model,
                "tier registry lookup — resolved from operator override (model_registry)"
            );
            return Some(model);
        }
        self.catalog_derived(&key, tier).await
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    //! A fixed stub registry for routing tests — production derives tiers from the
    //! live catalog, but classifier/precedence tests just need deterministic models.
    use super::*;

    pub struct StubRegistry;

    #[async_trait]
    impl TierRegistry for StubRegistry {
        async fn model_for(&self, provider: &str, tier: Tier) -> Option<String> {
            match provider.trim().to_ascii_lowercase().as_str() {
                "anthropic" => Some(
                    match tier {
                        Tier::Tier1 => "claude-opus-4-8",
                        Tier::Tier2 => "claude-sonnet-4-6",
                        Tier::Tier3 => "claude-haiku-4-5",
                    }
                    .to_string(),
                ),
                "openai" => Some(
                    match tier {
                        Tier::Tier1 => "gpt-5.5",
                        Tier::Tier2 => "gpt-5.4",
                        Tier::Tier3 => "gpt-4o-mini",
                    }
                    .to_string(),
                ),
                _ => None,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::StubRegistry;
    use super::*;

    #[test]
    fn tier_levels_are_stable() {
        assert_eq!(Tier::Tier1.as_level(), 1);
        assert_eq!(Tier::Tier2.as_level(), 2);
        assert_eq!(Tier::Tier3.as_level(), 3);
        assert_eq!(Tier::from_level(2), Some(Tier::Tier2));
        assert_eq!(Tier::from_level(4), None);
    }

    #[test]
    fn tiers_rank_by_price_descending() {
        let priced = vec![
            ("deepseek-v4-flash".to_string(), 0.42),
            ("deepseek-v4-pro".to_string(), 2.74),
        ];
        let (t1, t2, t3) = tiers_from_priced_catalog(priced).unwrap();
        assert_eq!(t1, "deepseek-v4-pro");
        assert_eq!(t2, "deepseek-v4-flash");
        assert_eq!(t3, "deepseek-v4-flash");
    }

    #[test]
    fn tiers_collapse_with_a_single_model() {
        let priced = vec![("only-model".to_string(), 1.0)];
        let (t1, t2, t3) = tiers_from_priced_catalog(priced).unwrap();
        assert_eq!(t1, "only-model");
        assert_eq!(t2, "only-model");
        assert_eq!(t3, "only-model");
    }

    #[test]
    fn tier2_is_the_median_with_many_models() {
        let priced = vec![
            ("cheap".to_string(), 0.1),
            ("mid".to_string(), 1.0),
            ("pricey".to_string(), 10.0),
        ];
        let (t1, t2, t3) = tiers_from_priced_catalog(priced).unwrap();
        assert_eq!(t1, "pricey");
        assert_eq!(t2, "mid");
        assert_eq!(t3, "cheap");
    }

    #[test]
    fn non_chat_models_are_excluded() {
        let priced = vec![
            ("text-embedding-3-large".to_string(), 0.13),
            ("gpt-4o-mini".to_string(), 0.75),
            ("gpt-5.5".to_string(), 30.0),
            ("dall-e-3".to_string(), 0.04),
        ];
        let (t1, _, t3) = tiers_from_priced_catalog(priced).unwrap();
        assert_eq!(t1, "gpt-5.5");
        assert_eq!(t3, "gpt-4o-mini");
    }

    #[test]
    fn a_video_model_never_wins_a_tier() {
        assert!(!is_chat_model("sora-2"));
        assert!(!is_chat_model("Sora-2-Pro"));
        let priced = vec![
            ("sora-2-pro".to_string(), 120.0),
            ("gpt-5.5".to_string(), 30.0),
            ("gpt-4o-mini".to_string(), 0.75),
        ];
        let (t1, _, t3) = tiers_from_priced_catalog(priced).unwrap();
        assert_eq!(t1, "gpt-5.5");
        assert_eq!(t3, "gpt-4o-mini");
    }

    #[test]
    fn empty_or_all_non_chat_catalog_yields_no_tiers() {
        assert!(tiers_from_priced_catalog(vec![]).is_none());
        assert!(
            tiers_from_priced_catalog(vec![("text-embedding-3-small".to_string(), 0.02)]).is_none()
        );
    }

    #[tokio::test]
    async fn stub_registry_serves_fixed_models() {
        let r = StubRegistry;
        assert_eq!(
            r.model_for("anthropic", Tier::Tier1).await.as_deref(),
            Some("claude-opus-4-8")
        );
        assert_eq!(r.model_for("gemini", Tier::Tier1).await, None);
    }
}

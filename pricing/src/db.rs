//! [`PriceBook`] over the `model_pricing` table, which the pricing sync keeps
//! current.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use rust_decimal::prelude::ToPrimitive;
use sqlx::PgPool;
use tokio::sync::RwLock;

use crate::book::{BookTier, PriceBook, PriceRow};
use crate::model::resolve_model;
use crate::ratio::{CacheRatioSample, CacheRatios};

/// How long a resolved row may be reused. Prices move daily at most; this only
/// bounds how long a sync takes to become visible.
const CACHE_TTL: Duration = Duration::from_secs(300);

/// A row plus the window it is valid for, so a cached entry can be reused only
/// for calls that fall inside that window.
#[derive(Clone, Copy)]
struct CachedRow {
    row: PriceRow,
    effective_from: DateTime<Utc>,
    effective_until: Option<DateTime<Utc>>,
    fetched_at: Instant,
}

impl CachedRow {
    fn covers(&self, at: DateTime<Utc>) -> bool {
        self.fetched_at.elapsed() < CACHE_TTL
            && self.effective_from <= at
            && self.effective_until.is_none_or(|until| until > at)
    }
}

/// Reads rates from `model_pricing`, honouring the row's effective window.
///
/// The window matters: `model_pricing` carries genuine price history (the sync
/// closes a row with `effective_until` and opens a new one when a rate moves),
/// so pricing a call at `now()` instead of at the time it happened silently
/// rewrites history whenever an old trace is re-materialized.
pub struct DbPriceBook {
    db: PgPool,
    cache: RwLock<HashMap<String, Option<CachedRow>>>,
    backdated: bool,
}

impl DbPriceBook {
    pub fn new(db: PgPool) -> Self {
        Self {
            db,
            cache: RwLock::new(HashMap::new()),
            backdated: false,
        }
    }

    /// Answers only for a call older than the model's whole price history, with
    /// the model's newest rate.
    ///
    /// History starts when a model's first row is written, on most installs at
    /// first boot, so imported transcripts predate every row and would otherwise
    /// fall through to the compiled-in list, which lags the synced book. The
    /// newest row is used rather than the earliest because a first row is often
    /// a seed placeholder corrected moments later: 0046 closed Opus 5's, priced
    /// at 3x its list rate. Answers are attributed as [`BookTier::Backdated`].
    pub fn backdated(db: PgPool) -> Self {
        Self {
            backdated: true,
            ..Self::new(db)
        }
    }

    async fn lookup(
        &self,
        key: &str,
        provider: Option<&str>,
        model: &str,
        at: DateTime<Utc>,
    ) -> Option<PriceRow> {
        if let Some(Some(cached)) = self.cache.read().await.get(key)
            && cached.covers(at)
        {
            return Some(cached.row);
        }

        let fetched = self.query(provider, model, at).await;
        self.cache.write().await.insert(key.to_string(), fetched);
        fetched.map(|cached| cached.row)
    }

    async fn query(
        &self,
        provider: Option<&str>,
        model: &str,
        at: DateTime<Utc>,
    ) -> Option<CachedRow> {
        #[derive(sqlx::FromRow)]
        struct Row {
            input_price_per_1m: rust_decimal::Decimal,
            output_price_per_1m: rust_decimal::Decimal,
            cache_creation_price_per_1m: Option<rust_decimal::Decimal>,
            cache_creation_1h_price_per_1m: Option<rust_decimal::Decimal>,
            cache_read_price_per_1m: Option<rust_decimal::Decimal>,
            effective_from: DateTime<Utc>,
            effective_until: Option<DateTime<Utc>>,
            history_start: Option<DateTime<Utc>>,
        }

        let sql = if self.backdated {
            r#"SELECT * FROM (
                   SELECT input_price_per_1m, output_price_per_1m,
                          cache_creation_price_per_1m, cache_read_price_per_1m, cache_creation_1h_price_per_1m,
                          effective_from, NULL::TIMESTAMPTZ AS effective_until,
                          min(effective_from) OVER () AS history_start
                   FROM model_pricing
                   WHERE lower(model) = $1
                     AND ($2::TEXT IS NULL OR lower(provider) = $2)
               ) rows
               WHERE history_start > $3
               ORDER BY effective_from DESC
               LIMIT 1"#
        } else {
            r#"SELECT input_price_per_1m, output_price_per_1m,
                            cache_creation_price_per_1m, cache_read_price_per_1m, cache_creation_1h_price_per_1m,
                            effective_from, effective_until, NULL::TIMESTAMPTZ AS history_start
                     FROM model_pricing
                     WHERE lower(model) = $1
                       AND ($2::TEXT IS NULL OR lower(provider) = $2)
                       AND effective_from <= $3
                       AND (effective_until IS NULL OR effective_until > $3)
                     ORDER BY effective_from DESC
                     LIMIT 1"#
        };

        let row = sqlx::query_as::<_, Row>(sql)
            .bind(model.to_lowercase())
            .bind(provider.map(str::to_lowercase))
            .bind(at)
            .fetch_optional(&self.db)
            .await
            .map_err(|e| tracing::warn!(model, error = %e, "model_pricing lookup failed"))
            .ok()
            .flatten()?;

        // A backdated answer holds for every call before the history starts.
        let (effective_from, effective_until) = match row.history_start {
            Some(start) => (DateTime::<Utc>::MIN_UTC, Some(start)),
            None => (row.effective_from, row.effective_until),
        };
        Some(CachedRow {
            row: PriceRow {
                input_per_1m: row.input_price_per_1m.to_f64()?,
                output_per_1m: row.output_price_per_1m.to_f64()?,
                cache_read_per_1m: row.cache_read_price_per_1m.and_then(|v| v.to_f64()),
                cache_creation_per_1m: row.cache_creation_price_per_1m.and_then(|v| v.to_f64()),
                cache_creation_1h_per_1m: row
                    .cache_creation_1h_price_per_1m
                    .and_then(|v| v.to_f64()),
            },
            effective_from,
            effective_until,
            fetched_at: Instant::now(),
        })
    }
}

#[async_trait]
impl PriceBook for DbPriceBook {
    async fn exact(&self, provider: &str, model: &str, at: DateTime<Utc>) -> Option<PriceRow> {
        self.lookup(
            &format!("{provider}\u{0}{model}"),
            Some(provider),
            model,
            at,
        )
        .await
    }

    async fn by_model(&self, model: &str, at: DateTime<Utc>) -> Option<PriceRow> {
        self.lookup(&format!("\u{0}{model}"), None, model, at).await
    }

    fn tier(&self) -> BookTier {
        if self.backdated {
            BookTier::Backdated
        } else {
            BookTier::Synced
        }
    }
}

/// Derive cache ratios from the rows that already carry cache prices, so the
/// inference used for the rest sharpens as the sync widens its coverage.
///
/// Falls back to the measured defaults when the query fails — a pricing read
/// must never be the reason a request errors.
pub async fn load_cache_ratios(db: &PgPool) -> CacheRatios {
    #[derive(sqlx::FromRow)]
    struct Row {
        model: String,
        input_price_per_1m: rust_decimal::Decimal,
        cache_read_price_per_1m: rust_decimal::Decimal,
        cache_creation_price_per_1m: rust_decimal::Decimal,
    }

    let rows = sqlx::query_as::<_, Row>(
        r#"SELECT model, input_price_per_1m,
                  cache_read_price_per_1m, cache_creation_price_per_1m
           FROM model_pricing
           WHERE effective_until IS NULL
             AND cache_read_price_per_1m IS NOT NULL
             AND cache_creation_price_per_1m IS NOT NULL
             AND input_price_per_1m > 0"#,
    )
    .fetch_all(db)
    .await;

    let rows = match rows {
        Ok(rows) => rows,
        Err(e) => {
            tracing::warn!(error = %e, "cache-ratio sampling failed; using measured defaults");
            return CacheRatios::measured();
        }
    };

    CacheRatios::from_samples(rows.into_iter().filter_map(|row| {
        Some(CacheRatioSample {
            vendor: resolve_model(None, &row.model).vendor,
            input_per_1m: row.input_price_per_1m.to_f64()?,
            cache_read_per_1m: row.cache_read_price_per_1m.to_f64()?,
            cache_creation_per_1m: row.cache_creation_price_per_1m.to_f64()?,
        })
    }))
}

//! The wired-up resolver: books, ratios and the four steps behind one call.
//!
//! Callers hold one of these and ask it to price a call. Constructing it is the
//! composition root's job; handlers receive it.

use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use tokio::sync::RwLock;

use crate::book::{PriceBook, StaticPriceBook};
use crate::cost::CostBreakdown;
use crate::db::{DbPriceBook, load_cache_ratios};
use crate::model::resolve_model;
use crate::quote::{PriceQuote, quote};
use crate::ratio::CacheRatios;
use crate::usage::{NormalizedUsage, PromptConvention, RawUsage, normalize_usage};

/// How long derived cache ratios are reused. They move only when the pricing
/// sync widens its coverage, which it does daily at most.
const RATIO_TTL: Duration = Duration::from_secs(3_600);

/// Everything a caller needs to persist about one priced call.
#[derive(Debug, Clone, PartialEq)]
pub struct PricedCall {
    /// The four token classes, with `input` guaranteed cache-exclusive.
    pub usage: NormalizedUsage,
    /// The rates used, and where each came from.
    pub quote: PriceQuote,
    /// Spend, split by token class.
    pub cost: CostBreakdown,
}

impl PricedCall {
    /// Why this call cost what it did, for storing alongside the figure.
    ///
    /// Defined here rather than at each call site so all usage writers record
    /// provenance in the same shape.
    pub fn provenance(&self) -> serde_json::Value {
        serde_json::json!({
            "source": format!("{:?}", self.quote.source),
            "cache_source": format!("{:?}", self.quote.cache_source),
            "estimated": self.cost.estimated,
            "resolved_as": self.quote.resolved_as,
            "rates_per_1m": {
                "input": self.quote.input_per_1m,
                "output": self.quote.output_per_1m,
                "cache_read": self.quote.cache_read_per_1m,
                "cache_creation": self.quote.cache_creation_per_1m,
                "cache_creation_1h": self.quote.cache_creation_1h_per_1m,
            },
            "input_usd": self.cost.input_usd,
            "output_usd": self.cost.output_usd,
            "cache_read_usd": self.cost.cache_read_usd,
            "cache_creation_usd": self.cost.cache_creation_usd,
        })
    }
}

/// Prices a call against the synced price book, falling back to list prices and
/// then to vendor cache conventions.
pub struct PricingEngine {
    books: Vec<Box<dyn PriceBook>>,
    ratios: RwLock<Option<(Arc<CacheRatios>, Instant)>>,
    /// `None` for an offline engine — see [`PricingEngine::offline`].
    db: Option<PgPool>,
}

impl PricingEngine {
    /// The production engine: the synced price book first, then its newest
    /// rates for calls older than its history, then list prices.
    pub fn new(db: PgPool) -> Self {
        Self {
            books: vec![
                Box::new(DbPriceBook::new(db.clone())),
                Box::new(DbPriceBook::backdated(db.clone())),
                Box::new(StaticPriceBook),
            ],
            ratios: RwLock::new(None),
            db: Some(db),
        }
    }

    /// List prices only, with no database behind it.
    ///
    /// For unit tests, which the repo requires to be hermetic, and for any
    /// caller that has no pool. Handing `new` an unreachable pool instead makes
    /// every lookup wait out a connect timeout — a suite that used one took two
    /// minutes and then failed.
    pub fn offline() -> Self {
        Self {
            books: vec![Box::new(StaticPriceBook)],
            ratios: RwLock::new(None),
            db: None,
        }
    }

    /// Normalize, resolve rates, and cost one call.
    ///
    /// `at` is when the call happened, not now — `model_pricing` carries price
    /// history, and re-pricing an old call at today's rates rewrites it.
    pub async fn price(
        &self,
        provider: Option<&str>,
        model: &str,
        raw: RawUsage,
        convention: PromptConvention,
        at: DateTime<Utc>,
    ) -> PricedCall {
        self.price_with_context(
            provider,
            model,
            raw,
            convention,
            at,
            crate::PricingContext::default(),
        )
        .await
    }

    /// Price reported cache durations and serving context through the same engine.
    pub async fn price_with_context(
        &self,
        provider: Option<&str>,
        model: &str,
        raw: RawUsage,
        convention: PromptConvention,
        at: DateTime<Utc>,
        context: crate::PricingContext<'_>,
    ) -> PricedCall {
        let usage = normalize_usage(raw, convention);
        let key = resolve_model(provider.filter(|p| *p != "unknown"), model);
        let ratios = self.ratios().await;
        let books: Vec<&dyn PriceBook> = self.books.iter().map(Box::as_ref).collect();
        let quote = quote(&books, &key, at, &ratios).await;
        let cost = crate::context::context_cost(&usage, &quote, &key, context);
        PricedCall { usage, quote, cost }
    }

    /// Cache ratios, refreshed from the price book on a TTL so the inference
    /// used for cache-less models sharpens as the sync widens its coverage.
    async fn ratios(&self) -> Arc<CacheRatios> {
        if let Some((ratios, loaded_at)) = self.ratios.read().await.as_ref()
            && loaded_at.elapsed() < RATIO_TTL
        {
            return ratios.clone();
        }
        let fresh = Arc::new(match &self.db {
            Some(db) => load_cache_ratios(db).await,
            None => CacheRatios::measured(),
        });
        *self.ratios.write().await = Some((fresh.clone(), Instant::now()));
        fresh
    }
}

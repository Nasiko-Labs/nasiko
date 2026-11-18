//! Cost reporting for the observability read path.
//!
//! The arithmetic lives in `nasiko-pricing`, which the LLM gateway prices
//! through as well — this module only adapts it to the shape the observability
//! API already publishes, and owns the boot-time seeding of `model_pricing`.
//!
//! It used to carry a second cost engine of its own: a `PricingSource` trait, a
//! hardcoded list-price table, and a `DbPricing` that looked rows up by exact
//! model name at `now()`. That engine short-circuited on a database row with
//! NULL cache columns instead of inheriting the missing rates, so cache reads on
//! a partially-priced model billed at the full input rate; it had no notion of a
//! model family, so a point release like `claude-opus-4-6` matched nothing; and
//! it ignored the provider entirely, so a Bedrock-served model was costed
//! against whichever other book happened to carry the same name. All three are
//! fixed by having one implementation rather than two.

use chrono::{DateTime, Utc};
use nasiko_pricing::{PricingEngine, PromptConvention, RawUsage};

/// One call to be costed: who served it, what it was, when it ran, and the four
/// token classes.
///
/// Carried as a struct rather than seven positional arguments because the
/// provider and the timestamp were the two the old signature omitted, and their
/// absence is what made Bedrock traffic and re-priced history wrong.
#[derive(Debug, Clone, Copy)]
pub struct CostRequest<'a> {
    /// Provider label as the span reported it, used to prefer that provider's
    /// own price row. `None` falls back to matching on the model name alone.
    pub provider: Option<&'a str>,
    pub model: Option<&'a str>,
    /// When the call ran. `model_pricing` carries real price history, so an old
    /// trace must be costed at the rates that were in effect then.
    pub at: DateTime<Utc>,
    /// Fresh prompt tokens — cache-exclusive, as `extract_usage_attrs` returns.
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_creation_tokens: u64,
    pub context: nasiko_pricing::PricingContext<'a>,
}

impl<'a> CostRequest<'a> {
    /// A request for one span's already-normalized usage.
    pub fn from_usage(
        provider: Option<&'a str>,
        model: Option<&'a str>,
        at: DateTime<Utc>,
        usage: &'a crate::types::SpanUsage,
    ) -> Self {
        Self {
            provider,
            model,
            at,
            input_tokens: usage.input,
            output_tokens: usage.output,
            cache_read_tokens: usage.cache_read,
            cache_creation_tokens: usage.cache_creation,
            context: nasiko_pricing::PricingContext {
                cache_creation_5m: usage.cache_creation_5m,
                cache_creation_1h: usage.cache_creation_1h,
                speed: usage.speed.as_deref(),
                service_tier: usage.service_tier.as_deref(),
                inference_geo: usage.inference_geo.as_deref(),
                conflicting_observations: usage.conflicting_observations,
            },
        }
    }
}

/// USD cost broken down by token class.
///
/// Field names are `prompt`/`completion` rather than `input`/`output` because
/// they are published that way by `/api/observability/*` (`cost_summary.prompt.cost`)
/// and the UI reads those keys. The rename is a wire-format change, not a
/// pricing one, so it is kept out of this seam.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct CostBreakdown {
    pub prompt_usd: f64,
    pub completion_usd: f64,
    pub cache_read_usd: f64,
    pub cache_creation_usd: f64,
    pub total_usd: f64,
    /// True when any contributing rate was inferred rather than looked up, so a
    /// dashboard can report the estimated share instead of presenting a guess
    /// as a measurement.
    pub estimated: bool,
}

impl CostBreakdown {
    pub fn add_assign(&mut self, other: Self) {
        self.prompt_usd = round6(self.prompt_usd + other.prompt_usd);
        self.completion_usd = round6(self.completion_usd + other.completion_usd);
        self.cache_read_usd = round6(self.cache_read_usd + other.cache_read_usd);
        self.cache_creation_usd = round6(self.cache_creation_usd + other.cache_creation_usd);
        self.total_usd = round6(
            self.prompt_usd + self.completion_usd + self.cache_read_usd + self.cache_creation_usd,
        );
        self.estimated |= other.estimated;
    }
}

impl From<nasiko_pricing::CostBreakdown> for CostBreakdown {
    fn from(cost: nasiko_pricing::CostBreakdown) -> Self {
        Self {
            prompt_usd: cost.input_usd,
            completion_usd: cost.output_usd,
            cache_read_usd: cost.cache_read_usd,
            cache_creation_usd: cost.cache_creation_usd,
            total_usd: cost.total_usd,
            estimated: cost.estimated,
        }
    }
}

/// Cost one call through the platform's single pricing engine.
pub async fn compute_cost(engine: &PricingEngine, request: CostRequest<'_>) -> CostBreakdown {
    engine
        .price_with_context(
            request.provider,
            request.model.unwrap_or_default(),
            RawUsage {
                input: request.input_tokens,
                output: request.output_tokens,
                cache_read: request.cache_read_tokens,
                cache_creation: request.cache_creation_tokens,
                total: None,
            },
            // Span usage reaches us already split by `extract_usage_attrs`,
            // which resolves the provider's prompt convention up front.
            PromptConvention::Exclusive,
            request.at,
            request.context,
        )
        .await
        .cost
        .into()
}

pub(crate) fn round6(v: f64) -> f64 {
    (v * 1_000_000.0).round() / 1_000_000.0
}

/// One curated seed row for `model_pricing` — USD per 1M tokens,
/// best-effort public list rates.
pub struct SeedPrice {
    pub provider: &'static str,
    pub model: &'static str,
    pub input_per_1m: f64,
    pub output_per_1m: f64,
    pub cache_creation_per_1m: Option<f64>,
    pub cache_read_per_1m: Option<f64>,
}

/// Declare a [`SeedPrice`] with less noise.
macro_rules! seed {
    ($provider:literal, $model:literal, $in:expr, $out:expr) => {
        SeedPrice {
            provider: $provider,
            model: $model,
            input_per_1m: $in,
            output_per_1m: $out,
            cache_creation_per_1m: None,
            cache_read_per_1m: None,
        }
    };
    ($provider:literal, $model:literal, $in:expr, $out:expr, $cw:expr, $cr:expr) => {
        SeedPrice {
            provider: $provider,
            model: $model,
            input_per_1m: $in,
            output_per_1m: $out,
            cache_creation_per_1m: Some($cw),
            cache_read_per_1m: Some($cr),
        }
    };
}

/// Curated seed rows for `model_pricing`: USD per 1M tokens, best-effort
/// public list rates. This is the offline baseline only: the LLM router's
/// pricing-sync loop (`oss/llm-router/src/routing/pricing_sync.rs`) refreshes
/// rows from the Portkey price book once provider keys are configured. VERIFY
/// against current provider pricing before relying on cost figures.
///
/// The Anthropic, OpenAI and DeepSeek rows were audited against the upstream
/// book on 2026-09-29 (migration 0047 carries the same corrections for databases
/// already seeded). The Gemini and Groq rows were not. Portkey keys Google by
/// context tier (`gemini-2.5-pro-lte-128k`); the sync now also writes each such
/// model under its bare name at its lowest tier (`with_bare_tier_names`), so a
/// synced Gemini price supersedes the seed row here once a sync has run.
///
/// Deliberately code, not a migration: price updates ship with the binary
/// instead of requiring a new migration per price change.
pub const SEED_PRICING: &[SeedPrice] = &[
    seed!("openai", "gpt-4o", 2.50, 10.00),
    seed!("openai", "gpt-4o-mini", 0.15, 0.60),
    seed!("openai", "gpt-4.1", 2.00, 8.00),
    seed!("openai", "gpt-4.1-mini", 0.40, 1.60),
    seed!("openai", "gpt-4.1-nano", 0.10, 0.40),
    seed!("openai", "gpt-4-turbo", 10.00, 30.00),
    seed!("openai", "gpt-3.5-turbo", 0.50, 1.50),
    seed!("openai", "o1-preview", 15.00, 60.00),
    seed!("openai", "o1-mini", 1.10, 4.40, 0.00, 0.55),
    seed!("openai", "o3", 2.00, 8.00, 0.00, 0.50),
    seed!("openai", "o3-mini", 1.10, 4.40),
    seed!("openai", "text-embedding-3-small", 0.02, 0.00),
    seed!("openai", "text-embedding-3-large", 0.13, 0.00),
    seed!("anthropic", "claude-opus-4", 15.00, 75.00, 18.75, 1.50),
    seed!("anthropic", "claude-sonnet-4", 3.00, 15.00, 3.75, 0.30),
    seed!("anthropic", "claude-haiku-4", 0.80, 4.00, 1.00, 0.08),
    // Anthropic re-priced mid-family, so the three rows above are not safe
    // family fallbacks for every point release: `claude-opus-4-8` reduces to
    // `claude-opus-4` and would price at 15/75 instead of 5/25, and
    // `claude-haiku-4-5` at 0.80/4.00 instead of 1.00/5.00. Seeding the point
    // releases keeps the family probe from ever being reached for them.
    seed!("anthropic", "claude-opus-4-5", 5.00, 25.00, 6.25, 0.50),
    seed!("anthropic", "claude-opus-4-6", 5.00, 25.00, 6.25, 0.50),
    seed!("anthropic", "claude-opus-4-7", 5.00, 25.00, 6.25, 0.50),
    seed!("anthropic", "claude-opus-4-8", 5.00, 25.00, 6.25, 0.50),
    seed!("anthropic", "claude-haiku-4-5", 1.00, 5.00, 1.25, 0.10),
    seed!("anthropic", "claude-3-5-sonnet", 3.00, 15.00),
    seed!("anthropic", "claude-3-5-haiku", 0.80, 4.00),
    seed!(
        "anthropic",
        "claude-3-5-sonnet-20241022",
        3.00,
        15.00,
        3.75,
        0.30
    ),
    seed!(
        "anthropic",
        "claude-3-5-haiku-20241022",
        0.80,
        4.00,
        1.00,
        0.08
    ),
    // `gemini`, not `google` — the router's provider label is what lands in
    // `token_usage.provider`, and `calculate_token_cost` matches (provider, model)
    // exactly, so a `google`-labelled row can never price a Gemini call.
    seed!("gemini", "gemini-2.5-pro", 1.25, 10.00),
    seed!("gemini", "gemini-2.5-flash", 0.30, 2.50),
    seed!("gemini", "gemini-1.5-pro", 1.25, 5.00),
    seed!("gemini", "gemini-1.5-flash", 0.075, 0.30),
    seed!("gemini", "gemini-2.0-flash", 0.10, 0.40),
    seed!("groq", "llama-3.3-70b-versatile", 0.59, 0.79),
    seed!("groq", "llama-3.1-8b-instant", 0.05, 0.08),
    seed!("deepseek", "deepseek-chat", 0.14, 0.28, 0.00, 0.0028),
    seed!("deepseek", "deepseek-reasoner", 0.14, 0.28, 0.00, 0.0028),
    seed!("deepseek", "deepseek-v4-flash", 0.14, 0.28),
    seed!("deepseek", "deepseek-v4-pro", 0.435, 0.87, 0.00, 0.0036),
];

/// Seed `model_pricing` from [`SEED_PRICING`] at server boot.
///
/// Gap-filling, never overwriting: a row is inserted only when the
/// `(provider, model)` pair has NO currently-active pricing row, so
/// operator-set prices and pricing-sync history always win and re-boots are
/// idempotent. Best-effort — a failure is logged, not fatal (cost falls back
/// to the shared engine's static book and inferred rates).
pub async fn seed_model_pricing(db: &sqlx::PgPool) {
    let mut inserted = 0u32;
    for row in SEED_PRICING {
        let res = sqlx::query(
            "INSERT INTO model_pricing \
             (provider, model, input_price_per_1m, output_price_per_1m, \
              cache_creation_price_per_1m, cache_read_price_per_1m, notes) \
             SELECT $1, $2, $3, $4, $5, $6, 'boot seed (static list)' \
             WHERE NOT EXISTS ( \
                 SELECT 1 FROM model_pricing \
                 WHERE provider = $1 AND model = $2 AND effective_until IS NULL \
             )",
        )
        .bind(row.provider)
        .bind(row.model)
        .bind(row.input_per_1m)
        .bind(row.output_per_1m)
        .bind(row.cache_creation_per_1m)
        .bind(row.cache_read_per_1m)
        .execute(db)
        .await;
        match res {
            Ok(done) => inserted += done.rows_affected() as u32,
            Err(e) => {
                tracing::warn!(provider = row.provider, model = row.model, error = %e, "model pricing seed failed (non-fatal)")
            }
        }
    }
    if inserted > 0 {
        tracing::info!(inserted, "model pricing seeded from static list");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn priced(input: f64, output: f64, read: f64, creation: f64) -> nasiko_pricing::CostBreakdown {
        nasiko_pricing::CostBreakdown {
            input_usd: input,
            output_usd: output,
            cache_read_usd: read,
            cache_creation_usd: creation,
            total_usd: input + output + read + creation,
            estimated: false,
        }
    }

    #[test]
    fn the_engines_classes_map_onto_the_published_field_names() {
        // `prompt`/`completion` is the wire spelling the UI reads; the engine
        // speaks `input`/`output`. This mapping is the only place the two meet.
        let cost: CostBreakdown = priced(1.0, 2.0, 0.5, 0.25).into();
        assert_eq!(cost.prompt_usd, 1.0);
        assert_eq!(cost.completion_usd, 2.0);
        assert_eq!(cost.cache_read_usd, 0.5);
        assert_eq!(cost.cache_creation_usd, 0.25);
        assert_eq!(cost.total_usd, 3.75);
    }

    #[test]
    fn accumulating_keeps_the_total_equal_to_its_parts() {
        let mut total = CostBreakdown::default();
        for _ in 0..3 {
            total.add_assign(priced(1.0, 2.0, 0.5, 0.25).into());
        }
        let parts = total.prompt_usd
            + total.completion_usd
            + total.cache_read_usd
            + total.cache_creation_usd;
        assert!((total.total_usd - parts).abs() < 1e-9);
    }

    #[test]
    fn one_estimated_call_makes_the_running_total_an_estimate() {
        let mut total: CostBreakdown = priced(1.0, 1.0, 0.0, 0.0).into();
        assert!(!total.estimated);
        let mut estimate = priced(1.0, 1.0, 0.0, 0.0);
        estimate.estimated = true;
        total.add_assign(estimate.into());
        assert!(total.estimated);
    }

    #[test]
    fn a_span_usage_becomes_a_request_without_reinterpreting_its_classes() {
        let usage = crate::types::SpanUsage {
            input: 10,
            output: 20,
            cache_read: 30,
            cache_creation: 40,
            model: Some("gpt-4o".into()),
            ..Default::default()
        };
        let at = Utc::now();
        let request = CostRequest::from_usage(Some("openai"), Some("gpt-4o"), at, &usage);
        assert_eq!(request.input_tokens, 10);
        assert_eq!(request.output_tokens, 20);
        assert_eq!(request.cache_read_tokens, 30);
        assert_eq!(request.cache_creation_tokens, 40);
        assert_eq!(request.provider, Some("openai"));
    }
}

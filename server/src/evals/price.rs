//! Pricing a token count the router never saw, with the router's own engine.
//!
//! An eval compares a replay's cost with the original session's. The replay is priced by the
//! router call by call; the original (a Claude Code session, say) only has token counts. Pricing
//! those here, through the same `PricingEngine` and price book, keeps both sides of the comparison
//! on the same rates.

use axum::http::StatusCode;
use chrono::Utc;
use nasiko_pricing::{PricingEngine, PromptConvention, RawUsage};
use serde::{Deserialize, Serialize};

use super::ApiError;

/// `POST /api/evals/price` body. Token classes are disjoint, as Anthropic reports them:
/// `input_tokens` excludes both cache counts.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct PriceRequest {
    /// The provider the model is served by (`anthropic`, `openai`, …); helps resolve a bare model
    /// name. Omitted, the model name alone decides.
    #[serde(default)]
    pub provider: Option<String>,
    pub model: String,
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default)]
    pub cache_read_tokens: u64,
    #[serde(default)]
    pub cache_creation_tokens: u64,
}

/// What the tokens cost at today's rates.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct PricedTokens {
    pub cost_usd: f64,
    /// Where the rates came from: `Exact`, `Family`, `Static`, `Ratio` or `Default`.
    pub source: String,
    /// True when any rate was inferred rather than looked up.
    pub estimated: bool,
}

pub(super) async fn price(
    engine: &PricingEngine,
    request: PriceRequest,
) -> Result<PricedTokens, ApiError> {
    let model = request.model.trim();
    if model.is_empty() {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            "invalid_model",
            "model must not be empty",
        ));
    }
    let priced = engine
        .price(
            request.provider.as_deref().filter(|p| !p.trim().is_empty()),
            model,
            RawUsage {
                input: request.input_tokens,
                output: request.output_tokens,
                cache_read: request.cache_read_tokens,
                cache_creation: request.cache_creation_tokens,
                total: None,
            },
            PromptConvention::Exclusive,
            Utc::now(),
        )
        .await;
    Ok(PricedTokens {
        cost_usd: priced.cost.total_usd,
        source: format!("{:?}", priced.quote.source),
        estimated: priced.cost.estimated,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(model: &str) -> PriceRequest {
        PriceRequest {
            provider: Some("anthropic".into()),
            model: model.into(),
            input_tokens: 1_000_000,
            output_tokens: 1_000_000,
            cache_read_tokens: 0,
            cache_creation_tokens: 0,
        }
    }

    #[tokio::test]
    async fn an_empty_model_is_refused() {
        let error = price(&PricingEngine::offline(), request("  "))
            .await
            .expect_err("empty model");
        assert_eq!(error.1, "invalid_model");
    }

    #[tokio::test]
    async fn a_listed_model_is_priced_from_the_list_and_matches_the_engine() {
        let engine = PricingEngine::offline();
        let priced = price(&engine, request("claude-sonnet-4-20250514"))
            .await
            .unwrap();
        let direct = engine
            .price(
                Some("anthropic"),
                "claude-sonnet-4-20250514",
                RawUsage {
                    input: 1_000_000,
                    output: 1_000_000,
                    cache_read: 0,
                    cache_creation: 0,
                    total: None,
                },
                PromptConvention::Exclusive,
                Utc::now(),
            )
            .await;
        assert_eq!(priced.cost_usd, direct.cost.total_usd);
        assert_eq!(priced.source, format!("{:?}", direct.quote.source));
        assert!(priced.cost_usd > 0.0);
    }

    #[tokio::test]
    async fn no_tokens_cost_nothing() {
        let mut empty = request("claude-sonnet-4-20250514");
        empty.input_tokens = 0;
        empty.output_tokens = 0;
        let priced = price(&PricingEngine::offline(), empty).await.unwrap();
        assert_eq!(priced.cost_usd, 0.0);
    }
}

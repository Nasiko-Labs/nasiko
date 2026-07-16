//! Ordered fallback execution.
//!
//! Try the primary resolved config, then each `fallback_models` entry in order; the
//! first success wins, and only after all are exhausted do we surface a 502. Each
//! call returns the **effective** provider/model actually used, so usage logging
//! records the fallback when one fires.
//!
//! Key selection (decision §6.5): a fallback whose provider matches the primary reuses
//! the resolved key; a cross-provider fallback uses that provider's platform key (see
//! [`GatewayConfig::platform_key_for`]). A fallback with no usable key is skipped.

use futures::stream::BoxStream;

use super::{ProviderError, provider_for};
use crate::config::GatewayConfig;
use crate::error::GatewayError;
use crate::ir::{ChatChunk, ChatRequest, ChatResponse, EmbeddingsRequest, EmbeddingsResponse};
use crate::resolver::ResolvedConfig;

/// The provider/model actually used for a call.
type Effective = (String, String);

/// Run a non-streaming chat with ordered fallbacks. Returns the response and the
/// effective `(provider, model)`.
pub async fn execute_chat(
    http: &reqwest::Client,
    cfg: &GatewayConfig,
    primary: &ResolvedConfig,
    req: &ChatRequest,
) -> Result<(ChatResponse, Effective), GatewayError> {
    let attempts = build_attempts(primary, cfg);
    let mut last: Option<GatewayError> = None;
    let total = attempts.len();
    for (i, attempt) in attempts.iter().enumerate() {
        log_request("chat", attempt, i, total);
        match provider_for(&attempt.provider, http, cfg) {
            Err(e) => last = Some(e),
            Ok(provider) => match provider.chat(req, attempt).await {
                Ok(resp) => {
                    log_response("chat", attempt, &resp);
                    return Ok((resp, (attempt.provider.clone(), attempt.model.clone())));
                }
                Err(e) => {
                    warn_attempt(attempt, &e, i, total);
                    last = Some(e.into());
                }
            },
        }
    }
    Err(last.unwrap_or_else(|| GatewayError::Upstream("no provider attempts".to_string())))
}

/// Run a streaming chat with ordered fallbacks (fallback applies to the *initial*
/// connect failure; once bytes flow, mid-stream errors end the stream). Returns the
/// chunk stream and the effective `(provider, model)`.
pub async fn execute_chat_stream(
    http: &reqwest::Client,
    cfg: &GatewayConfig,
    primary: &ResolvedConfig,
    req: &ChatRequest,
) -> Result<(BoxStream<'static, Result<ChatChunk, ProviderError>>, Effective), GatewayError> {
    let attempts = build_attempts(primary, cfg);
    let mut last: Option<GatewayError> = None;
    let total = attempts.len();
    for (i, attempt) in attempts.iter().enumerate() {
        log_request("chat_stream", attempt, i, total);
        match provider_for(&attempt.provider, http, cfg) {
            Err(e) => last = Some(e),
            Ok(provider) => match provider.chat_stream(req, attempt).await {
                Ok(stream) => {
                    tracing::info!(
                        target: "nasiko::llm_router::provider",
                        provider = %attempt.provider,
                        model = %attempt.model,
                        "provider response ← chat_stream established (body streamed as SSE chunks)"
                    );
                    return Ok((stream, (attempt.provider.clone(), attempt.model.clone())));
                }
                Err(e) => {
                    warn_attempt(attempt, &e, i, total);
                    last = Some(e.into());
                }
            },
        }
    }
    Err(last.unwrap_or_else(|| GatewayError::Upstream("no provider attempts".to_string())))
}

/// Run embeddings with ordered fallbacks (always non-streaming). Returns the response
/// and the effective `(provider, model)`. Same key rules as chat; usefully, a primary
/// that has no embeddings API (e.g. Anthropic → 501) now falls back instead of hard-failing.
pub async fn execute_embeddings(
    http: &reqwest::Client,
    cfg: &GatewayConfig,
    primary: &ResolvedConfig,
    req: &EmbeddingsRequest,
) -> Result<(EmbeddingsResponse, Effective), GatewayError> {
    let attempts = build_attempts(primary, cfg);
    let mut last: Option<GatewayError> = None;
    let total = attempts.len();
    for (i, attempt) in attempts.iter().enumerate() {
        log_request("embeddings", attempt, i, total);
        match provider_for(&attempt.provider, http, cfg) {
            Err(e) => last = Some(e),
            Ok(provider) => match provider.embeddings(req, attempt).await {
                Ok(resp) => {
                    log_response("embeddings", attempt, &resp);
                    return Ok((resp, (attempt.provider.clone(), attempt.model.clone())));
                }
                Err(e) => {
                    warn_attempt(attempt, &e, i, total);
                    last = Some(e.into());
                }
            },
        }
    }
    Err(last.unwrap_or_else(|| GatewayError::Upstream("no provider attempts".to_string())))
}

/// Log the outbound request to an upstream LLM — provider + model only (never the
/// prompt/messages or the api key). Emitted per attempt so fallbacks are visible.
fn log_request(op: &str, attempt: &ResolvedConfig, i: usize, total: usize) {
    tracing::info!(
        target: "nasiko::llm_router::provider",
        op,
        provider = %attempt.provider,
        model = %attempt.model,
        attempt = i + 1,
        of = total,
        "provider request → dispatching {op} to upstream LLM (provider/model)"
    );
}

/// Log the response body returned by the upstream LLM. `info!` so it shows at the
/// default log level; bodies can be large, so filter this target down to `warn` if
/// it gets noisy (`RUST_LOG=nasiko::llm_router::provider=warn`).
fn log_response<T: serde::Serialize>(op: &str, attempt: &ResolvedConfig, resp: &T) {
    tracing::info!(
        target: "nasiko::llm_router::provider",
        op,
        provider = %attempt.provider,
        model = %attempt.model,
        response_body = %serde_json::to_string(resp)
            .unwrap_or_else(|e| format!("<serialize error: {e}>")),
        "provider response ← {op} body"
    );
}

fn warn_attempt(attempt: &ResolvedConfig, err: &ProviderError, i: usize, total: usize) {
    let more = i + 1 < total;
    tracing::warn!(
        provider = %attempt.provider, model = %attempt.model, error = %err,
        "llm attempt failed{}", if more { "; trying fallback" } else { "; exhausted" }
    );
}

/// Build the ordered attempt list: the primary, then each usable fallback.
pub(crate) fn build_attempts(primary: &ResolvedConfig, cfg: &GatewayConfig) -> Vec<ResolvedConfig> {
    let mut attempts = vec![ResolvedConfig {
        fallback_models: Vec::new(),
        ..primary.clone()
    }];

    for entry in &primary.fallback_models {
        let (provider, model) = split_prefixed(entry, &primary.provider);
        // Same provider ⇒ reuse the resolved key (may be a per-user secret). A
        // cross-provider fallback can't use that key, so fall back to the platform
        // key for that provider.
        let api_key = if provider == primary.provider {
            primary.api_key.clone()
        } else {
            cfg.platform_key_for(&provider).to_string()
        };
        if api_key.is_empty() {
            tracing::warn!(%entry, "skipping fallback: no api key available");
            continue;
        }
        attempts.push(ResolvedConfig {
            provider,
            model,
            litellm_model: entry.clone(),
            api_key,
            fallback_models: Vec::new(),
            temperature: primary.temperature,
            max_tokens: primary.max_tokens,
            has_llm_config: primary.has_llm_config,
            // Fallback attempts are never pinned — pinning disables fallbacks upstream.
            pinned_model: None,
        });
    }
    attempts
}

/// Split a `"provider/model"` id; an unprefixed entry inherits the primary provider.
fn split_prefixed(entry: &str, default_provider: &str) -> (String, String) {
    match entry.split_once('/') {
        Some((p, m)) => (p.to_string(), m.to_string()),
        None => (default_provider.to_string(), entry.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn cfg(platform_key: &str) -> GatewayConfig {
        GatewayConfig {
            platform_openai_api_key: platform_key.into(),
            ..Default::default()
        }
    }

    fn primary(provider: &str, fallbacks: Vec<&str>) -> ResolvedConfig {
        ResolvedConfig {
            provider: provider.into(),
            model: "primary-model".into(),
            litellm_model: format!("{provider}/primary-model"),
            api_key: "primary-key".into(),
            fallback_models: fallbacks.into_iter().map(String::from).collect(),
            temperature: Some(0.3),
            max_tokens: Some(100),
            has_llm_config: false,
            pinned_model: None,
        }
    }

    #[test]
    fn same_provider_fallback_reuses_key_cross_provider_uses_platform() {
        let p = primary("anthropic", vec!["anthropic/claude-haiku", "openai/gpt-4o-mini"]);
        let attempts = build_attempts(&p, &cfg("sk-platform"));
        assert_eq!(attempts.len(), 3);
        // primary
        assert_eq!(attempts[0].model, "primary-model");
        assert!(attempts[0].fallback_models.is_empty());
        // same-provider fallback → reuse primary key
        assert_eq!(attempts[1].provider, "anthropic");
        assert_eq!(attempts[1].api_key, "primary-key");
        // cross-provider fallback → platform key + carried params
        assert_eq!(attempts[2].provider, "openai");
        assert_eq!(attempts[2].model, "gpt-4o-mini");
        assert_eq!(attempts[2].api_key, "sk-platform");
        assert_eq!(attempts[2].temperature, Some(0.3));
        assert_eq!(attempts[2].max_tokens, Some(100));
    }

    #[test]
    fn cross_provider_fallback_skipped_without_platform_key() {
        let p = primary("anthropic", vec!["openai/gpt-4o-mini"]);
        let attempts = build_attempts(&p, &cfg("")); // no platform key
        assert_eq!(attempts.len(), 1); // fallback skipped
        assert_eq!(attempts[0].provider, "anthropic");
    }

    #[tokio::test]
    async fn primary_failure_falls_back_to_openai_and_reports_effective() {
        // Primary Anthropic returns 401 (bad key); fallback OpenAI succeeds.
        let mut anthropic = mockito::Server::new_async().await;
        anthropic
            .mock("POST", "/messages")
            .with_status(401)
            .with_body("invalid x-api-key")
            .create_async()
            .await;
        let mut openai = mockito::Server::new_async().await;
        openai
            .mock("POST", "/chat/completions")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                json!({
                    "id": "chatcmpl-1", "object": "chat.completion", "model": "gpt-4o-mini",
                    "choices": [{ "index": 0, "message": { "role": "assistant", "content": "ok" }, "finish_reason": "stop" }],
                    "usage": { "prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2 }
                })
                .to_string(),
            )
            .create_async()
            .await;

        let cfg = GatewayConfig {
            anthropic_api_base: anthropic.url(),
            openai_api_base: openai.url(),
            platform_openai_api_key: "sk-platform".into(),
            ..Default::default()
        };
        let primary = ResolvedConfig {
            provider: "anthropic".into(),
            model: "claude-3-5-sonnet-20241022".into(),
            litellm_model: "anthropic/claude-3-5-sonnet-20241022".into(),
            api_key: "sk-bad".into(),
            fallback_models: vec!["openai/gpt-4o-mini".into()],
            temperature: None,
            max_tokens: None,
            has_llm_config: false,
            pinned_model: None,
        };
        let req: ChatRequest =
            serde_json::from_value(json!({ "messages": [{ "role": "user", "content": "hi" }] }))
                .unwrap();

        let (resp, (provider, model)) =
            execute_chat(&reqwest::Client::new(), &cfg, &primary, &req).await.unwrap();
        assert_eq!(provider, "openai"); // effective = the fallback
        assert_eq!(model, "gpt-4o-mini");
        assert_eq!(resp.choices[0].message.text().as_deref(), Some("ok"));
    }

    #[tokio::test]
    async fn embeddings_fall_back_across_providers() {
        // Primary provider is Anthropic (no embeddings API → 501); fallback OpenAI succeeds.
        let mut openai = mockito::Server::new_async().await;
        openai
            .mock("POST", "/embeddings")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                json!({
                    "object": "list",
                    "data": [{ "object": "embedding", "embedding": [0.1, 0.2], "index": 0 }],
                    "model": "text-embedding-3-small",
                    "usage": { "prompt_tokens": 2, "total_tokens": 2 }
                })
                .to_string(),
            )
            .create_async()
            .await;

        let cfg = GatewayConfig {
            openai_api_base: openai.url(),
            platform_openai_api_key: "sk-platform".into(),
            ..Default::default()
        };
        let primary = ResolvedConfig {
            provider: "anthropic".into(),
            model: "claude-3-5-sonnet-20241022".into(),
            litellm_model: "anthropic/claude-3-5-sonnet-20241022".into(),
            api_key: "sk-ant".into(),
            fallback_models: vec!["openai/text-embedding-3-small".into()],
            temperature: None,
            max_tokens: None,
            has_llm_config: false,
            pinned_model: None,
        };
        let req: EmbeddingsRequest =
            serde_json::from_value(json!({ "model": "x", "input": "hi" })).unwrap();

        let (resp, (provider, model)) =
            execute_embeddings(&reqwest::Client::new(), &cfg, &primary, &req).await.unwrap();
        assert_eq!(provider, "openai"); // effective = the fallback
        assert_eq!(model, "text-embedding-3-small");
        assert_eq!(resp.data.len(), 1);
    }

    #[tokio::test]
    async fn all_attempts_exhausted_is_502() {
        let mut openai = mockito::Server::new_async().await;
        openai
            .mock("POST", "/chat/completions")
            .with_status(500)
            .with_body("boom")
            .expect_at_least(1)
            .create_async()
            .await;
        let cfg = GatewayConfig {
            openai_api_base: openai.url(),
            platform_openai_api_key: "sk-platform".into(),
            ..Default::default()
        };
        // primary openai + one openai fallback, both hit the failing mock
        let primary = ResolvedConfig {
            provider: "openai".into(),
            model: "gpt-4o".into(),
            litellm_model: "openai/gpt-4o".into(),
            api_key: "sk-x".into(),
            fallback_models: vec!["openai/gpt-4o-mini".into()],
            temperature: None,
            max_tokens: None,
            has_llm_config: false,
            pinned_model: None,
        };
        let req: ChatRequest =
            serde_json::from_value(json!({ "messages": [{ "role": "user", "content": "hi" }] }))
                .unwrap();
        let err = execute_chat(&reqwest::Client::new(), &cfg, &primary, &req).await.unwrap_err();
        assert!(matches!(err, GatewayError::Upstream(_)));
        assert_eq!(err.status(), axum::http::StatusCode::BAD_GATEWAY);
    }
}

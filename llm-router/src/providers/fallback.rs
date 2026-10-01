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

use super::{ProviderClient, ProviderError, provider_for};
use crate::config::GatewayConfig;
use crate::error::GatewayError;
use crate::ir::{ChatChunk, ChatRequest, ChatResponse, EmbeddingsRequest, EmbeddingsResponse};
use crate::resolver::ResolvedConfig;

/// The provider/model actually used for a call.
type Effective = (String, String);

/// Cap on how many distinct parameters we'll adjust-and-retry against one model before
/// giving up on it. A backstop against a provider that keeps rejecting params; real
/// mismatches resolve in one or two fixes.
const MAX_PARAM_FIXES: usize = 4;

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
        let provider = match provider_for(attempt, http, cfg) {
            Ok(p) => p,
            Err(e) => {
                last = Some(e);
                continue;
            }
        };
        // Working copies so a parameter-fix retry can mutate the request + config.
        let mut work_req = req.clone();
        let mut work_cfg = attempt.clone();
        let mut applied: Vec<String> = Vec::new();
        loop {
            match provider.chat(&work_req, &work_cfg).await {
                Ok(resp) => {
                    log_response("chat", attempt, &resp);
                    return Ok((resp, (attempt.provider.clone(), attempt.model.clone())));
                }
                Err(e) => {
                    if try_fix_param(&*provider, &e, &mut work_req, &mut work_cfg, &mut applied) {
                        log_param_fix(attempt, applied.last().unwrap(), &e);
                        continue;
                    }
                    warn_attempt(attempt, &e, i, total);
                    last = Some(e.into());
                    break;
                }
            }
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
) -> Result<
    (
        BoxStream<'static, Result<ChatChunk, ProviderError>>,
        Effective,
    ),
    GatewayError,
> {
    let attempts = build_attempts(primary, cfg);
    let mut last: Option<GatewayError> = None;
    let total = attempts.len();
    for (i, attempt) in attempts.iter().enumerate() {
        log_request("chat_stream", attempt, i, total);
        let provider = match provider_for(attempt, http, cfg) {
            Ok(p) => p,
            Err(e) => {
                last = Some(e);
                continue;
            }
        };
        // The initial connect (status check) happens before any chunk flows, so a
        // parameter-rejection surfaces here and is safe to fix-and-retry — same as `chat`.
        let mut work_req = req.clone();
        let mut work_cfg = attempt.clone();
        let mut applied: Vec<String> = Vec::new();
        loop {
            match provider.chat_stream(&work_req, &work_cfg).await {
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
                    if try_fix_param(&*provider, &e, &mut work_req, &mut work_cfg, &mut applied) {
                        log_param_fix(attempt, applied.last().unwrap(), &e);
                        continue;
                    }
                    warn_attempt(attempt, &e, i, total);
                    last = Some(e.into());
                    break;
                }
            }
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
        match provider_for(attempt, http, cfg) {
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

/// Try to recover from `err` by adjusting a rejected parameter and retrying the *same*
/// model. Two repairs, in order: drop a param the provider says it doesn't accept, or
/// set one it says it needs at a specific value. Either way the param is recorded in
/// `applied`, which caps the total and guards the loop. Returns `true` when the caller
/// should retry.
pub(crate) fn try_fix_param(
    provider: &dyn ProviderClient,
    err: &ProviderError,
    req: &mut ChatRequest,
    cfg: &mut ResolvedConfig,
    applied: &mut Vec<String>,
) -> bool {
    if applied.len() >= MAX_PARAM_FIXES {
        return false;
    }
    // Guard against a provider that re-reports the same param: if we already adjusted it
    // (or the adjustment is a no-op), retrying would just fail identically — give up.
    if let Some(param) = provider.droppable_param(err) {
        if applied.iter().any(|a| a == &param) || !strip_param(req, cfg, &param) {
            return false;
        }
        applied.push(param);
        return true;
    }
    let Some((param, value)) = provider.repairable_param(err) else {
        return false;
    };
    if applied.iter().any(|a| a == &param) || req.extra.get(&param) == Some(&value) {
        return false;
    }
    req.extra.insert(param.clone(), value);
    applied.push(param);
    true
}

/// Remove `param` from both the request and the resolved config so the retry omits it.
/// Named params (`temperature`, `max_tokens`) live on both structs; anything else is a
/// passthrough field in the request's `extra` map. `max_completion_tokens` is the wire
/// alias OpenAI uses for `max_tokens`. Returns whether anything was actually removed.
fn strip_param(req: &mut ChatRequest, cfg: &mut ResolvedConfig, param: &str) -> bool {
    match param {
        "temperature" => {
            let had = req.temperature.is_some() || cfg.temperature.is_some();
            req.temperature = None;
            cfg.temperature = None;
            had
        }
        "max_tokens" | "max_completion_tokens" => {
            let had = req.max_tokens.is_some() || cfg.max_tokens.is_some();
            req.max_tokens = None;
            cfg.max_tokens = None;
            had
        }
        other => req.extra.remove(other).is_some(),
    }
}

fn log_param_fix(attempt: &ResolvedConfig, param: &str, err: &ProviderError) {
    tracing::warn!(
        target: "nasiko::llm_router::provider",
        provider = %attempt.provider, model = %attempt.model, param, error = %err,
        "upstream rejected parameter; adjusting it and retrying the same model"
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
        // Same-provider fallbacks reuse the primary's key (and payer); cross-provider
        // fallbacks always use the platform key.
        let platform_paid = if provider == primary.provider {
            primary.platform_paid
        } else {
            true
        };
        // A same-provider fallback inherits the primary's endpoint, so a custom
        // provider's own fallback still targets it in the same dialect; a
        // cross-provider fallback uses the built-in base URL (and a custom
        // cross-provider name has no platform key, so it is already skipped by the
        // `is_empty` guard above).
        let custom_endpoint = if provider == primary.provider {
            primary.custom_endpoint.clone()
        } else {
            None
        };
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
            // Tier overrides don't apply to fallback attempts.
            tier1_model: None,
            tier2_model: None,
            tier3_model: None,
            platform_paid,
            custom_endpoint,
            is_coding_agent: primary.is_coding_agent,
            compress_enabled: primary.compress_enabled,
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
            tier1_model: None,
            tier2_model: None,
            tier3_model: None,
            platform_paid: true,
            custom_endpoint: None,
            is_coding_agent: false,
            compress_enabled: false,
        }
    }

    /// A minimal tool-carrying request, the shape Claude Code sends every turn.
    fn chat_req() -> ChatRequest {
        serde_json::from_value(json!({
            "messages": [{ "role": "user", "content": "hi" }],
            "tools": [{
                "type": "function",
                "function": { "name": "Read", "parameters": { "type": "object" } }
            }]
        }))
        .expect("valid chat request")
    }

    /// The gpt-5.x rejection of function tools; `OpenAiProvider` classifies it purely
    /// from the error body, so no network is involved.
    fn reasoning_effort_rejection() -> ProviderError {
        ProviderError::Status {
            status: 400,
            message: json!({
                "error": {
                    "message": "Function tools with reasoning_effort are not supported for gpt-5.6 in /v1/chat/completions. To use function tools, use /v1/responses or set reasoning_effort to 'none'.",
                    "type": "invalid_request_error",
                    "param": "reasoning_effort",
                    "code": serde_json::Value::Null
                }
            })
            .to_string(),
            retryable: false,
        }
    }

    #[test]
    fn a_repairable_rejection_sets_the_param_and_retries_once() {
        let provider =
            crate::providers::OpenAiProvider::new(reqwest::Client::new(), "http://x".into());
        let err = reasoning_effort_rejection();
        let mut req = chat_req();
        let mut cfg = primary("openai", vec![]);
        let mut applied: Vec<String> = Vec::new();

        assert!(try_fix_param(
            &provider,
            &err,
            &mut req,
            &mut cfg,
            &mut applied
        ));
        assert_eq!(req.extra.get("reasoning_effort"), Some(&json!("none")));
        assert_eq!(applied, vec!["reasoning_effort".to_string()]);
        // The tools the caller sent are untouched — the repair is additive.
        assert!(req.tools.is_some());

        // Re-reported by the provider ⇒ the retry would fail identically. Give up
        // rather than loop.
        assert!(!try_fix_param(
            &provider,
            &err,
            &mut req,
            &mut cfg,
            &mut applied
        ));
        assert_eq!(applied.len(), 1);
    }

    #[test]
    fn a_droppable_rejection_still_strips_the_param() {
        let provider =
            crate::providers::OpenAiProvider::new(reqwest::Client::new(), "http://x".into());
        let err = ProviderError::Status {
            status: 400,
            message: json!({
                "error": {
                    "message": "Unsupported value: 'temperature' does not support 0.3 with this model.",
                    "param": "temperature",
                    "code": "unsupported_value"
                }
            })
            .to_string(),
            retryable: false,
        };
        let mut req = chat_req();
        let mut cfg = primary("openai", vec![]); // temperature: Some(0.3)
        let mut applied: Vec<String> = Vec::new();

        assert!(try_fix_param(
            &provider,
            &err,
            &mut req,
            &mut cfg,
            &mut applied
        ));
        assert_eq!(cfg.temperature, None);
        assert_eq!(applied, vec!["temperature".to_string()]);
        assert!(!req.extra.contains_key("reasoning_effort"));
    }

    #[test]
    fn an_unrecognized_error_is_never_fixed() {
        let provider =
            crate::providers::OpenAiProvider::new(reqwest::Client::new(), "http://x".into());
        let mut req = chat_req();
        let mut cfg = primary("openai", vec![]);
        let mut applied: Vec<String> = Vec::new();

        assert!(!try_fix_param(
            &provider,
            &ProviderError::Status {
                status: 401,
                message: json!({ "error": { "code": "invalid_api_key" } }).to_string(),
                retryable: false,
            },
            &mut req,
            &mut cfg,
            &mut applied
        ));
        assert!(applied.is_empty());
        assert!(req.extra.is_empty());
        assert_eq!(cfg.temperature, Some(0.3));
    }

    #[test]
    fn param_fixes_are_capped() {
        let provider =
            crate::providers::OpenAiProvider::new(reqwest::Client::new(), "http://x".into());
        let mut req = chat_req();
        let mut cfg = primary("openai", vec![]);
        let mut applied: Vec<String> = (0..MAX_PARAM_FIXES).map(|i| format!("param-{i}")).collect();

        assert!(!try_fix_param(
            &provider,
            &reasoning_effort_rejection(),
            &mut req,
            &mut cfg,
            &mut applied
        ));
        assert!(!req.extra.contains_key("reasoning_effort"));
    }

    #[test]
    fn same_provider_fallback_reuses_key_cross_provider_uses_platform() {
        let p = primary(
            "anthropic",
            vec!["anthropic/claude-haiku", "openai/gpt-4o-mini"],
        );
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

    #[test]
    fn cross_provider_fallback_to_custom_name_is_skipped() {
        // A fallback naming a custom (non-built-in) provider gets no platform key
        // (platform_key_for returns "" for non-built-ins), so it is skipped rather
        // than mis-keyed with the OpenAI key.
        let p = primary("openai", vec!["my-gateway/llama-3.1-70b"]);
        let attempts = build_attempts(&p, &cfg("sk-platform"));
        assert_eq!(attempts.len(), 1); // custom-named fallback skipped
        assert_eq!(attempts[0].provider, "openai");
    }

    #[test]
    fn same_provider_custom_fallback_inherits_endpoint() {
        // A same-provider fallback for a custom provider reuses the primary key and
        // carries the primary's endpoint — URL *and* dialect — so the attempt still
        // targets it the same way.
        let mut p = primary("azure-prod", vec!["azure-prod/prod-gpt4o-mini"]);
        let dialect = crate::providers::ProviderDialect::AzureOpenAi {
            api_version: "2024-10-21".into(),
        };
        p.custom_endpoint = Some(crate::resolver::CustomEndpoint {
            base_url: "https://acme.openai.azure.com".into(),
            dialect: dialect.clone(),
        });
        let attempts = build_attempts(&p, &cfg("sk-platform"));
        assert_eq!(attempts.len(), 2);
        assert_eq!(attempts[1].provider, "azure-prod");
        assert_eq!(attempts[1].api_key, "primary-key"); // same-provider ⇒ reuse key
        let endpoint = attempts[1]
            .custom_endpoint
            .as_ref()
            .expect("same-provider fallback keeps the endpoint");
        assert_eq!(endpoint.base_url, "https://acme.openai.azure.com");
        assert_eq!(endpoint.dialect, dialect);
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
            tier1_model: None,
            tier2_model: None,
            tier3_model: None,
            platform_paid: true,
            custom_endpoint: None,
            is_coding_agent: false,
            compress_enabled: false,
        };
        let req: ChatRequest =
            serde_json::from_value(json!({ "messages": [{ "role": "user", "content": "hi" }] }))
                .unwrap();

        let (resp, (provider, model)) = execute_chat(&reqwest::Client::new(), &cfg, &primary, &req)
            .await
            .unwrap();
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
            tier1_model: None,
            tier2_model: None,
            tier3_model: None,
            platform_paid: true,
            custom_endpoint: None,
            is_coding_agent: false,
            compress_enabled: false,
        };
        let req: EmbeddingsRequest =
            serde_json::from_value(json!({ "model": "x", "input": "hi" })).unwrap();

        let (resp, (provider, model)) =
            execute_embeddings(&reqwest::Client::new(), &cfg, &primary, &req)
                .await
                .unwrap();
        assert_eq!(provider, "openai"); // effective = the fallback
        assert_eq!(model, "text-embedding-3-small");
        assert_eq!(resp.data.len(), 1);
    }

    #[tokio::test]
    async fn param_rejection_is_dropped_and_retried_on_same_model() {
        // gpt-5.5 rejects a non-default temperature with a 400 naming the param. The
        // executor must drop `temperature` and retry the SAME model (no fallback needed).
        let mut openai = mockito::Server::new_async().await;
        // First call carries temperature → 400 param rejection.
        let rejected = openai
            .mock("POST", "/chat/completions")
            .match_body(mockito::Matcher::PartialJson(json!({ "temperature": 0.1 })))
            .with_status(400)
            .with_header("content-type", "application/json")
            .with_body(
                json!({
                    "error": {
                        "message": "Unsupported value: 'temperature' does not support 0.1 with this model. Only the default (1) value is supported.",
                        "type": "invalid_request_error",
                        "param": "temperature",
                        "code": "unsupported_value"
                    }
                })
                .to_string(),
            )
            .expect(1)
            .create_async()
            .await;
        // Retry without temperature → 200. (mockito matches the first mock whose body
        // matcher matches; the retry omits temperature so it falls through to this one.)
        let ok = openai
            .mock("POST", "/chat/completions")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                json!({
                    "id": "chatcmpl-1", "object": "chat.completion", "model": "gpt-5.5",
                    "choices": [{ "index": 0, "message": { "role": "assistant", "content": "ok" }, "finish_reason": "stop" }],
                    "usage": { "prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2 }
                })
                .to_string(),
            )
            .expect(1)
            .create_async()
            .await;

        let cfg = GatewayConfig {
            openai_api_base: openai.url(),
            platform_openai_api_key: "sk-platform".into(),
            ..Default::default()
        };
        // Temperature comes from the resolved config (as tier routing sets it); no fallbacks.
        let primary = ResolvedConfig {
            provider: "openai".into(),
            model: "gpt-5.5".into(),
            litellm_model: "openai/gpt-5.5".into(),
            api_key: "sk-x".into(),
            fallback_models: vec![],
            temperature: Some(0.1),
            max_tokens: None,
            has_llm_config: false,
            pinned_model: None,
            tier1_model: None,
            tier2_model: None,
            tier3_model: None,
            platform_paid: true,
            custom_endpoint: None,
            is_coding_agent: false,
            compress_enabled: false,
        };
        let req: ChatRequest =
            serde_json::from_value(json!({ "messages": [{ "role": "user", "content": "hi" }] }))
                .unwrap();

        let (resp, (provider, model)) = execute_chat(&reqwest::Client::new(), &cfg, &primary, &req)
            .await
            .unwrap();
        rejected.assert_async().await;
        ok.assert_async().await;
        assert_eq!(provider, "openai"); // recovered on the same model, no fallback
        assert_eq!(model, "gpt-5.5");
        assert_eq!(resp.choices[0].message.text().as_deref(), Some("ok"));
    }

    #[tokio::test]
    async fn anthropic_deprecated_param_is_dropped_and_retried_on_same_model() {
        // claude-opus-4-8 rejects `temperature` with a 400 naming it in the message text
        // (Anthropic's shape). Temperature comes from the request, the agent is pinned
        // (no fallbacks) — the executor must still drop it and retry the SAME model.
        let mut anthropic = mockito::Server::new_async().await;
        let rejected = anthropic
            .mock("POST", "/messages")
            .match_body(mockito::Matcher::PartialJson(json!({ "temperature": 0.7 })))
            .with_status(400)
            .with_header("content-type", "application/json")
            .with_body(
                json!({
                    "type": "error",
                    "error": {
                        "type": "invalid_request_error",
                        "message": "`temperature` is deprecated for this model."
                    }
                })
                .to_string(),
            )
            .expect(1)
            .create_async()
            .await;
        let ok = anthropic
            .mock("POST", "/messages")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                json!({
                    "id": "msg_01", "type": "message", "role": "assistant",
                    "content": [{ "type": "text", "text": "ok" }],
                    "stop_reason": "end_turn",
                    "usage": { "input_tokens": 5, "output_tokens": 1 }
                })
                .to_string(),
            )
            .expect(1)
            .create_async()
            .await;

        let cfg = GatewayConfig {
            anthropic_api_base: anthropic.url(),
            ..Default::default()
        };
        let primary = ResolvedConfig {
            provider: "anthropic".into(),
            model: "claude-opus-4-8".into(),
            litellm_model: "anthropic/claude-opus-4-8".into(),
            api_key: "sk-ant".into(),
            fallback_models: vec![], // pinned → no fallbacks
            temperature: None,       // config sets none; the request carries it
            max_tokens: None,
            has_llm_config: true,
            pinned_model: Some("claude-opus-4-8".into()),
            tier1_model: None,
            tier2_model: None,
            tier3_model: None,
            platform_paid: true,
            custom_endpoint: None,
            is_coding_agent: false,
            compress_enabled: false,
        };
        let req: ChatRequest = serde_json::from_value(json!({
            "temperature": 0.7,
            "messages": [{ "role": "user", "content": "hi" }]
        }))
        .unwrap();

        let (resp, (provider, model)) = execute_chat(&reqwest::Client::new(), &cfg, &primary, &req)
            .await
            .unwrap();
        rejected.assert_async().await;
        ok.assert_async().await;
        assert_eq!(provider, "anthropic"); // recovered on the same pinned model
        assert_eq!(model, "claude-opus-4-8");
        assert_eq!(resp.choices[0].message.text().as_deref(), Some("ok"));
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
            tier1_model: None,
            tier2_model: None,
            tier3_model: None,
            platform_paid: true,
            custom_endpoint: None,
            is_coding_agent: false,
            compress_enabled: false,
        };
        let req: ChatRequest =
            serde_json::from_value(json!({ "messages": [{ "role": "user", "content": "hi" }] }))
                .unwrap();
        let err = execute_chat(&reqwest::Client::new(), &cfg, &primary, &req)
            .await
            .unwrap_err();
        assert!(matches!(err, GatewayError::Upstream(_)));
        assert_eq!(err.status(), axum::http::StatusCode::BAD_GATEWAY);
    }
}

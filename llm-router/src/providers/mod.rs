//! Provider clients — the spokes of the hub.
//!
//! A [`ProviderClient`] takes the canonical IR + a [`ResolvedConfig`] and calls one
//! provider, returning IR. Per-provider impls (OpenAI / Anthropic / Gemini) land in
//! steps 4–6 and own the OpenAI⇄provider translation; the ordered fallback executor
//! lands in step 8. Errors are [`ProviderError`] (carrying retryability) and convert
//! to a `GatewayError` once fallbacks are exhausted (see `From<ProviderError>`).

use async_trait::async_trait;
use futures::stream::BoxStream;

use serde_json::{Map, Value};

use crate::config::GatewayConfig;
use crate::error::GatewayError;
use crate::ir::{
    ChatChunk, ChatRequest, ChatResponse, ChunkChoice, Delta, EmbeddingsRequest,
    EmbeddingsResponse, Usage,
};
use crate::resolver::{CustomEndpoint, ResolvedConfig, ResolvedEmbeddingConfig};

/// Construct the provider client for a destination. Used by the handlers and the
/// fallback executor. Built-in providers use their base URL from [`GatewayConfig`];
/// a custom (DB-registered) provider is OpenAI-shaped and uses `custom_endpoint` — its
/// base URL plus the [`ProviderDialect`] describing how that endpoint wants to be
/// addressed (plain OpenAI, Azure OpenAI, …). A non-built-in provider with no endpoint
/// is a server-side gap (500) — the resolver populates it for every registered custom
/// provider, so a missing one means the row was dropped mid-flight.
///
/// Takes the destination as two plain arguments rather than a `&ResolvedConfig`, because
/// the embedding path resolves to [`ResolvedEmbeddingConfig`] and must be able to build a
/// client without holding a chat-shaped struct.
pub fn provider_for(
    provider: &str,
    custom_endpoint: Option<&CustomEndpoint>,
    http: &reqwest::Client,
    cfg: &GatewayConfig,
) -> Result<Box<dyn ProviderClient>, GatewayError> {
    match provider {
        "openai" => Ok(Box::new(
            OpenAiProvider::new(http.clone(), cfg.openai_api_base.clone())
                .with_max_tools(cfg.openai_max_tools)
                .with_cache_key(cfg.cache_markers_enabled)
                .with_document_urls_refused(true)
                .with_strict_media(true),
        )),
        "anthropic" => Ok(Box::new(
            AnthropicProvider::new(http.clone(), cfg.anthropic_api_base.clone())
                .with_cache_markers(cfg.cache_markers_enabled),
        )),
        "gemini" => Ok(Box::new(
            GeminiProvider::new(http.clone(), cfg.gemini_api_base.clone())
                .with_explicit_cache(cfg.cache_markers_enabled),
        )),
        "vertex" => {
            static TOKENS: std::sync::OnceLock<std::sync::Arc<vertex_auth::TokenSource>> =
                std::sync::OnceLock::new();
            let tokens: std::sync::Arc<vertex_auth::TokenSource> = match TOKENS.get() {
                Some(t) => t.clone(),
                None => {
                    let t = std::sync::Arc::new(
                        vertex_auth::TokenSource::discover(http.clone())
                            .map_err(|e| GatewayError::Internal(e.to_string()))?,
                    );
                    TOKENS.get_or_init(|| t).clone()
                }
            };
            Ok(Box::new(vertex::VertexProvider::new(
                http.clone(),
                cfg.vertex_api_base(),
                cfg.vertex_openapi_base(),
                tokens,
            )))
        }
        "openrouter" => Ok(Box::new(
            OpenRouterProvider::new(
                http.clone(),
                cfg.openrouter_api_base.clone(),
                cfg.openrouter_http_referer.clone(),
                cfg.openrouter_x_title.clone(),
            )
            .with_cache_markers(cfg.cache_markers_enabled),
        )),
        other => match custom_endpoint {
            Some(endpoint) => {
                // A member's endpoint goes out only through the public-only client.
                let guarded;
                let http = if endpoint.public_only {
                    guarded = crate::public_only_client();
                    &guarded
                } else {
                    http
                };
                custom_provider_client(endpoint, http, cfg)
            }
            None => Err(GatewayError::Internal(format!(
                "provider '{other}' has no endpoint (unregistered custom provider?)"
            ))),
        },
    }
}

/// The client for a custom provider's endpoint, over `http`.
fn custom_provider_client(
    endpoint: &CustomEndpoint,
    http: &reqwest::Client,
    cfg: &GatewayConfig,
) -> Result<Box<dyn ProviderClient>, GatewayError> {
    match &endpoint.dialect {
        // Bedrock Converse has its own wire format; use the dedicated provider.
        ProviderDialect::BedrockConverse => Ok(Box::new(
            BedrockConverseProvider::new(http.clone(), endpoint.base_url.clone())
                .with_cache_markers(cfg.cache_markers_enabled),
        )),
        // Other custom providers carry OpenAI-shaped bodies; the dialect
        // supplies the envelope (URL layout + credential header).
        _ => Ok(Box::new(OpenAiProvider::with_dialect(
            http.clone(),
            endpoint.base_url.clone(),
            endpoint.dialect.clone(),
        ))),
    }
}

/// Run **one** embeddings call against the resolved destination and return the response.
///
/// Deliberately not in [`fallback`] and deliberately not a loop. Falling back to a
/// different embedding model is not the same kind of move as falling back on chat: chat
/// models differ in quality, embedding models differ in dimensionality and vector space.
/// An agent that indexed at 3072 dimensions and then silently searched at 1536 gets no
/// error — it gets a corpus split across two incomparable spaces. So the resolver picks
/// one model by presence, this makes one attempt, and a failure is surfaced as-is: no
/// fallback list, no provider switch, no `try_drop_param` retry.
///
/// It lives here rather than next to the chat fallback logic so nobody reintroduces
/// chaining later just because the code sat beside a loop that does it.
pub async fn execute_embeddings_once(
    http: &reqwest::Client,
    cfg: &GatewayConfig,
    resolved: &ResolvedEmbeddingConfig,
    req: &EmbeddingsRequest,
    attempts: &mut attempts::AttemptLog,
) -> Result<EmbeddingsResponse, GatewayError> {
    tracing::info!(
        target: "nasiko::llm_router::provider",
        op = "embeddings",
        provider = %resolved.provider,
        model = %resolved.model,
        "provider request → dispatching embeddings to upstream LLM (single attempt, no fallback)"
    );
    let provider = provider_for(
        &resolved.provider,
        resolved.custom_endpoint.as_ref(),
        http,
        cfg,
    )?;
    attempts.start(&resolved.provider, &resolved.model, resolved.platform_paid);
    let result = provider.embeddings(req, resolved).await;
    match &result {
        Ok(_) => attempts.settle(),
        Err(e) => attempts.fail(e),
    }
    match result {
        Ok(resp) => {
            tracing::debug!(
                target: "nasiko::llm_router::provider",
                op = "embeddings",
                provider = %resolved.provider,
                model = %resolved.model,
                response_body = %serde_json::to_string(&resp)
                    .unwrap_or_else(|e| format!("<serialize error: {e}>")),
                "provider response ← embeddings body"
            );
            Ok(resp)
        }
        Err(e) => {
            tracing::warn!(
                target: "nasiko::llm_router::provider",
                provider = %resolved.provider,
                model = %resolved.model,
                error = %e,
                "embedding attempt failed; surfacing it (no fallback by design — a different \
                 embedding model would return vectors in a different space)"
            );
            Err(e.into())
        }
    }
}

pub mod anthropic;
pub mod attempts;
pub mod bedrock_converse;
pub mod dialect;
pub mod fallback;
pub mod gemini;
mod gemini_cache;
pub(crate) mod media;
pub mod openai;
pub mod openrouter;
pub(crate) mod sse;
pub mod vertex;
pub mod vertex_auth;

pub use anthropic::AnthropicProvider;
pub use bedrock_converse::BedrockConverseProvider;
pub use dialect::{KIND_AZURE_OPENAI, KIND_BEDROCK_CONVERSE, KIND_OPENAI, ProviderDialect};
pub use gemini::GeminiProvider;
pub use openai::OpenAiProvider;
pub use openrouter::OpenRouterProvider;

/// Current unix time (seconds) for synthesized response `created` fields. Providers
/// that don't return a creation timestamp (Anthropic, Gemini) stamp one here.
pub(crate) fn now_unix() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

// ── Streaming chunk builders (shared by the translating spokes) ───────────────

/// A `chat.completion.chunk` carrying one incremental `delta`.
pub(crate) fn delta_chunk(id: &str, model: &str, delta: Delta) -> ChatChunk {
    ChatChunk {
        id: format!("chatcmpl-{id}"),
        object: "chat.completion.chunk".to_string(),
        created: Some(now_unix()),
        model: model.to_string(),
        choices: vec![ChunkChoice {
            index: 0,
            delta,
            finish_reason: None,
        }],
        usage: None,
        extra: Map::new(),
    }
}

/// A terminal chunk carrying only `finish_reason`.
pub(crate) fn finish_chunk(id: &str, model: &str, finish_reason: String) -> ChatChunk {
    ChatChunk {
        id: format!("chatcmpl-{id}"),
        object: "chat.completion.chunk".to_string(),
        created: Some(now_unix()),
        model: model.to_string(),
        choices: vec![ChunkChoice {
            index: 0,
            delta: Delta::default(),
            finish_reason: Some(finish_reason),
        }],
        usage: None,
        extra: Map::new(),
    }
}

/// OpenAI-style trailing usage chunk: empty `choices`, populated `usage`.
pub(crate) fn usage_chunk(id: &str, model: &str, usage: Usage) -> ChatChunk {
    ChatChunk {
        id: format!("chatcmpl-{id}"),
        object: "chat.completion.chunk".to_string(),
        created: Some(now_unix()),
        model: model.to_string(),
        choices: vec![],
        usage: Some(usage),
        extra: Map::new(),
    }
}

/// A failed provider call. `retryable` drives the fallback executor (step 8): retry on
/// transport/5xx faults, never on 4xx request-shape errors.
#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    #[error("provider returned {status}: {message}")]
    Status {
        status: u16,
        message: String,
        retryable: bool,
    },
    #[error("transport error: {0}")]
    Transport(String),
    #[error("response parse error: {0}")]
    Parse(String),
    /// Refused at this edge, never sent: the request carries something the provider
    /// cannot accept (an audio part on the Anthropic spoke). The caller's to fix, so it
    /// surfaces as a 400 — a 502 would read as a provider fault and invite SDK retries.
    #[error("{0}")]
    Rejected(String),
    /// The model did not answer within its share of the chain's deadline, so the chain moved
    /// on (or, as the last attempt, gave up). Ends as a 504.
    #[error("{0}")]
    TimedOut(String),
}

impl ProviderError {
    /// Whether this failure is worth retrying against a fallback model.
    pub fn retryable(&self) -> bool {
        match self {
            ProviderError::Status { retryable, .. } => *retryable,
            ProviderError::Transport(_) | ProviderError::TimedOut(_) => true,
            ProviderError::Parse(_) | ProviderError::Rejected(_) => false,
        }
    }
}

/// Whether a provider 400 is Gemini refusing the API key. Gemini answers a bad key with 400
/// `API_KEY_INVALID` where other providers send 401, so it is a key fault, not a bad request:
/// it stays a 502 (the owner's stored key is wrong, not the caller's token) and a fallback
/// may still answer.
pub(crate) fn is_bad_key_400(status: u16, message: &str) -> bool {
    status == 400 && (message.contains("API_KEY_INVALID") || message.contains("API key not valid"))
}

/// Who paid with the key a provider refused, which decides who can fix it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum KeyOwner {
    /// The owner's own secret, named by their LLM config.
    User,
    /// The platform key for a built-in provider (`PLATFORM_<PROVIDER>_API_KEY`).
    Platform,
    /// The key stored with a custom provider in LLM router.
    CustomProvider,
}

impl KeyOwner {
    pub(crate) fn of(platform_paid: bool, custom_provider: bool) -> Self {
        match (platform_paid, custom_provider) {
            (false, _) => KeyOwner::User,
            (true, true) => KeyOwner::CustomProvider,
            (true, false) => KeyOwner::Platform,
        }
    }
}

/// A provider refusing the key it was sent (401, 403, or Gemini's 400 `API_KEY_INVALID`),
/// reworded to say whose key it was and where to fix it, or `None` for any other failure.
/// Stays a 502: the caller's own token is fine, and a 401 would send SDKs into a
/// credential-refresh loop. The provider's raw body is in the attempt log, not here.
pub(crate) fn rejected_key_error(
    err: &ProviderError,
    provider: &str,
    owner: KeyOwner,
) -> Option<GatewayError> {
    let ProviderError::Status {
        status, message, ..
    } = err
    else {
        return None;
    };
    if !matches!(status, 401 | 403) && !is_bad_key_400(*status, message) {
        return None;
    }
    let (whose, fix) = match owner {
        KeyOwner::User => (
            "the API key from your LLM config",
            "Update the key in LLM router.".to_string(),
        ),
        KeyOwner::Platform => (
            "this server's API key",
            match crate::error::platform_key_env(provider) {
                Some(env) => format!("Ask your admin to check {env}."),
                None => "Ask your admin to check it.".to_string(),
            },
        ),
        KeyOwner::CustomProvider => (
            "the API key stored for this provider",
            "Ask your admin to update it in LLM router.".to_string(),
        ),
    };
    Some(GatewayError::Upstream(format!(
        "provider '{provider}' rejected {whose} (HTTP {status}). {fix}"
    )))
}

pub(crate) fn param_rejection(detail: &str) -> Option<String> {
    let lower = detail.to_ascii_lowercase();
    if lower.contains("doesn't support the temperature field") {
        return Some("temperature".to_string());
    }
    if ![
        "deprecated",
        "unsupported",
        "not supported",
        "does not support",
    ]
    .iter()
    .any(|p| lower.contains(p))
    {
        return None;
    }
    let start = detail.find('`')? + 1;
    let end = start + detail[start..].find('`')?;
    let param = detail[start..end].trim();
    (!param.is_empty()).then(|| param.to_string())
}

fn passes_through(status: u16) -> bool {
    matches!(status, 400 | 404 | 409 | 413 | 415 | 422 | 429)
}

impl From<ProviderError> for GatewayError {
    fn from(e: ProviderError) -> Self {
        match e {
            ProviderError::Rejected(message) => GatewayError::BadRequest(message),
            ProviderError::Status {
                status,
                ref message,
                ..
            } if passes_through(status) && !is_bad_key_400(status, message) => {
                GatewayError::Provider {
                    status: axum::http::StatusCode::from_u16(status)
                        .unwrap_or(axum::http::StatusCode::BAD_GATEWAY),
                    message: format!("Upstream LLM error: {e}"),
                }
            }
            ProviderError::TimedOut(message) => GatewayError::UpstreamTimeout(message),
            other => GatewayError::Upstream(other.to_string()),
        }
    }
}

/// A single-provider client. Implementations translate the IR to the provider's wire
/// format, call it, and translate the result back to IR.
#[async_trait]
pub trait ProviderClient: Send + Sync {
    async fn chat(
        &self,
        req: &ChatRequest,
        cfg: &ResolvedConfig,
    ) -> Result<ChatResponse, ProviderError>;

    async fn chat_stream(
        &self,
        req: &ChatRequest,
        cfg: &ResolvedConfig,
    ) -> Result<BoxStream<'static, Result<ChatChunk, ProviderError>>, ProviderError>;

    /// One embeddings call. Takes [`ResolvedEmbeddingConfig`] rather than
    /// [`ResolvedConfig`]: the embedding path never has a chat model, a fallback list or
    /// a tier, and giving it a type that cannot carry them keeps it that way.
    async fn embeddings(
        &self,
        req: &EmbeddingsRequest,
        cfg: &ResolvedEmbeddingConfig,
    ) -> Result<EmbeddingsResponse, ProviderError>;

    /// If `err` is a "this model doesn't accept parameter X" rejection, return the IR
    /// parameter to drop so the executor can retry the *same* model without it. Returns
    /// `None` for any other error (transport, auth, quota, genuine bad request).
    ///
    /// This is the general seam for model/parameter capability mismatches: rather than
    /// maintaining a per-model table of unsupported params, we let the provider report
    /// the offending field from its own error body. Default: never droppable — providers
    /// that can recognize their param-rejection shape override this.
    fn droppable_param(&self, _err: &ProviderError) -> Option<String> {
        None
    }

    /// If `err` is a "this model needs parameter X set to value V" rejection, return the
    /// IR parameter and the value so the executor can retry the *same* model with it set.
    ///
    /// The mirror of [`droppable_param`](ProviderClient::droppable_param), for the
    /// rejections a drop cannot fix: the offending value is the model's own *default*,
    /// not something we sent, so there is nothing to strip — the request has to name the
    /// param explicitly. Same seam rule: the provider recognizes its own error shape
    /// rather than us keeping a per-model capability table. Default: never repairable.
    fn repairable_param(&self, _err: &ProviderError) -> Option<(String, Value)> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retryability_rules() {
        assert!(ProviderError::Transport("timeout".into()).retryable());
        assert!(!ProviderError::Parse("bad json".into()).retryable());
        assert!(
            ProviderError::Status {
                status: 503,
                message: "x".into(),
                retryable: true
            }
            .retryable()
        );
        assert!(
            !ProviderError::Status {
                status: 400,
                message: "x".into(),
                retryable: false
            }
            .retryable()
        );
    }

    #[test]
    fn converts_to_upstream_gateway_error() {
        let g: GatewayError = ProviderError::Transport("boom".into()).into();
        assert!(matches!(g, GatewayError::Upstream(_)));
        assert_eq!(g.status(), axum::http::StatusCode::BAD_GATEWAY);
    }

    fn status_error(status: u16, message: &str) -> GatewayError {
        ProviderError::Status {
            status,
            message: message.into(),
            retryable: status == 429 || status >= 500,
        }
        .into()
    }

    #[test]
    fn a_provider_status_the_caller_can_act_on_reaches_the_caller_unchanged() {
        for status in [400u16, 404, 409, 413, 415, 422, 429] {
            let g = status_error(status, r#"{"error":{"message":"bad image"}}"#);
            assert_eq!(g.status().as_u16(), status, "provider status {status}");
            assert_eq!(
                g.to_string(),
                format!(
                    r#"Upstream LLM error: provider returned {status}: {{"error":{{"message":"bad image"}}}}"#
                )
            );
        }
    }

    #[test]
    fn a_provider_credential_or_server_failure_stays_a_502() {
        for status in [401u16, 403, 405, 500, 502, 503, 529] {
            assert_eq!(
                status_error(status, "x").status(),
                axum::http::StatusCode::BAD_GATEWAY,
                "provider status {status}"
            );
        }
    }

    #[test]
    fn a_gemini_bad_key_400_stays_a_502_with_the_provider_message() {
        let gemini_bad_key = r#"{"error":{"code":400,"message":"API key not valid. Please pass a valid API key.","status":"INVALID_ARGUMENT","details":[{"reason":"API_KEY_INVALID"}]}}"#;
        let g = status_error(400, gemini_bad_key);
        assert!(matches!(g, GatewayError::Upstream(_)), "{g}");
        assert_eq!(g.status(), axum::http::StatusCode::BAD_GATEWAY);
        assert!(g.to_string().contains("API key not valid"), "{g}");
    }

    #[test]
    fn a_rejected_key_names_whose_key_and_only_for_key_failures() {
        let refused = |status: u16, message: &str| ProviderError::Status {
            status,
            message: message.into(),
            retryable: false,
        };
        let custom = rejected_key_error(
            &refused(403, "forbidden"),
            "my-gw",
            KeyOwner::CustomProvider,
        )
        .unwrap();
        assert_eq!(custom.status(), axum::http::StatusCode::BAD_GATEWAY);
        assert_eq!(
            custom.to_string(),
            "Upstream LLM error: provider 'my-gw' rejected the API key stored for this provider \
             (HTTP 403). Ask your admin to update it in LLM router."
        );
        let gemini = rejected_key_error(
            &refused(400, "API key not valid. Please pass a valid API key."),
            "gemini",
            KeyOwner::Platform,
        )
        .unwrap();
        assert!(
            gemini.to_string().contains("PLATFORM_GEMINI_API_KEY"),
            "{gemini}"
        );
        for not_a_key_failure in [refused(400, "bad image"), refused(500, "boom")] {
            assert!(
                rejected_key_error(&not_a_key_failure, "openai", KeyOwner::User).is_none(),
                "{not_a_key_failure}"
            );
        }
        assert_eq!(KeyOwner::of(false, true), KeyOwner::User);
        assert_eq!(KeyOwner::of(true, true), KeyOwner::CustomProvider);
        assert_eq!(KeyOwner::of(true, false), KeyOwner::Platform);
    }

    #[test]
    fn a_timed_out_attempt_is_a_504() {
        let g: GatewayError =
            ProviderError::TimedOut("model m did not answer within 2.0s".into()).into();
        assert_eq!(g.status(), axum::http::StatusCode::GATEWAY_TIMEOUT);
        assert!(g.to_string().contains("did not answer within"));
    }

    #[test]
    fn rejected_converts_to_bad_request_not_upstream() {
        let e = ProviderError::Rejected("Anthropic does not accept audio input".into());
        assert!(!e.retryable());
        let g: GatewayError = e.into();
        assert!(matches!(g, GatewayError::BadRequest(_)));
        assert_eq!(g.status(), axum::http::StatusCode::BAD_REQUEST);
        assert_eq!(g.to_string(), "Anthropic does not accept audio input");
    }

    fn resolved(provider: &str, base_url: Option<&str>) -> ResolvedConfig {
        resolved_with(provider, base_url, ProviderDialect::OpenAi)
    }

    fn resolved_with(
        provider: &str,
        base_url: Option<&str>,
        dialect: ProviderDialect,
    ) -> ResolvedConfig {
        ResolvedConfig {
            agent_id: String::new(),
            compress_enabled: false,
            provider: provider.into(),
            model: "m".into(),
            litellm_model: format!("{provider}/m"),
            api_key: "k".into(),
            fallback_models: Vec::new(),
            temperature: None,
            max_tokens: None,
            has_llm_config: false,
            pinned_model: None,
            tier1_model: None,
            tier2_model: None,
            tier3_model: None,
            platform_paid: true,
            custom_endpoint: base_url.map(|base_url| crate::resolver::CustomEndpoint {
                base_url: base_url.to_string(),
                dialect,
                public_only: false,
            }),
            is_coding_agent: false,
            is_discovered_agent: false,
        }
    }

    #[test]
    fn provider_for_custom_builds_against_overridden_base_url() {
        let http = reqwest::Client::new();
        let cfg = GatewayConfig::default();
        // A custom (non-built-in) provider with an endpoint builds an OpenAI-shaped
        // client against that URL — in either dialect.
        let gateway = resolved("my-gateway", Some("https://gw/v1"));
        assert!(
            provider_for(
                &gateway.provider,
                gateway.custom_endpoint.as_ref(),
                &http,
                &cfg
            )
            .is_ok()
        );
        let azure = resolved_with(
            "azure-prod",
            Some("https://acme.openai.azure.com"),
            ProviderDialect::AzureOpenAi {
                api_version: "2024-10-21".into(),
            },
        );
        assert!(provider_for(&azure.provider, azure.custom_endpoint.as_ref(), &http, &cfg).is_ok());
        // ...but with no endpoint it is a server-side gap (the resolver should have
        // populated it), surfaced as an Internal error rather than a mis-target.
        assert!(matches!(
            provider_for("my-gateway", None, &http, &cfg),
            Err(GatewayError::Internal(_))
        ));
    }
}

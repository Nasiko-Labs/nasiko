//! Provider clients — the spokes of the hub.
//!
//! A [`ProviderClient`] takes the canonical IR + a [`ResolvedConfig`] and calls one
//! provider, returning IR. Per-provider impls (OpenAI / Anthropic / Gemini) land in
//! steps 4–6 and own the OpenAI⇄provider translation; the ordered fallback executor
//! lands in step 8. Errors are [`ProviderError`] (carrying retryability) and convert
//! to `GatewayError::Upstream` once fallbacks are exhausted.

use async_trait::async_trait;
use futures::stream::BoxStream;

use serde_json::Map;

use crate::config::GatewayConfig;
use crate::error::GatewayError;
use crate::ir::{
    ChatChunk, ChatRequest, ChatResponse, ChunkChoice, Delta, EmbeddingsRequest,
    EmbeddingsResponse, Usage,
};
use crate::resolver::ResolvedConfig;

/// Construct the provider client for a provider id. Used by the handler and the
/// fallback executor. Unknown providers are a server-side gap (500).
pub fn provider_for(
    provider: &str,
    http: &reqwest::Client,
    cfg: &GatewayConfig,
) -> Result<Box<dyn ProviderClient>, GatewayError> {
    match provider {
        "openai" => Ok(Box::new(OpenAiProvider::new(
            http.clone(),
            cfg.openai_api_base.clone(),
        ))),
        "anthropic" => Ok(Box::new(AnthropicProvider::new(
            http.clone(),
            cfg.anthropic_api_base.clone(),
        ))),
        "gemini" => Ok(Box::new(GeminiProvider::new(
            http.clone(),
            cfg.gemini_api_base.clone(),
        ))),
        other => Err(GatewayError::Internal(format!(
            "provider '{other}' is not supported yet"
        ))),
    }
}

pub mod anthropic;
pub mod fallback;
pub mod gemini;
pub mod openai;
pub(crate) mod sse;

pub use anthropic::AnthropicProvider;
pub use gemini::GeminiProvider;
pub use openai::OpenAiProvider;

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
}

impl ProviderError {
    /// Whether this failure is worth retrying against a fallback model.
    pub fn retryable(&self) -> bool {
        match self {
            ProviderError::Status { retryable, .. } => *retryable,
            ProviderError::Transport(_) => true,
            ProviderError::Parse(_) => false,
        }
    }
}

impl From<ProviderError> for GatewayError {
    fn from(e: ProviderError) -> Self {
        GatewayError::Upstream(e.to_string())
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

    async fn embeddings(
        &self,
        req: &EmbeddingsRequest,
        cfg: &ResolvedConfig,
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
}

//! The seam through which the orchestrator's own LLM calls reach the LLM router.
//!
//! The router lives in the same process (the server), so calls go through this trait
//! instead of HTTP: no token, no traceparent. The orchestrator does not depend on the
//! router crate; the server implements [`LlmBackend`] on `nasiko_llm_router::platform`.
//! The router picks the provider (OpenAI, Vertex, Anthropic, …) and writes `trace_usage`,
//! so callers going through a backend must not record usage themselves.

use std::sync::Arc;

use async_trait::async_trait;
use uuid::Uuid;

/// Who a call is billed to and how its usage row is labelled.
#[derive(Debug, Clone)]
pub struct LlmCallContext {
    /// Run owner; `trace_usage.user_id`.
    pub user_id: Uuid,
    /// `trace_usage.agent_name`, e.g. `maf`, `maf:step2`, `orchestrator:selector`.
    pub component: String,
    /// `trace_usage.session_id`; `None` writes a `direct:` row.
    pub session_id: Option<String>,
}

/// An OpenAI-shaped chat/embeddings endpoint served in-process.
#[async_trait]
pub trait LlmBackend: Send + Sync {
    /// `body` is an OpenAI `/v1/chat/completions` request (non-streaming); the reply is
    /// the OpenAI response JSON. A missing/empty `model` uses the platform default.
    async fn chat(
        &self,
        call: &LlmCallContext,
        body: serde_json::Value,
    ) -> Result<serde_json::Value, String>;

    /// `texts` embedded with `model`; vectors in input order.
    async fn embed(
        &self,
        call: &LlmCallContext,
        model: &str,
        texts: &[String],
    ) -> Result<Vec<Vec<f32>>, String>;

    /// Speech to text. `mime_type` is the upload content type (e.g. `audio/webm`).
    /// The default rejects, so backends without audio support need no change.
    async fn transcribe(
        &self,
        _call: &LlmCallContext,
        _audio: Vec<u8>,
        _mime_type: &str,
    ) -> Result<String, String> {
        Err("transcription not supported by this backend".into())
    }

    /// Provider label platform calls go to (for logs and spans).
    fn provider(&self) -> String;
}

pub type SharedLlmBackend = Arc<dyn LlmBackend>;

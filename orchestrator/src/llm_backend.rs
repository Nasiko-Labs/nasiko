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

/// The platform's own model-call components: the `trace_usage.agent_name` of usage that
/// belongs to no registered agent. FinOps scoped queries match [`PlatformComponent::ALL`], so a
/// new component only needs a variant here. MAF per-step rows are `maf:step<i>`
/// ([`PlatformComponent::MAF_STEP_PREFIX`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PlatformComponent {
    Orchestrator,
    RouterSelection,
    Maf,
    MafPlanner,
    Titling,
    Transcription,
    CapabilityGeneration,
    HistoryEmbedding,
    AgentEmbedding,
    FinopsInsights,
}

impl PlatformComponent {
    pub const ALL: [PlatformComponent; 10] = [
        Self::Orchestrator,
        Self::RouterSelection,
        Self::Maf,
        Self::MafPlanner,
        Self::Titling,
        Self::Transcription,
        Self::CapabilityGeneration,
        Self::HistoryEmbedding,
        Self::AgentEmbedding,
        Self::FinopsInsights,
    ];

    /// Prefix of MAF per-step rows, `maf:step<i>`.
    pub const MAF_STEP_PREFIX: &'static str = "maf:step";

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Orchestrator => "orchestrator",
            Self::RouterSelection => "router_selection",
            Self::Maf => "maf",
            Self::MafPlanner => "maf_planner",
            Self::Titling => "titling",
            Self::Transcription => "transcription",
            Self::CapabilityGeneration => "capability_generation",
            Self::HistoryEmbedding => "history_embedding",
            Self::AgentEmbedding => "agent_embedding",
            Self::FinopsInsights => "finops_insights",
        }
    }

    /// Every component label, for SQL `= ANY($1)` filters.
    pub fn all_names() -> Vec<&'static str> {
        Self::ALL.iter().map(|c| c.as_str()).collect()
    }

    /// Whether `agent_name` is a platform row: a component label or a MAF step row.
    pub fn is_platform_row(agent_name: &str) -> bool {
        Self::ALL.iter().any(|c| c.as_str() == agent_name)
            || agent_name.starts_with(Self::MAF_STEP_PREFIX)
    }
}

impl std::fmt::Display for PlatformComponent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod component_tests {
    use super::PlatformComponent;

    #[test]
    fn all_lists_every_variant_once() {
        let names: Vec<&str> = PlatformComponent::all_names();
        let unique: std::collections::HashSet<&str> = names.iter().copied().collect();
        assert_eq!(unique.len(), names.len());
        assert!(PlatformComponent::is_platform_row("titling"));
        assert!(PlatformComponent::is_platform_row("maf:step3"));
        assert!(!PlatformComponent::is_platform_row("my-agent"));
    }
}

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

/// A failed backend chat call. `status` is the upstream provider's HTTP status when it
/// rejected the request (e.g. 400 for an unsupported `response_format`), so callers can
/// retry with a different request; `None` for transport/config failures.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LlmError {
    pub status: Option<u16>,
    pub message: String,
}

impl std::fmt::Display for LlmError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl From<LlmError> for String {
    fn from(e: LlmError) -> Self {
        e.message
    }
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
    ) -> Result<serde_json::Value, LlmError>;

    /// `texts` embedded with `model`; vectors in input order.
    async fn embed(
        &self,
        call: &LlmCallContext,
        model: &str,
        texts: &[String],
    ) -> Result<Vec<Vec<f32>>, String>;

    /// `provider/model` an [`LlmBackend::embed`] call for `model` actually runs on. Embedding
    /// caches key by it, so vectors from different providers or models (different
    /// dimensions) never mix. Default: `model` unchanged.
    fn embedding_identity(&self, model: &str) -> String {
        model.to_string()
    }

    /// Whether an [`LlmBackend::embed`] call for `model` can succeed (some provider
    /// serves embeddings). Default: `true`.
    fn can_embed(&self, _model: &str) -> bool {
        true
    }

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

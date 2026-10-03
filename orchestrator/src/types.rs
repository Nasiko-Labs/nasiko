use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentCard {
    pub id: Uuid,
    pub name: String,
    pub description: String,
    pub skills: Vec<String>,
    pub tags: Vec<String>,
    pub url: Option<String>,
    /// Persisted embedding of `name + description + tags`, loaded from
    /// `agents.embedding`. `None` if never computed.
    #[serde(default)]
    pub embedding: Option<Vec<f32>>,
    /// Hash of the text that produced `embedding` (see `vector_store::hash_prompt`),
    /// loaded from `agents.embedding_content_hash`. Compared against a freshly
    /// computed hash to detect a stale embedding.
    #[serde(default)]
    pub embedding_content_hash: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct FilePart {
    pub filename: String,
    pub content_type: String,
    pub data: Vec<u8>,
}

impl FilePart {
    /// Encode a raw file into the `FilePart` format stored in orchestrator types.
    /// The `data` field becomes a base64 data URI: `data:<mime>;base64,<bytes>`.
    pub fn encode(filename: String, bytes: &[u8], mime_type: String) -> Self {
        let encoded = B64.encode(bytes);
        let data_uri = format!("data:{};base64,{}", mime_type, encoded);
        FilePart {
            filename,
            content_type: mime_type,
            data: data_uri.into_bytes(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct RouteRequest {
    pub query: String,
    pub session_id: String,
    pub user_id: Uuid,
    pub file_parts: Vec<FilePart>,
}

#[derive(Debug, Clone)]
pub struct RouteResult {
    pub agent: AgentCard,
    pub fallback_used: bool,
}

/// Mirrors `router_request_log` columns (`oss/migrations/0001_schema.sql`).
/// Written fire-and-forget after every successful route().
#[derive(Debug, Clone)]
pub struct RouterLogEntry {
    pub request_id: String,
    pub user_id: Uuid,
    pub session_id: String,
    pub query: String,
    pub agents_considered: i32,
    pub selected_agent_id: Option<Uuid>,
    pub selected_agent_name: Option<String>,
    pub selection_reasoning: Option<String>,
    pub fallback_used: bool,
    pub total_latency_ms: i32,
    pub registry_fetch_ms: Option<i32>,
    pub selection_llm_ms: Option<i32>,
    pub stage1_candidates: Option<i32>,
    pub stage2_candidates: Option<i32>,
    pub embedding_model: Option<String>,
    pub file_count: i32,
    /// UUID of token_usage record tracking the Stage 3 LLM selector call.
    pub selection_token_usage_id: Option<Uuid>,
    /// `false` for a routing decision that ended in a refusal (the caller's
    /// `RoutingPolicy` rejected every candidate) rather than an agent selection.
    pub success: bool,
    /// Set when `success` is `false`, to say why routing did not produce a pick.
    pub error_message: Option<String>,
}

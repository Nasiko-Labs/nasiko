use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;

#[derive(Serialize)]
pub struct CursorPage<T: Serialize> {
    pub data: Vec<T>,
    pub has_more: bool,
    pub next_cursor: Option<String>,
    pub prev_cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct ChatSession {
    pub session_id: String,
    pub user_id: Uuid,
    pub agent_id: Option<Uuid>,
    pub agent_url: Option<String>,
    pub title: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct ChatSessionView {
    pub session_id: String,
    pub user_id: Uuid,
    pub agent_id: Option<Uuid>,
    pub agent_url: Option<String>,
    pub title: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub agent_name: Option<String>,
    pub last_message: Option<String>,
    // Per-session rollups computed in the list query itself, so the sessions
    // page renders its stats columns without a trace-store round-trip.
    // `total_tokens` covers **platform-paid** spend only (migration 041), so a
    // NULL means "nothing billed here", not "no data" — BYO-key agent spend is
    // visible on the session detail page, which reads Tempo.
    pub message_count: Option<i64>,
    pub trace_count: Option<i64>,
    pub total_tokens: Option<i64>,
    pub latency_p50_ms: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct ChatMessage {
    pub id: Uuid,
    pub session_id: String,
    pub role: String,
    pub content: String,
    pub file_parts: Option<sqlx::types::Json<serde_json::Value>>,
    pub has_file_parts: bool,
    pub timestamp: DateTime<Utc>,
    // Per-message usage (assistant rows; platform-paid spend only — see
    // migration 041). All optional: user rows and BYO-key agent replies
    // carry at most duration/trace.
    pub input_tokens: Option<i32>,
    pub output_tokens: Option<i32>,
    pub model: Option<String>,
    pub duration_ms: Option<i32>,
    pub cost_usd: Option<rust_decimal::Decimal>,
    pub usage_estimated: Option<bool>,
    pub trace_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CreateSession {
    pub agent_id: Option<String>,
    // NOTE: `agent_url` is intentionally NOT a client input (stored-SSRF risk —
    // see `create_session`). The canonical URL is always resolved server-side
    // from the `agents` table once `agent_id` is validated. A client that sends
    // `agent_url` in the body has that field ignored, not stored.
    /// The user's first message. The server derives the session title from this
    /// via a single LLM call (see `create_session`); it is not stored verbatim.
    pub first_prompt: Option<String>,
    /// Client-chosen session ID (e.g. `nasiko chat --session-id my-run`).
    /// When omitted the server generates one. If a session with this ID
    /// already exists and belongs to the caller, it is returned as-is.
    pub session_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateSession {
    pub title: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct ChatMessageFile {
    pub id: Uuid,
    pub message_id: Option<Uuid>,
    pub session_id: String,
    pub filename: String,
    pub mime_type: String,
    pub size_bytes: i64,
    pub storage_uri: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
pub struct SendMessage {
    pub role: String,
    pub content: String,
    pub file_parts: Option<serde_json::Value>,
    pub file_ids: Option<Vec<Uuid>>,
    /// Optional per-message usage captured client-side from the stream's
    /// `usage_meta` event (direct-agent chats persist through this route).
    pub usage: Option<MessageUsage>,
}

/// The usage subset a client may attach when persisting an assistant message.
#[derive(Debug, Deserialize)]
pub struct MessageUsage {
    pub input_tokens: Option<i32>,
    pub output_tokens: Option<i32>,
    pub model: Option<String>,
    pub duration_ms: Option<i32>,
    pub cost_usd: Option<rust_decimal::Decimal>,
    pub estimated: Option<bool>,
    pub trace_id: Option<String>,
}

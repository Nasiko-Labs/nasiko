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
    pub id: Uuid,
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
    pub id: Uuid,
    pub session_id: String,
    pub user_id: Uuid,
    pub agent_id: Option<Uuid>,
    pub agent_url: Option<String>,
    pub title: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub agent_name: Option<String>,
    pub last_message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct ChatMessage {
    pub id: Uuid,
    pub session_id: String,
    pub role: String,
    pub content: String,
    pub file_parts: Option<sqlx::types::Json<serde_json::Value>>,
    pub has_file_parts: bool,
    pub trace_id: Option<String>,
    pub timestamp: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
pub struct CreateSession {
    pub agent_id: Option<String>,
    // NOTE: `agent_url` is intentionally NOT a client input (stored-SSRF risk —
    // see `create_session`). The canonical URL is always resolved server-side
    // from the `agents` table once `agent_id` is validated.
    pub title: Option<String>,
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
    pub trace_id: Option<String>,
}

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;

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
    pub timestamp: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
pub struct CreateSession {
    pub agent_id: Option<Uuid>,
    pub agent_url: Option<String>,
    pub title: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateSession {
    pub title: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct SendMessage {
    pub role: String,
    pub content: String,
    pub file_parts: Option<serde_json::Value>,
}

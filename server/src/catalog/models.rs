use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct Agent {
    pub id: Uuid,
    pub name: String,
    pub display_name: Option<String>,
    pub description: Option<String>,
    pub owner_id: Uuid,
    pub url: Option<String>,
    pub icon_url: Option<String>,
    pub version: String,
    pub protocol_version: String,
    pub preferred_transport: String,
    pub documentation_url: Option<String>,
    pub capabilities: sqlx::types::Json<serde_json::Value>,
    pub security_schemes: sqlx::types::Json<serde_json::Value>,
    pub default_input_modes: sqlx::types::Json<Vec<String>>,
    pub default_output_modes: sqlx::types::Json<Vec<String>>,
    pub skills: sqlx::types::Json<Vec<Skill>>,
    pub tags: Vec<String>,
    pub metadata: sqlx::types::Json<serde_json::Value>,
    pub status: String,
    pub image: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Skill {
    pub id: String,
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub examples: Vec<serde_json::Value>,
}

/// Lightweight projection returned by the by-skill discovery endpoint.
#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct AgentSummary {
    pub id: Uuid,
    pub name: String,
    pub display_name: Option<String>,
    pub description: Option<String>,
    pub url: Option<String>,
    pub icon_url: Option<String>,
    pub version: String,
    pub status: String,
    pub tags: Vec<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Capabilities {
    #[serde(default)]
    pub streaming: bool,
    #[serde(default, rename = "pushNotifications")]
    pub push_notifications: bool,
    #[serde(default, rename = "stateTransitionHistory")]
    pub state_transition_history: bool,
    #[serde(default)]
    pub chat_agent: bool,
}

#[derive(Debug, Deserialize)]
pub struct CreateAgent {
    pub name: String,
    pub display_name: Option<String>,
    pub description: Option<String>,
    pub url: Option<String>,
    pub icon_url: Option<String>,
    pub version: Option<String>,
    pub documentation_url: Option<String>,
    pub capabilities: Option<serde_json::Value>,
    pub skills: Option<Vec<Skill>>,
    pub tags: Option<Vec<String>>,
    pub metadata: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateAgent {
    pub display_name: Option<String>,
    pub description: Option<String>,
    pub url: Option<String>,
    pub icon_url: Option<String>,
    pub version: Option<String>,
    pub documentation_url: Option<String>,
    pub capabilities: Option<serde_json::Value>,
    pub skills: Option<Vec<Skill>>,
    pub tags: Option<Vec<String>>,
    pub metadata: Option<serde_json::Value>,
    pub status: Option<String>,
    pub image: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct AgentVersion {
    pub id: Uuid,
    pub agent_id: Uuid,
    pub build_id: Option<Uuid>,
    pub version: String,
    pub image_tag: String,
    pub changelog: Option<String>,
    pub created_at: DateTime<Utc>,
}

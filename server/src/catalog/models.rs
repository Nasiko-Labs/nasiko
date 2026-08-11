use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use utoipa::ToSchema;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, FromRow)]
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
    #[schema(value_type = serde_json::Value)]
    pub capabilities: sqlx::types::Json<serde_json::Value>,
    #[schema(value_type = serde_json::Value)]
    pub security_schemes: sqlx::types::Json<serde_json::Value>,
    #[schema(value_type = Vec<String>)]
    pub default_input_modes: sqlx::types::Json<Vec<String>>,
    #[schema(value_type = Vec<String>)]
    pub default_output_modes: sqlx::types::Json<Vec<String>>,
    #[schema(value_type = Vec<Skill>)]
    pub skills: sqlx::types::Json<Vec<Skill>>,
    pub tags: Vec<String>,
    #[schema(value_type = serde_json::Value)]
    pub metadata: sqlx::types::Json<serde_json::Value>,
    pub status: String,
    pub image: Option<String>,
    /// Path of the agent's advertised JSON-RPC transport (e.g. "/jsonrpc"),
    /// extracted from its AgentCard `supportedInterfaces` at deploy time.
    /// Clients chat via `{base}/api/agents/{id}{transport_path}`.
    pub transport_path: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
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
#[derive(Debug, Serialize, ToSchema, sqlx::FromRow)]
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

#[derive(Debug, Deserialize, ToSchema)]
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
    /// `push`/`deploy` already send this; previously silently dropped since
    /// this struct had no field to catch it, leaving `agents.image` NULL
    /// until the first update/reupload set it.
    pub image: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
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
    /// If `true`, replace an already-used version instead of rejecting it.
    /// Only set when the user explicitly confirms or passes `--overwrite`.
    #[serde(default)]
    pub allow_overwrite: bool,
    /// `true` (the default) for a real deploy — the new version becomes
    /// active, archiving whatever was running before. `nasiko push` sets
    /// this `false`: it only makes an image available in the registry
    /// without deploying it, so it must not claim the new version is now
    /// active or archive the version that's genuinely still running.
    #[serde(default = "default_activate_version")]
    pub activate_version: bool,
}

fn default_activate_version() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, FromRow)]
pub struct AgentVersion {
    pub id: Uuid,
    pub agent_id: Uuid,
    pub build_id: Option<Uuid>,
    pub version: String,
    pub image_tag: String,
    pub changelog: Option<String>,
    pub is_active: bool,
    pub can_rollback: bool,
    pub previous_version: Option<String>,
    pub status: String,
    pub created_at: DateTime<Utc>,
}

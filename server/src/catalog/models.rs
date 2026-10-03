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
    /// Drives the control plane's own minimal-code ladder injection at A2A
    /// dispatch time (a2a_dispatch.rs), not a secret — unlike the old
    /// CODING_AGENT_MINIMAL_CODE env var this replaces, this column has a
    /// real read-back route, which is what lets the Settings-tab switch show
    /// its actual current state instead of a per-browser guess.
    pub minimal_code_enabled: bool,
    /// Per-agent opt-in for structural payload compression on this agent's LLM calls
    /// (`nasiko-compress`, applied in the LLM router). Off unless explicitly enabled.
    #[serde(default)]
    pub compress_enabled: bool,
    /// Per-agent opt-in for compact tool definitions on this agent's LLM calls
    /// (`nasiko-tool-compact`, applied in the LLM router behind `TOKEN_COMPACT_TOOLS`). Off
    /// unless explicitly enabled; deliberately separate from `compress_enabled`.
    #[serde(default)]
    pub compact_tools_enabled: bool,
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

/// Whether an agent's card reads as code work, and so should be offered minimal-code mode.
///
/// The single implementation behind all three consumers: the A2A dispatch path (which decides
/// whether to inject the ladder), the agent detail response (which decides whether the
/// settings page renders the toggle), and through that response, the settings page itself.
/// They previously each derived this for themselves — a Postgres `ILIKE '%code%'` and a
/// mirrored JavaScript `/code/i` — and both were wrong in the same two ways, because a
/// substring match on "code" misses `coding` entirely while matching `encode`.
///
/// `description` is deliberately not searched: it is prose, and matching it would classify a
/// documentation agent that merely mentions code as a coding agent.
pub fn has_coding_skills(skills: &[Skill]) -> bool {
    skills.iter().any(|skill| {
        nasiko_coding_policy::mentions_coding(&skill.id)
            || nasiko_coding_policy::mentions_coding(&skill.name)
            || skill
                .tags
                .iter()
                .any(|tag| nasiko_coding_policy::mentions_coding(tag))
    })
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
    /// Toggle payload compression for this agent. Omitted = leave as-is.
    pub compress_enabled: Option<bool>,
    /// Toggle compact tool definitions for this agent. Omitted = leave as-is.
    pub compact_tools_enabled: Option<bool>,
    /// `true` (the default) for a real deploy — the new version becomes
    /// active, archiving whatever was running before. `nasiko push` sets
    /// this `false`: it only makes an image available in the registry
    /// without deploying it, so it must not claim the new version is now
    /// active or archive the version that's genuinely still running.
    #[serde(default = "default_activate_version")]
    pub activate_version: bool,
    pub minimal_code_enabled: Option<bool>,
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

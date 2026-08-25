use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fmt;
use std::str::FromStr;
use uuid::Uuid;

/// Wire/DB string didn't match any known variant of the enum named in the error.
#[derive(Debug, thiserror::Error)]
#[error("invalid {enum_name} value: {value}")]
pub struct ParseEnumError {
    pub enum_name: &'static str,
    pub value: String,
}

macro_rules! db_enum {
    ($name:ident { $($variant:ident => $wire:literal),+ $(,)? }) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
        #[serde(rename_all = "snake_case")]
        pub enum $name {
            $($variant),+
        }

        impl $name {
            pub fn as_str(&self) -> &'static str {
                match self {
                    $(Self::$variant => $wire),+
                }
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl FromStr for $name {
            type Err = ParseEnumError;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                match s {
                    $($wire => Ok(Self::$variant),)+
                    other => Err(ParseEnumError {
                        enum_name: stringify!($name),
                        value: other.to_string(),
                    }),
                }
            }
        }
    };
}

db_enum!(HitlKind {
    InputRequired => "input_required",
    AuthRequired => "auth_required",
    ToolApproval => "tool_approval",
});

db_enum!(HitlOrigin {
    DirectChat => "direct_chat",
    AgentProxy => "agent_proxy",
    Orchestrator => "orchestrator",
    Maf => "maf",
    McpTool => "mcp_tool",
});

db_enum!(HitlStatus {
    Pending => "pending",
    Resolved => "resolved",
    Rejected => "rejected",
    Expired => "expired",
    Canceled => "canceled",
});

db_enum!(ResumeStatus {
    NotStarted => "not_started",
    Dispatching => "dispatching",
    Dispatched => "dispatched",
    Failed => "failed",
    DeliveryOutcomeUnknown => "delivery_outcome_unknown",
});

/// Mirrors the `hitl_requests` table (migration `0007_hitl.sql`).
///
/// `question` is write-once at creation; `human_response` is written only by `resolve()`;
/// `resume_state` is written only by the resume dispatcher and is never included in any API
/// response (see docs/HITL_IMPLEMENTATION_PLAN.md §4).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HitlRequest {
    pub id: Uuid,

    pub kind: HitlKind,
    pub origin: HitlOrigin,
    pub status: HitlStatus,
    pub resume_status: ResumeStatus,

    pub agent_id: Uuid,
    pub owner_user_id: Uuid,
    pub resolved_by: Option<Uuid>,

    pub task_id: Option<String>,
    pub context_id: Option<String>,
    pub chat_session_id: Option<String>,
    pub maf_execution_id: Option<Uuid>,
    pub maf_step_index: Option<i32>,
    pub arguments_hash: Option<String>,
    pub consumed_at: Option<DateTime<Utc>>,

    pub question: Value,
    pub human_response: Option<Value>,
    pub resume_state: Value,

    pub resume_dispatch_attempts: i32,
    pub resume_last_error: Option<String>,

    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    pub resolved_at: Option<DateTime<Utc>>,
}

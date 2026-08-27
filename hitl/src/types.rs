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

// 'completed' replaces the earlier 'dispatched'/'dispatching' pair for this MVP slice: nothing
// yet pushes a delivery through this column (tool_approval resumes via consumed_at, and the
// input_required/auth_required push-dispatcher isn't wired up yet), so there's no push-in-flight
// state to distinguish from push-confirmed. The claim lease lives in `resume_claimed_at` on
// `HitlRequest` instead of being folded into this enum, mirroring `build_jobs.picked_at`.
db_enum!(ResumeStatus {
    NotStarted => "not_started",
    Completed => "completed",
    Failed => "failed",
    DeliveryOutcomeUnknown => "delivery_outcome_unknown",
});

/// Mirrors the `hitl_requests` table (migration `0007_hitl.sql`).
///
/// `question` is write-once at creation; `human_response` is written only by `resolve()`;
/// `resume_state` is written only by the resume dispatcher and is never included in any API
/// response. Deliberately not `Serialize`: this is a DB row mirror, not a wire type — an API
/// handler must build its own response DTO rather than returning this directly, so `resume_state`
/// can never leak by a stray `Json(row)`.
#[derive(Debug, Clone, Deserialize)]
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

    /// kind=tool_approval only. Part of the approval's matching identity alongside `agent_id`
    /// and `context_id` — see `uq_hitl_pending_per_tool_call`.
    pub connector_id: Option<Uuid>,
    /// kind=tool_approval only. See `connector_id`.
    pub tool_name: Option<String>,
    /// Audit-only: shown to the human at approval time, but not part of the matching key — a
    /// retried call is matched by (agent_id, connector_id, tool_name, context_id), not by
    /// hashing arguments, since a retry may carry regenerated (non-identical) arguments.
    pub arguments_hash: Option<String>,
    pub consumed_at: Option<DateTime<Utc>>,

    pub question: Value,
    pub human_response: Option<Value>,
    pub resume_state: Value,

    /// Delivery lease for the resume dispatcher; `None` = unclaimed. Mirrors `build_jobs.picked_at`.
    pub resume_claimed_at: Option<DateTime<Utc>>,
    pub resume_dispatch_attempts: i32,
    pub resume_last_error: Option<String>,

    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    pub resolved_at: Option<DateTime<Utc>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    // Mirrors the CHECK constraint lists in `oss/migrations/0007_hitl.sql` verbatim. There's no
    // sqlx wiring in this crate yet to derive this from the DB directly, so it's asserted by hand
    // — if one side changes without the other, these tests catch the drift.
    const SQL_KIND_VALUES: &[&str] = &["input_required", "auth_required", "tool_approval"];
    const SQL_ORIGIN_VALUES: &[&str] = &[
        "direct_chat",
        "agent_proxy",
        "orchestrator",
        "maf",
        "mcp_tool",
    ];
    const SQL_STATUS_VALUES: &[&str] = &["pending", "resolved", "rejected", "expired", "canceled"];
    const SQL_RESUME_STATUS_VALUES: &[&str] = &[
        "not_started",
        "completed",
        "failed",
        "delivery_outcome_unknown",
    ];

    fn assert_round_trips<T>(variants: &[T])
    where
        T: fmt::Display + FromStr + PartialEq + fmt::Debug,
        T::Err: fmt::Debug,
    {
        for variant in variants {
            let wire = variant.to_string();
            let parsed: T = wire.parse().expect("as_str()/Display output must re-parse");
            assert_eq!(&parsed, variant, "FromStr(Display(v)) != v for {wire:?}");
        }
    }

    fn assert_matches_sql_check<T: fmt::Display>(variants: &[T], expected: &[&str]) {
        let actual: Vec<String> = variants.iter().map(|v| v.to_string()).collect();
        assert_eq!(
            actual.len(),
            expected.len(),
            "variant count doesn't match the SQL CHECK list"
        );
        for wire in expected {
            assert!(
                actual.iter().any(|s| s == wire),
                "SQL CHECK allows {wire:?} but no enum variant produces it"
            );
        }
    }

    #[test]
    fn hitl_kind_round_trips_and_matches_sql_check() {
        let all = [
            HitlKind::InputRequired,
            HitlKind::AuthRequired,
            HitlKind::ToolApproval,
        ];
        assert_round_trips(&all);
        assert_matches_sql_check(&all, SQL_KIND_VALUES);
    }

    #[test]
    fn hitl_origin_round_trips_and_matches_sql_check() {
        let all = [
            HitlOrigin::DirectChat,
            HitlOrigin::AgentProxy,
            HitlOrigin::Orchestrator,
            HitlOrigin::Maf,
            HitlOrigin::McpTool,
        ];
        assert_round_trips(&all);
        assert_matches_sql_check(&all, SQL_ORIGIN_VALUES);
    }

    #[test]
    fn hitl_status_round_trips_and_matches_sql_check() {
        let all = [
            HitlStatus::Pending,
            HitlStatus::Resolved,
            HitlStatus::Rejected,
            HitlStatus::Expired,
            HitlStatus::Canceled,
        ];
        assert_round_trips(&all);
        assert_matches_sql_check(&all, SQL_STATUS_VALUES);
    }

    #[test]
    fn resume_status_round_trips_and_matches_sql_check() {
        let all = [
            ResumeStatus::NotStarted,
            ResumeStatus::Completed,
            ResumeStatus::Failed,
            ResumeStatus::DeliveryOutcomeUnknown,
        ];
        assert_round_trips(&all);
        assert_matches_sql_check(&all, SQL_RESUME_STATUS_VALUES);
    }

    #[test]
    fn serde_round_trip_agrees_with_as_str() {
        for kind in [
            HitlKind::InputRequired,
            HitlKind::AuthRequired,
            HitlKind::ToolApproval,
        ] {
            let json = serde_json::to_string(&kind).unwrap();
            assert_eq!(json, format!("\"{}\"", kind.as_str()));
            let back: HitlKind = serde_json::from_str(&json).unwrap();
            assert_eq!(back, kind);
        }
    }

    #[test]
    fn unknown_value_is_a_parse_error_not_a_panic() {
        let err = "not_a_real_status".parse::<HitlStatus>().unwrap_err();
        assert_eq!(err.enum_name, "HitlStatus");
        assert_eq!(err.value, "not_a_real_status");
    }
}

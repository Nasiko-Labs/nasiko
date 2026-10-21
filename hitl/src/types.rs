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

// Delivery state of the human's decision to the paused execution. In-flight is not a variant:
// an attempt is underway when `HitlRequest::resume_claimed_at` is set while this is still
// `NotStarted` (the lease lives on the row, mirroring `build_jobs.picked_at`).
// `Completed` = peer confirmed receipt; `Failed` = attempts exhausted or non-retryable;
// `DeliveryOutcomeUnknown` = lease expired mid-attempt, set by the recovery sweep, never
// auto-retried. `Skipped` = resume deliberately never attempted for this row (a mirrored
// `mcp_tool` row whose real resume happens on its linked `direct_chat`/`agent_proxy` row instead
// — see `repo::skip_resume_for_mirrored_row`'s own doc comment); distinct from `Completed`
// because no delivery attempt was ever made.
db_enum!(ResumeStatus {
    NotStarted => "not_started",
    Completed => "completed",
    Failed => "failed",
    DeliveryOutcomeUnknown => "delivery_outcome_unknown",
    Skipped => "skipped",
});

/// Wire vocabulary for the `tool_approval`/`auth_required` resolve flows. These live outside
/// `db_enum!` because none of them are their own DB column — `decision`/`scope`/`auth_action` are
/// request-only fields on `HitlResolveRequest`, and `auth_outcome` is a key inside
/// `human_response` JSONB, not a CHECK-backed column — but the values still need one canonical
/// spelling shared by `oss/server/src/router/hitl.rs` (the resolve API), `oss/hitl/src/notifier.rs`
/// (the resume message builder) and `oss/server/src/hitl/mod.rs` (the agent-facing reply text),
/// which previously each hardcoded their own copies of these strings.
pub const DECISION_APPROVE: &str = "approve";
pub const DECISION_REJECT: &str = "reject";

pub const GRANT_SCOPE_ONCE: &str = "once";
pub const GRANT_SCOPE_SESSION: &str = "session";

pub const AUTH_ACTION_START: &str = "start";
pub const AUTH_ACTION_CONFIRM: &str = "confirm";

pub const AUTH_OUTCOME_CONFIRMED: &str = "confirmed";
pub const AUTH_OUTCOME_DENIED: &str = "denied";
/// Not an outcome value — the literal reply text echoed to the agent for a successful
/// `auth_required` resume. Named separately because a deterministic agent may match this word
/// literally (see `oss/server/src/hitl/mod.rs::answer_text`'s own doc comment).
pub const AUTH_REPLY_AUTHORIZED: &str = "authorized";

/// Mirrors the `hitl_requests` table (migration `0007_hitl.sql`).
///
/// `question` is write-once at creation; `human_response` is written only by `resolve()`;
/// `resume_state` is written only by the resume dispatcher and is never included in any API
/// response. Deliberately not `Serialize`: this is a DB row mirror, not a wire type — an API
/// handler must build its own response DTO rather than returning this directly, so `resume_state`
/// can never leak by a stray `Json(row)`.
#[derive(Debug, Clone)]
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

/// A validated, not-yet-persisted HITL request. Fields are private; the only way to build one
/// is through the origin-specific smart constructors below, each of which only exposes the
/// fields valid for that origin per the `hitl_requests` validity table (`0007_hitl.sql`) — an
/// invalid field combination (e.g. a `maf_execution_id` on a `direct_chat` row) is simply
/// unconstructable rather than checked at runtime.
#[derive(Debug, Clone)]
pub struct NewHitlRequest {
    pub(crate) kind: HitlKind,
    pub(crate) origin: HitlOrigin,
    pub(crate) agent_id: Uuid,
    pub(crate) owner_user_id: Uuid,
    pub(crate) task_id: Option<String>,
    pub(crate) context_id: Option<String>,
    pub(crate) chat_session_id: Option<String>,
    pub(crate) maf_execution_id: Option<Uuid>,
    pub(crate) maf_step_index: Option<i32>,
    pub(crate) connector_id: Option<Uuid>,
    pub(crate) tool_name: Option<String>,
    pub(crate) arguments_hash: Option<String>,
    pub(crate) question: Value,
}

impl NewHitlRequest {
    /// `origin = direct_chat` — `agent_stream()`'s direct-agent path.
    pub fn direct_chat(
        kind: HitlKind,
        agent_id: Uuid,
        owner_user_id: Uuid,
        task_id: impl Into<String>,
        context_id: impl Into<String>,
        question: Value,
    ) -> Self {
        Self::conversational(
            HitlOrigin::DirectChat,
            kind,
            agent_id,
            owner_user_id,
            task_id,
            context_id,
            None,
            question,
        )
    }

    /// `origin = agent_proxy` — `agent_proxy.rs`'s direct-agent path (Phase 5).
    pub fn agent_proxy(
        kind: HitlKind,
        agent_id: Uuid,
        owner_user_id: Uuid,
        task_id: impl Into<String>,
        context_id: impl Into<String>,
        question: Value,
    ) -> Self {
        Self::conversational(
            HitlOrigin::AgentProxy,
            kind,
            agent_id,
            owner_user_id,
            task_id,
            context_id,
            None,
            question,
        )
    }

    /// `origin = orchestrator` — a paused sub-agent call within a ReAct turn (Phase 7).
    /// `chat_session_id` is the outer chat session the turn belongs to.
    pub fn orchestrator(
        kind: HitlKind,
        agent_id: Uuid,
        owner_user_id: Uuid,
        task_id: impl Into<String>,
        context_id: impl Into<String>,
        chat_session_id: impl Into<String>,
        question: Value,
    ) -> Self {
        Self::conversational(
            HitlOrigin::Orchestrator,
            kind,
            agent_id,
            owner_user_id,
            task_id,
            context_id,
            Some(chat_session_id.into()),
            question,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn conversational(
        origin: HitlOrigin,
        kind: HitlKind,
        agent_id: Uuid,
        owner_user_id: Uuid,
        task_id: impl Into<String>,
        context_id: impl Into<String>,
        chat_session_id: Option<String>,
        question: Value,
    ) -> Self {
        Self {
            kind,
            origin,
            agent_id,
            owner_user_id,
            task_id: Some(task_id.into()),
            context_id: Some(context_id.into()),
            chat_session_id,
            maf_execution_id: None,
            maf_step_index: None,
            connector_id: None,
            tool_name: None,
            arguments_hash: None,
            question,
        }
    }

    /// `origin = maf` — a paused step within a MAF execution (Phase 8).
    #[allow(clippy::too_many_arguments)]
    pub fn maf(
        kind: HitlKind,
        agent_id: Uuid,
        owner_user_id: Uuid,
        task_id: impl Into<String>,
        context_id: impl Into<String>,
        maf_execution_id: Uuid,
        maf_step_index: i32,
        question: Value,
    ) -> Self {
        Self {
            kind,
            origin: HitlOrigin::Maf,
            agent_id,
            owner_user_id,
            task_id: Some(task_id.into()),
            context_id: Some(context_id.into()),
            chat_session_id: None,
            maf_execution_id: Some(maf_execution_id),
            maf_step_index: Some(maf_step_index),
            connector_id: None,
            tool_name: None,
            arguments_hash: None,
            question,
        }
    }

    /// `origin = mcp_tool`, `kind = tool_approval` always — a blocked `tools/call` awaiting
    /// human approval (Phase 6). No `task_id`: the paused thing is one HTTP request, not an
    /// A2A task (§2.4). `arguments_hash` is audit/display-only, never part of the matching key.
    pub fn mcp_tool(
        agent_id: Uuid,
        owner_user_id: Uuid,
        context_id: impl Into<String>,
        connector_id: Uuid,
        tool_name: impl Into<String>,
        arguments_hash: Option<String>,
        question: Value,
    ) -> Self {
        Self {
            kind: HitlKind::ToolApproval,
            origin: HitlOrigin::McpTool,
            agent_id,
            owner_user_id,
            task_id: None,
            context_id: Some(context_id.into()),
            chat_session_id: None,
            maf_execution_id: None,
            maf_step_index: None,
            connector_id: Some(connector_id),
            tool_name: Some(tool_name.into()),
            arguments_hash,
            question,
        }
    }

    /// Attaches the owning chat session after construction, additively — every existing
    /// constructor call site keeps compiling unchanged. `direct_chat()` has no
    /// `chat_session_id` parameter of its own (unlike `orchestrator()`, which takes one because
    /// its origin always has one); the web UI's chat session is known only at the call site in
    /// `a2a_dispatch.rs`, chained on here instead of widening every constructor's arg list.
    /// Accepts either a bare id or an `Option` so both a freshly-resolved session id and one
    /// propagated verbatim from a prior row (sequential HITL, `hitl/mod.rs::deliver`) chain the
    /// same way.
    pub fn with_chat_session_id(mut self, id: impl Into<Option<String>>) -> Self {
        self.chat_session_id = id.into();
        self
    }
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
        "skipped",
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
            ResumeStatus::Skipped,
        ];
        assert_round_trips(&all);
        assert_matches_sql_check(&all, SQL_RESUME_STATUS_VALUES);
    }

    fn assert_serde_agrees_with_as_str<T>(variants: &[T])
    where
        T: Serialize + for<'de> Deserialize<'de> + Copy + PartialEq + fmt::Debug + fmt::Display,
    {
        for &variant in variants {
            let json = serde_json::to_string(&variant).unwrap();
            assert_eq!(
                json,
                format!("\"{}\"", variant),
                "serde output disagrees with Display/as_str()"
            );
            let back: T = serde_json::from_str(&json).unwrap();
            assert_eq!(back, variant);
        }
    }

    #[test]
    fn serde_round_trip_agrees_with_as_str() {
        assert_serde_agrees_with_as_str(&[
            HitlKind::InputRequired,
            HitlKind::AuthRequired,
            HitlKind::ToolApproval,
        ]);
        assert_serde_agrees_with_as_str(&[
            HitlOrigin::DirectChat,
            HitlOrigin::AgentProxy,
            HitlOrigin::Orchestrator,
            HitlOrigin::Maf,
            HitlOrigin::McpTool,
        ]);
        assert_serde_agrees_with_as_str(&[
            HitlStatus::Pending,
            HitlStatus::Resolved,
            HitlStatus::Rejected,
            HitlStatus::Expired,
            HitlStatus::Canceled,
        ]);
        assert_serde_agrees_with_as_str(&[
            ResumeStatus::NotStarted,
            ResumeStatus::Completed,
            ResumeStatus::Failed,
            ResumeStatus::DeliveryOutcomeUnknown,
            ResumeStatus::Skipped,
        ]);
    }

    // Parses the value list out of `CHECK (<column> IN (...))` for one column, by name, out of
    // the raw migration source. Anchoring on `CHECK (<column>` (not just `<column>`) and a `\b`
    // after it is what keeps `status` from matching inside `resume_status`'s own CHECK block.
    fn parse_check_values(migration: &str, column: &str) -> Vec<String> {
        let pattern = format!(r"CHECK\s*\(\s*{column}\b\s+IN\s*\(([^)]*)\)");
        let re = regex::Regex::new(&pattern).expect("pattern is a fixed, valid regex");
        let captures = re.captures(migration).unwrap_or_else(|| {
            panic!("no `CHECK ({column} IN (...))` block found in the migration")
        });
        captures[1]
            .split(',')
            .map(|v| v.trim().trim_matches('\'').to_string())
            .collect()
    }

    #[test]
    fn sql_check_values_match_the_migration_exactly() {
        // Unlike assert_matches_sql_check (which only checks the hand-copied consts above
        // against the enums), this parses each column's real CHECK (...) block out of the
        // migration file and compares it against those consts value-for-value, in order — so
        // the migration and the consts above can't silently drift apart in either direction.
        //
        // `resume_status` reads from 0027, not 0007: sqlx hashes applied migration files, so a
        // widened CHECK on an already-shipped column has to be re-expressed as a forward-only
        // ALTER (0011_baseline_deltas.sql's own precedent) rather than edited in place — 0027 is
        // the column's current source of truth, same as 0007 still is for the other three.
        let migration_0007 = include_str!("../../migrations/0007_hitl.sql");
        let migration_0027 = include_str!("../../migrations/0027_hitl_resume_status_skipped.sql");
        for (migration, column, expected) in [
            (migration_0007, "kind", SQL_KIND_VALUES),
            (migration_0007, "origin", SQL_ORIGIN_VALUES),
            (migration_0007, "status", SQL_STATUS_VALUES),
            (migration_0027, "resume_status", SQL_RESUME_STATUS_VALUES),
        ] {
            let actual = parse_check_values(migration, column);
            assert_eq!(
                actual, expected,
                "CHECK ({column} IN (...)) in the migration no longer matches SQL_*_VALUES above"
            );
        }
    }

    #[test]
    fn unknown_value_is_a_parse_error_not_a_panic() {
        let err = "not_a_real_status".parse::<HitlStatus>().unwrap_err();
        assert_eq!(err.enum_name, "HitlStatus");
        assert_eq!(err.value, "not_a_real_status");
    }

    // Per-origin constructor validity — mirrors 0007_hitl.sql's validity table (§4 of the HITL
    // plan): each constructor must set exactly the fields valid for its origin and leave every
    // other origin-specific field unset.
    #[test]
    fn direct_chat_constructor_sets_only_task_and_context() {
        let req = NewHitlRequest::direct_chat(
            HitlKind::InputRequired,
            Uuid::new_v4(),
            Uuid::new_v4(),
            "task-1",
            "ctx-1",
            Value::Null,
        );
        assert_eq!(req.origin, HitlOrigin::DirectChat);
        assert_eq!(req.task_id.as_deref(), Some("task-1"));
        assert_eq!(req.context_id.as_deref(), Some("ctx-1"));
        assert!(req.chat_session_id.is_none());
        assert!(req.maf_execution_id.is_none());
        assert!(req.maf_step_index.is_none());
        assert!(req.connector_id.is_none());
        assert!(req.tool_name.is_none());
    }

    #[test]
    fn agent_proxy_constructor_matches_direct_chat_shape() {
        let req = NewHitlRequest::agent_proxy(
            HitlKind::AuthRequired,
            Uuid::new_v4(),
            Uuid::new_v4(),
            "task-1",
            "ctx-1",
            Value::Null,
        );
        assert_eq!(req.origin, HitlOrigin::AgentProxy);
        assert!(req.task_id.is_some());
        assert!(req.context_id.is_some());
        assert!(req.chat_session_id.is_none());
    }

    #[test]
    fn orchestrator_constructor_sets_chat_session_id() {
        let req = NewHitlRequest::orchestrator(
            HitlKind::InputRequired,
            Uuid::new_v4(),
            Uuid::new_v4(),
            "task-1",
            "ctx-1",
            "session-1",
            Value::Null,
        );
        assert_eq!(req.origin, HitlOrigin::Orchestrator);
        assert!(req.task_id.is_some());
        assert!(req.context_id.is_some());
        assert_eq!(req.chat_session_id.as_deref(), Some("session-1"));
        assert!(req.maf_execution_id.is_none());
    }

    #[test]
    fn maf_constructor_sets_execution_and_step_only() {
        let exec_id = Uuid::new_v4();
        let req = NewHitlRequest::maf(
            HitlKind::AuthRequired,
            Uuid::new_v4(),
            Uuid::new_v4(),
            "task-1",
            "ctx-1",
            exec_id,
            3,
            Value::Null,
        );
        assert_eq!(req.origin, HitlOrigin::Maf);
        assert!(req.task_id.is_some());
        assert!(req.context_id.is_some());
        assert!(req.chat_session_id.is_none());
        assert_eq!(req.maf_execution_id, Some(exec_id));
        assert_eq!(req.maf_step_index, Some(3));
        assert!(req.connector_id.is_none());
    }

    #[test]
    fn mcp_tool_constructor_forces_tool_approval_kind_and_no_task_id() {
        let connector_id = Uuid::new_v4();
        let req = NewHitlRequest::mcp_tool(
            Uuid::new_v4(),
            Uuid::new_v4(),
            "ctx-1",
            connector_id,
            "github_create_issue",
            Some("sha256:abc".to_string()),
            Value::Null,
        );
        assert_eq!(req.origin, HitlOrigin::McpTool);
        assert_eq!(req.kind, HitlKind::ToolApproval);
        assert!(req.task_id.is_none());
        assert_eq!(req.context_id.as_deref(), Some("ctx-1"));
        assert_eq!(req.connector_id, Some(connector_id));
        assert_eq!(req.tool_name.as_deref(), Some("github_create_issue"));
        assert_eq!(req.arguments_hash.as_deref(), Some("sha256:abc"));
        assert!(req.maf_execution_id.is_none());
        assert!(req.chat_session_id.is_none());
    }
}

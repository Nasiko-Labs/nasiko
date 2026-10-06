//! Postgres persistence for `hitl_requests` (migration `0007_hitl.sql` +
//! `0022_hitl_auth_required.sql`).
//!
//! `oss/mcp-gateway` calls `create_pending_auth_required`/`create_pending_auth_required_with_ttl`
//! from `protocol::handle_auth_required` and `create_pending_tool_approval`/
//! `create_pending_tool_approval_with_ttl` from `protocol::create_tool_approval_id`. `resolve`
//! (used directly by tests, and indirectly via `HitlStore::resolve` in production — see
//! `store.rs`) never triggers a retry, a push, or a session grant — it only flips the row's own
//! status; `claim_for_resume`/`finish_resume`/`recover_stuck_resumes` (M6) are the resume
//! dispatcher's own claim/lease primitives, consumed by `crate::dispatcher`. The `/api/hitl/*`
//! human-facing API (`oss/server/src/router/hitl.rs`) goes entirely through the `HitlStore` trait
//! (`store.rs`) and `authz::authorize_hitl_action` — `HitlRequestRow`/`HitlError`/
//! `TryFrom<HitlRequestRow>` are shared with `store.rs` rather than duplicated here.

use chrono::{Duration, Utc};
use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

use crate::store::{HitlError, HitlRequestRow};
use crate::types::{
    AUTH_OUTCOME_CONFIRMED, DECISION_APPROVE, HitlRequest, HitlStatus, ResumeStatus,
};

type Result<T> = std::result::Result<T, HitlError>;

/// Default validity window for a pending request before it's considered
/// expired — 7 days, per the plan's "Remaining Decisions". Used only when a caller doesn't supply
/// its own `ttl_days` (`create_pending_auth_required`/`create_pending_tool_approval`, kept for
/// existing callers and tests) — real production callers should use the `_with_ttl`
/// variants with `Config::hitl_request_ttl_days` (env: `HITL_REQUEST_TTL_DAYS`) instead, matching
/// `PgHitlStore::with_ttl_days`'s equivalent knob for every other `HitlKind`. Before the `_with_ttl`
/// variants existed, setting that env var silently had no effect on any row this module created
/// (found in review) — `mcp_tool`-origin rows always got exactly 7 days no matter what was
/// configured.
const DEFAULT_EXPIRY_DAYS: i64 = 7;

/// Everything needed to create a pending `kind=auth_required`,
/// `origin=mcp_tool` request. Deliberately silent on *how* `context_id` was
/// resolved (a `session_traces` lookup from the forwarded `traceparent`, in
/// MCP's case, per the HITL blueprint) — the caller resolves it and hands it
/// over; this module only persists it, so it stays agnostic to whichever
/// resume mechanism (the future shared dispatcher) ends up consuming the row.
#[derive(Debug, Clone)]
pub struct NewAuthRequired {
    pub agent_id: Uuid,
    /// The user whose credential is missing/expired — also the sole
    /// authorization principal for this row once an approval/resolve API
    /// exists (owner_user_id is the one rule for every `HitlKind`, per the
    /// plan).
    pub owner_user_id: Uuid,
    /// The connector currently being resolved when the failure was detected.
    /// Part of this row's idempotent-creation identity alongside `agent_id`
    /// and `context_id` — see `uq_hitl_pending_per_connector_auth`.
    pub connector_id: Uuid,
    /// The paused conversation the resume dispatcher will eventually push
    /// "authentication is complete, retry `<tool>`" onto. Required — see
    /// `chk_hitl_mcp_auth_required_identity`.
    pub context_id: String,
    /// Free-form, human/agent-facing payload (connector name, provider,
    /// `auth_url`, the tool name that triggered detection, a message, …).
    /// Deliberately not schema-typed so a later milestone can add fields
    /// without a migration — mirrors how `tool_approval` already treats
    /// `arguments_hash` as audit/display data outside the matching key.
    pub question: Value,
}

/// Create a pending `auth_required`/`mcp_tool` request, or — if one already
/// exists for this exact `(owner_user_id, agent_id, connector_id, context_id)`
/// — return that existing row unchanged (only `updated_at` is bumped).
/// Idempotent by construction via `uq_hitl_pending_per_connector_auth`: safe
/// to call once per failed tool call against the same unusable connector
/// without ever creating a duplicate pending row.
pub async fn create_pending_auth_required(
    db: &PgPool,
    req: NewAuthRequired,
) -> Result<HitlRequest> {
    create_pending_auth_required_with_ttl(db, req, DEFAULT_EXPIRY_DAYS).await
}

/// Same as [`create_pending_auth_required`], but with an explicit TTL — the real entry point for
/// production callers, which should pass `Config::hitl_request_ttl_days` rather than relying on
/// the fixed `DEFAULT_EXPIRY_DAYS` fallback.
pub async fn create_pending_auth_required_with_ttl(
    db: &PgPool,
    req: NewAuthRequired,
    ttl_days: i64,
) -> Result<HitlRequest> {
    let expires_at = Utc::now() + Duration::days(ttl_days);
    let row = sqlx::query_as::<_, HitlRequestRow>(
        r#"
        INSERT INTO hitl_requests
            (kind, origin, agent_id, owner_user_id, connector_id, context_id, question, expires_at)
        VALUES
            ('auth_required', 'mcp_tool', $1, $2, $3, $4, $5, $6)
        -- `owner_user_id` is part of the conflict target, not just the row content (security
        -- review, migration 0023): `context_id` falls back to the raw, agent-controlled trace id
        -- when no `session_traces` mapping exists — without `owner_user_id` here, two different
        -- users whose calls happened to collide on the same (agent, connector, context) tuple
        -- would silently `DO UPDATE` and `RETURNING *` each other's row, handing one user's
        -- pending auth-required row back to the other's request. Same class of bug
        -- `0025_hitl_task_id_scope.sql` closed for `uq_hitl_pending_per_task`.
        ON CONFLICT (owner_user_id, agent_id, connector_id, context_id)
            WHERE status = 'pending' AND kind = 'auth_required' AND origin = 'mcp_tool'
            -- Also refreshes `expires_at`, not just `updated_at`: without this, a connector that
            -- stays broken past the original row's TTL expires (`expire_stale`) while still being
            -- actively hit on every call, and each hit after that creates a brand-new row instead
            -- of reusing this one (found in review) — every real re-hit should push the deadline
            -- out exactly as far as a fresh row would get.
            DO UPDATE SET updated_at = now(), expires_at = EXCLUDED.expires_at
        RETURNING *
        "#,
    )
    .bind(req.agent_id)
    .bind(req.owner_user_id)
    .bind(req.connector_id)
    .bind(&req.context_id)
    .bind(req.question)
    .bind(expires_at)
    .fetch_one(db)
    .await?;

    row.try_into()
}

/// Everything needed to create a pending `kind=tool_approval`,
/// `origin=mcp_tool` request. `arguments_hash` is deliberately not part of
/// this constructor — the finalized matching key for a retried call is tool
/// identity `(agent_id, connector_id, tool_name, context_id)`, never
/// argument content (a retry may regenerate slightly different arguments),
/// so a hash is audit/display data a later milestone can add without
/// changing this signature.
#[derive(Debug, Clone)]
pub struct NewToolApproval {
    pub agent_id: Uuid,
    /// The user whose approval decision is being asked for — the delegating
    /// user, not whoever manages the agent (connector credentials are always
    /// the calling user's own). Also the sole authorization principal for
    /// this row once a resolve API exists.
    pub owner_user_id: Uuid,
    /// The connector the tool belongs to. Part of this row's
    /// idempotent-creation identity — see `uq_hitl_pending_per_tool_call`.
    pub connector_id: Uuid,
    /// The un-namespaced tool name a retried call's own routing would
    /// resolve to (never the `{connector_prefix}__tool` wire form for
    /// generic MCP tools) — part of this row's identity alongside
    /// `agent_id`, `connector_id`, and `context_id`.
    pub tool_name: String,
    /// The paused conversation a future resolve/retry flow is scoped to.
    /// Required — see `chk_hitl_tool_approval_identity`.
    pub context_id: String,
    /// Free-form, human/agent-facing payload (tool name, connector label, a
    /// message, …). Deliberately not schema-typed so a later milestone can
    /// add fields without a migration.
    pub question: Value,
}

/// Create a pending `tool_approval` request, or — if one already exists for
/// this exact `(owner_user_id, agent_id, connector_id, tool_name, context_id)`
/// — return that existing row unchanged (only `updated_at` is bumped).
/// Idempotent by construction via `uq_hitl_pending_per_tool_call`
/// (`0007_hitl.sql`): safe to call once per `Stance::Ask` decision without
/// ever creating a duplicate pending row for the same tool/conversation.
pub async fn create_pending_tool_approval(
    db: &PgPool,
    req: NewToolApproval,
) -> Result<HitlRequest> {
    create_pending_tool_approval_with_ttl(db, req, DEFAULT_EXPIRY_DAYS).await
}

/// Same as [`create_pending_tool_approval`], but with an explicit TTL — the real entry point for
/// production callers, which should pass `Config::hitl_request_ttl_days` rather than relying on
/// the fixed `DEFAULT_EXPIRY_DAYS` fallback.
pub async fn create_pending_tool_approval_with_ttl(
    db: &PgPool,
    req: NewToolApproval,
    ttl_days: i64,
) -> Result<HitlRequest> {
    let expires_at = Utc::now() + Duration::days(ttl_days);
    let row = sqlx::query_as::<_, HitlRequestRow>(
        r#"
        INSERT INTO hitl_requests
            (kind, origin, agent_id, owner_user_id, connector_id, tool_name, context_id, question, expires_at)
        VALUES
            ('tool_approval', 'mcp_tool', $1, $2, $3, $4, $5, $6, $7)
        -- `owner_user_id` is part of the conflict target, not just the row content — same
        -- security fix, same reasoning, as `create_pending_auth_required_with_ttl`'s identical
        -- `ON CONFLICT` above (migration 0023): without it, a forced or coincidental collision on
        -- (agent, connector, tool, context) between two different users' calls would silently
        -- `DO UPDATE` and return one user's pending tool-approval row to the other's request.
        ON CONFLICT (owner_user_id, agent_id, connector_id, tool_name, context_id)
            WHERE status = 'pending' AND kind = 'tool_approval'
            -- Same fix as `create_pending_auth_required_with_ttl`'s identical `ON CONFLICT`: also
            -- refresh `expires_at`, not just `updated_at`, so a repeatedly-retried tool call keeps
            -- pushing its own deadline out instead of expiring mid-retry and then forking into a
            -- brand-new row.
            DO UPDATE SET updated_at = now(), expires_at = EXCLUDED.expires_at
        RETURNING *
        "#,
    )
    .bind(req.agent_id)
    .bind(req.owner_user_id)
    .bind(req.connector_id)
    .bind(&req.tool_name)
    .bind(&req.context_id)
    .bind(req.question)
    .bind(expires_at)
    .fetch_one(db)
    .await?;

    row.try_into()
}

/// Look up a request by id — read-only, used by the tests below and by
/// whichever future caller (a resolve endpoint, the dispatcher) needs to
/// re-fetch a row it already knows the id of.
pub async fn get_by_id(db: &PgPool, id: Uuid) -> Result<Option<HitlRequest>> {
    let row = sqlx::query_as::<_, HitlRequestRow>("SELECT * FROM hitl_requests WHERE id = $1")
        .bind(id)
        .fetch_optional(db)
        .await?;
    row.map(HitlRequest::try_from).transpose()
}

/// The `direct_chat`/`agent_proxy`/`maf`/`orchestrator`-origin row (if any, still `pending`)
/// mirroring this resolved `mcp_tool`-origin row's own event — an agent that
/// maps MCP's `ask_required`/`auth_required` onto the A2A `AUTH_REQUIRED`
/// task state creates its own separate row for the same pause, tagging it
/// with `question.metadata.hitl_request_id` pointing back at this one
/// (`build_pause_question` in `oss/types/src/a2a.rs` forwards the agent's
/// own status-message metadata verbatim). A sub-agent delegated to by the
/// orchestrator can use MCP tools just like a direct-chat agent can, so this
/// mirroring is not limited to direct chat/MAF — an `orchestrator`-origin row
/// can equally be a mirror.
///
/// Resolving only the MCP row leaves that mirrored row pending forever — the
/// mirror's own dispatcher only ever acts on rows it owns, and MCP's own
/// dispatcher sends a stateless, task-blind nudge that can never reach the
/// *specific* chat task/MAF step/orchestrator turn a human is watching (found
/// live: task-aware resume only exists on the `direct_chat`/`agent_proxy`/
/// `maf`/`orchestrator` side). The caller
/// (`router/hitl.rs::resolve`) uses this to auto-resolve the mirrored row in
/// lockstep with the one the human actually clicked, so a single approval
/// action both grants the real permission (this row) and resumes the
/// specific visible task/step the human is looking at (the mirrored row,
/// through its own existing, unmodified dispatcher).
///
/// `owner_user_id` (always the *real mcp row's* owner, i.e. the caller who is authorized to act on
/// it) is a required predicate, not an afterthought — a security fix, not a tidiness one.
/// `question.metadata.hitl_request_id` is fully agent-controlled (`build_pause_question` forwards
/// the agent's own status-message metadata verbatim, unvalidated), so without this filter a
/// malicious or buggy agent could plant *any* other user's real `hitl_request_id` — even one from
/// a completely unrelated event in the agent author's own history — into a victim's pause, and
/// have that victim's resolve of their own unrelated row silently auto-resolve, alias-hijack, and
/// resume the attacker-linked row instead: a forged approval plus (via
/// `ContinuationRegistry::alias`, `router/hitl.rs`) a way to read the resumed execution's output
/// under the *other* user's identity. Same root cause and same fix shape as
/// `resolve_display_row`'s own `caller_owner_id` check (`store.rs`).
///
/// `agent_id` is likewise required, matching `resolve_display_row`'s own check and its own
/// reasoning: the two halves of one real pause always belong to the same agent, so a link naming a
/// different agent's row can only be forged or stale. Without it, one agent's pause could be
/// resolved and its `ContinuationRegistry::alias` output-stream access granted via a link to a
/// different agent's row. `ORDER BY created_at DESC` makes the choice deterministic when duplicate
/// mirrors exist, rather than picking whichever row the query planner happens to return first.
pub async fn find_linked_direct_chat_row(
    db: &PgPool,
    mcp_row_id: Uuid,
    owner_user_id: Uuid,
    agent_id: Uuid,
) -> Result<Option<HitlRequest>> {
    let row = sqlx::query_as::<_, HitlRequestRow>(
        r#"
        SELECT * FROM hitl_requests
         WHERE origin IN ('direct_chat', 'agent_proxy', 'maf', 'orchestrator')
           AND status = 'pending'
           AND owner_user_id = $2
           AND agent_id = $3
           AND question->'metadata'->>'hitl_request_id' = $1
         ORDER BY created_at DESC
         LIMIT 1
        "#,
    )
    .bind(mcp_row_id.to_string())
    .bind(owner_user_id)
    .bind(agent_id)
    .fetch_optional(db)
    .await?;
    row.map(HitlRequest::try_from).transpose()
}

/// The human's decision on a pending request. Deliberately just these two —
/// "allow once" vs. "allow for this session" is a `tool_approval`-specific
/// distinction that belongs to the future retry-matching/session-grant work
/// (an unbuilt consumer of `Resolved`, not a different terminal status), not
/// to this generic status transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolveDecision {
    Approve,
    Reject,
}

impl ResolveDecision {
    pub fn target_status(self) -> HitlStatus {
        match self {
            Self::Approve => HitlStatus::Resolved,
            Self::Reject => HitlStatus::Rejected,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Approve => crate::types::DECISION_APPROVE,
            Self::Reject => crate::types::DECISION_REJECT,
        }
    }

    /// Parses the `decision` field of `HitlResolveRequest` — the wire vocabulary is exactly
    /// `crate::types::DECISION_APPROVE`/`DECISION_REJECT`, nothing else.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            crate::types::DECISION_APPROVE => Some(Self::Approve),
            crate::types::DECISION_REJECT => Some(Self::Reject),
            _ => None,
        }
    }
}

/// Atomically transition a pending request to `resolved`/`rejected`, recording
/// who decided and what they said. Returns `Ok(None)` if the row wasn't
/// `pending` at the moment of the update — either it never existed, or (the
/// case this guards against) it was already resolved/rejected by a concurrent
/// call; the `WHERE status = 'pending'` clause is the only mutual-exclusion
/// mechanism, so at most one caller ever observes `Some`.
///
/// The caller must independently authorize the action (`authorize_hitl_action`)
/// before calling this — this function has no opinion on who `resolved_by` is,
/// only that the transition itself is atomic. Deliberately generic across
/// `HitlKind`: it only flips `status`/`human_response`/`resolved_by`/
/// `resolved_at` — kind-specific consequences (session-grant creation, resume
/// dispatch) belong to later milestones, not this function.
pub async fn resolve(
    db: &PgPool,
    id: Uuid,
    decision: ResolveDecision,
    resolved_by: Uuid,
    human_response: Value,
) -> Result<Option<HitlRequest>> {
    let row = sqlx::query_as::<_, HitlRequestRow>(
        r#"
        UPDATE hitl_requests
           SET status = $2, human_response = $3, resolved_by = $4, resolved_at = now()
         WHERE id = $1 AND status = 'pending'
        RETURNING *
        "#,
    )
    .bind(id)
    .bind(decision.target_status().as_str())
    .bind(human_response)
    .bind(resolved_by)
    .fetch_optional(db)
    .await?;
    row.map(HitlRequest::try_from).transpose()
}

/// Auto-resolve every pending `auth_required`/`mcp_tool` row for this
/// `(owner_user_id, connector_id)` pair — the OAuth callback's hook
/// (`oss/mcp-gateway/src/oauth.rs::handle_callback`) once it has confirmed
/// the newly stored credential actually works.
///
/// Deliberately scoped by user+connector, not a single row id: the pending
/// rows are keyed `(agent_id, connector_id, context_id)`
/// (`uq_hitl_pending_per_connector_auth`), so a connector shared across
/// multiple agents/conversations can have several rows waiting on the exact
/// same credential fix. One successful re-auth clears all of them, not just
/// whichever tool call happened to trigger the callback.
///
/// Also auto-resolves each resolved row's linked `direct_chat`/`agent_proxy`
/// mirror, if any (see `find_linked_direct_chat_row`'s doc comment) — a real
/// broken-connector-credential pause reaches this same mirroring as a
/// `tool_approval` pause does, and this bulk path bypasses `router/hitl.rs`'s
/// `resolve()` handler entirely (mcp-gateway calls this directly), so without
/// this the mirror would stay pending until its TTL even after the real fix.
/// Best-effort: the caller (an OAuth/Composio callback) must not fail the
/// whole re-auth flow over a mirror-resolve error — this is only ever
/// resolving a second, redundant row, not the credential fix itself.
pub async fn resolve_pending_auth_required_for_connector(
    db: &PgPool,
    owner_user_id: Uuid,
    connector_id: Uuid,
) -> Result<Vec<HitlRequest>> {
    let human_response =
        serde_json::json!({"decision": DECISION_APPROVE, "auth_outcome": AUTH_OUTCOME_CONFIRMED});
    let rows = sqlx::query_as::<_, HitlRequestRow>(
        r#"
        UPDATE hitl_requests
           SET status = 'resolved', human_response = $3, resolved_by = $1, resolved_at = now()
         WHERE owner_user_id = $1 AND connector_id = $2 AND status = 'pending'
           AND kind = 'auth_required' AND origin = 'mcp_tool'
        RETURNING *
        "#,
    )
    .bind(owner_user_id)
    .bind(connector_id)
    .bind(human_response)
    .fetch_all(db)
    .await?;
    let resolved: Vec<HitlRequest> = rows
        .into_iter()
        .map(HitlRequest::try_from)
        .collect::<Result<_>>()?;
    for row in &resolved {
        if let Err(e) =
            resolve_linked_direct_chat_mirror(db, row.id, owner_user_id, row.agent_id).await
        {
            tracing::warn!(mcp_row_id = %row.id, error = %e, "failed to auto-resolve linked direct_chat mirror row");
        }
    }
    Ok(resolved)
}

/// Shared by every place that resolves an `mcp_tool`-origin row outside the
/// single-row `tool_approval` resolve API (`router/hitl.rs`'s own
/// `auto_resolve_linked_direct_chat_row` covers that one) — currently just
/// [`resolve_pending_auth_required_for_connector`]'s bulk OAuth-reconnect
/// path. Finds the linked mirror (if any, if still pending) and resolves it
/// with the same `{"auth_outcome": "confirmed"}` shape the console's own
/// two-click `auth_action: confirm` flow produces.
async fn resolve_linked_direct_chat_mirror(
    db: &PgPool,
    mcp_row_id: Uuid,
    resolved_by: Uuid,
    agent_id: Uuid,
) -> Result<()> {
    // The caller (`resolve_pending_auth_required_for_connector`) already scoped `mcp_row_id`'s own
    // `UPDATE ... WHERE owner_user_id = $1` to this same user, so `resolved_by` doubles as the real
    // mcp row's owner here — see `find_linked_direct_chat_row`'s own doc comment for why this must
    // never be skipped.
    let Some(linked) = find_linked_direct_chat_row(db, mcp_row_id, resolved_by, agent_id).await?
    else {
        return Ok(());
    };
    resolve(
        db,
        linked.id,
        ResolveDecision::Approve,
        resolved_by,
        serde_json::json!({"auth_outcome": AUTH_OUTCOME_CONFIRMED}),
    )
    .await?;
    // `linked` is the real resume now — never let the `mcp_tool` dispatcher also fire
    // `mcp_row_id`'s own task_id-less nudge on top of it. Same call
    // `router/hitl.rs::auto_resolve_linked_direct_chat_row` makes for the single-row resolve
    // path; missing it here left this bulk path's `mcp_tool` row at
    // `resume_status = 'not_started'`, so `repo::claim_for_resume` picked it up and
    // `RuntimeResumeNotifier` fired a context-free nudge racing the mirror's own, real,
    // task_id-bearing resume (found in review).
    if let Err(e) = skip_resume_for_mirrored_row(db, mcp_row_id).await {
        tracing::warn!(
            error = %e, %mcp_row_id,
            "failed to skip the mirrored mcp_tool row's own resume — it may still race the linked row's resume"
        );
    }
    Ok(())
}

/// Atomically claim the most recently resolved, not-yet-consumed
/// `tool_approval` row matching this exact `(owner_user_id, agent_id,
/// connector_id, tool_name, context_id)` tuple — the M7 retry-matching lookup
/// `protocol::handle_tools_call` performs right after `perms.decide()`
/// returns `Ask`.
///
/// `owner_user_id` is a required predicate, not an afterthought:
/// `context_id` comes from `session::resolve_context_id`, seeded from the
/// route layer's already-verified flow id (`verified_flow_id` —
/// `oss/server/src/mcp/handlers/gateway.rs::flow_user` — never a raw,
/// caller-supplied `traceparent` reparse), but it is still just a
/// trace-correlation value, not itself an identity boundary. Without also
/// matching the caller's own `user_id` (from their delegation token, the one
/// value here that actually is authenticated), an agent shared across users
/// could replay a `context_id` it legitimately observed while serving one
/// user to claim that user's approval decision on behalf of a different one
/// (found in security review — this was exploitable before this parameter
/// existed, back when `context_id` was derived directly from the
/// unauthenticated `traceparent` header).
///
/// Deliberately excludes `session`-scoped approvals
/// (`human_response->>'scope' = 'session'`): a session grant's reusability
/// lives in `mcp_session_tool_grants` instead, so its row must never be
/// claimed/consumed here — callers are expected to check
/// [`has_active_session_grant`] first. What remains claimable here is
/// exactly the "single use" set: `once`-scope (or scope-omitted) approvals,
/// and every rejection, each usable for exactly one retry — mirroring
/// `once`'s own one-time semantics from the approval side.
///
/// Concurrency-safe via `FOR UPDATE SKIP LOCKED`, the same pattern as
/// `claim_for_resume`: two concurrent retries of the same approved call can
/// never both proceed.
pub async fn claim_resolved_tool_approval(
    db: &PgPool,
    owner_user_id: Uuid,
    agent_id: Uuid,
    connector_id: Uuid,
    tool_name: &str,
    context_id: &str,
) -> Result<Option<HitlRequest>> {
    // status IN ('resolved', 'rejected'): ResolveDecision::Approve lands on
    // 'resolved', ResolveDecision::Reject lands on 'rejected' (see
    // ResolveDecision::target_status) — both are terminal, single-use
    // outcomes this claim must cover, not just the approved case.
    let row = sqlx::query_as::<_, HitlRequestRow>(
        r#"
        UPDATE hitl_requests
           SET consumed_at = now()
         WHERE id = (
             SELECT id FROM hitl_requests
              WHERE kind = 'tool_approval' AND status IN ('resolved', 'rejected')
                AND consumed_at IS NULL
                AND owner_user_id = $1
                AND agent_id = $2 AND connector_id = $3 AND tool_name = $4 AND context_id = $5
                AND (human_response ->> 'scope') IS DISTINCT FROM 'session'
              ORDER BY resolved_at DESC NULLS LAST, created_at DESC
              FOR UPDATE SKIP LOCKED
              LIMIT 1
         )
        RETURNING *
        "#,
    )
    .bind(owner_user_id)
    .bind(agent_id)
    .bind(connector_id)
    .bind(tool_name)
    .bind(context_id)
    .fetch_optional(db)
    .await?;
    row.map(HitlRequest::try_from).transpose()
}

/// Resolve the stable chat-session identity a `tool_approval` session grant
/// should be keyed by — the same `chat_sessions.session_id` direct-chat uses
/// as its own durable conversation identity (`agent_proxy.rs` upserts one row
/// per conversation and it never changes across messages within it).
///
/// This exists because the trace-derived `context_id` every other MCP HITL
/// identity uses (`session::resolve_context_id` in `oss/mcp-gateway`) is a
/// *per-message* value — a fresh distributed trace begins with every user
/// message. `resolve_context_id` already resolves it to the real
/// `chat_sessions.session_id` via `session_traces` when that mapping exists
/// (agent_proxy inserts one per forwarded message), so `current_context_id`
/// is trusted exactly when it already names a real session for this
/// `(owner_user_id, agent_id)` pair — that is the exact conversation this
/// call belongs to, not a guess.
///
/// When `current_context_id` does **not** resolve to a real session, this used to fall back to
/// "most recent session for this `(owner_user_id, agent_id)` pair" unconditionally — which let a
/// grant approved in one active chat silently authorize a tool in a *different*, more-recently-
/// touched chat with the same agent: an over-broad grant, not the unnecessary-re-ask the
/// fallback's own tradeoff assumed (found in review — twice; the fallback survived the first fix
/// that added the primary tier above, because that fix narrowed how often the fallback fires
/// without removing what it does when it does fire).
///
/// The fallback now only ever fires when it is **unambiguous**: exactly one `chat_sessions` row
/// exists for this `(owner_user_id, agent_id)` pair. With a single candidate there is nothing to
/// guess — "most recent" and "only" are the same session, so returning it carries none of the
/// original risk. With zero or two-or-more candidates this returns `None` rather than picking one:
/// zero means there is nothing to key by, and two-or-more is exactly the ambiguous case that used
/// to guess wrong. Both callers (`router/hitl.rs`'s grant write, `mcp-gateway/src/protocol.rs`'s
/// grant lookup) already fall back to the raw, per-message `context_id` on `None`, which narrows
/// the grant/lookup to that literal trace rather than mapping it onto some other conversation —
/// worst case an extra re-ask, never a wrong-chat authorization.
///
/// The primary-tier lookup matches `agent_id = $3 OR agent_id IS NULL` because
/// `ensure_orchestrator_chat_session` inserts orchestrator-routed chat sessions with `agent_id =
/// NULL` (the session fronts every sub-agent, not one) — matching on `agent_id = $3` alone made
/// this probe always miss for an orchestrator conversation, which used to fall through to a
/// direct-chat-only fallback and write the grant against whatever direct chat with this agent the
/// user happened to have open, not the orchestrator conversation the approval actually came from
/// (found in review — the same over-broad-grant failure mode, surviving for orchestrator chats
/// specifically until this lookup covered them too). The unambiguous-fallback query below applies
/// the identical relaxation for the same reason — an orchestrator session with no competing
/// candidate is exactly as safe to return as a direct-chat one.
pub async fn resolve_stable_session_context(
    db: &PgPool,
    owner_user_id: Uuid,
    agent_id: Uuid,
    current_context_id: &str,
) -> Result<Option<String>> {
    let is_current_a_real_session: bool = sqlx::query_scalar(
        r#"
        SELECT EXISTS(
            SELECT 1 FROM chat_sessions
             WHERE session_id = $1 AND user_id = $2
               AND (agent_id = $3 OR agent_id IS NULL) AND deleted_at IS NULL
        )
        "#,
    )
    .bind(current_context_id)
    .bind(owner_user_id)
    .bind(agent_id)
    .fetch_one(db)
    .await?;
    if is_current_a_real_session {
        return Ok(Some(current_context_id.to_string()));
    }

    // `LIMIT 2`, not 1: this only needs to distinguish "exactly one" from "more than one", never
    // which one is most recent — there is no safe way to break a tie between two-or-more
    // candidates, so a second row is enough to know this must return `None`.
    let candidates: Vec<String> = sqlx::query_scalar(
        r#"
        SELECT session_id FROM chat_sessions
         WHERE user_id = $1 AND (agent_id = $2 OR agent_id IS NULL) AND deleted_at IS NULL
         LIMIT 2
        "#,
    )
    .bind(owner_user_id)
    .bind(agent_id)
    .fetch_all(db)
    .await?;
    Ok(match <[String; 1]>::try_from(candidates) {
        Ok([only]) => Some(only),
        Err(_) => None,
    })
}

/// Default validity window for an "allow for this session" grant — 24 hours,
/// per the blueprint's own starting proposal ("end-of-session or 24h,
/// whichever first") for a product decision still open at TTL granularity;
/// this is the interim, documented choice, not a re-litigation of that open
/// question. `session` scope has no per-call expiry input, unlike `once`
/// (which is single-use by construction and needs none).
pub const DEFAULT_SESSION_GRANT_TTL_HOURS: i64 = 24;

/// Everything needed to record an "allow for this session" grant — created
/// by `POST /api/hitl/{id}/resolve` when a `tool_approval` request is
/// approved with `scope=session`. `expires_at` is computed internally from
/// [`DEFAULT_SESSION_GRANT_TTL_HOURS`], mirroring how
/// `create_pending_auth_required`/`create_pending_tool_approval` compute
/// their own `expires_at` rather than taking it as caller input.
#[derive(Debug, Clone)]
pub struct NewSessionGrant {
    pub agent_id: Uuid,
    pub connector_id: Uuid,
    pub tool_name: String,
    pub context_id: String,
    /// The human who approved the request — always the row's own
    /// `owner_user_id` in practice (the sole authorization principal), but
    /// passed explicitly rather than re-derived so this constructor stays
    /// independent of the caller's own row lookup.
    pub granted_by: Uuid,
    /// The `tool_approval` row this grant originated from — audit trail
    /// only, never consulted by [`has_active_session_grant`].
    pub hitl_request_id: Option<Uuid>,
}

/// Persist a new session grant. Deliberately not idempotent/upserting —
/// unlike the pending-row constructors above, a grant has no natural
/// "already exists" identity to collapse into (a second approval of the same
/// tuple, however unlikely given the retry-matching lookup, simply produces
/// a second grant with its own expiry; [`has_active_session_grant`] only
/// cares whether *any* unexpired row matches).
pub async fn create_session_grant(db: &PgPool, grant: NewSessionGrant) -> Result<()> {
    let expires_at = Utc::now() + Duration::hours(DEFAULT_SESSION_GRANT_TTL_HOURS);
    sqlx::query(
        r#"
        INSERT INTO mcp_session_tool_grants
            (agent_id, connector_id, tool_name, context_id, granted_by, hitl_request_id, expires_at)
        VALUES ($1, $2, $3, $4, $5, $6, $7)
        "#,
    )
    .bind(grant.agent_id)
    .bind(grant.connector_id)
    .bind(&grant.tool_name)
    .bind(&grant.context_id)
    .bind(grant.granted_by)
    .bind(grant.hitl_request_id)
    .bind(expires_at)
    .execute(db)
    .await?;
    Ok(())
}

/// True when an unexpired session grant exists for this exact `(granted_by,
/// agent_id, connector_id, tool_name, context_id)` tuple — the first check
/// `protocol::handle_tools_call`'s retry-matching lookup performs (before
/// [`claim_resolved_tool_approval`]), since a session grant is reusable for
/// the rest of the conversation rather than single-use.
///
/// `granted_by` is a required predicate for the same reason
/// [`claim_resolved_tool_approval`]'s `owner_user_id` is: `context_id` is
/// derived from the unauthenticated, agent-controlled `traceparent` header,
/// not an identity boundary. Without also matching the calling user's own
/// `user_id` against who the grant was actually granted to, one user's
/// "approve for this session" decision on a shared agent could be replayed
/// to skip approval for a completely different user (found in security
/// review).
pub async fn has_active_session_grant(
    db: &PgPool,
    granted_by: Uuid,
    agent_id: Uuid,
    connector_id: Uuid,
    tool_name: &str,
    context_id: &str,
) -> Result<bool> {
    let found: Option<i32> = sqlx::query_scalar(
        r#"
        SELECT 1 FROM mcp_session_tool_grants
         WHERE granted_by = $1
           AND agent_id = $2 AND connector_id = $3 AND tool_name = $4 AND context_id = $5
           AND expires_at > now()
         LIMIT 1
        "#,
    )
    .bind(granted_by)
    .bind(agent_id)
    .bind(connector_id)
    .bind(tool_name)
    .bind(context_id)
    .fetch_optional(db)
    .await?;
    Ok(found.is_some())
}

/// Deletes every `mcp_session_tool_grants` row past its own `expires_at` — this table had no
/// periodic sweep at all (found in review), unlike `hitl_requests`'s own `expire_stale`.
/// `create_session_grant` is deliberately non-idempotent (its own doc comment: a retried approval
/// can create a second grant for the same tuple), so without this the table only ever grows.
/// Called from the resume dispatcher's existing recovery tick (`crate::dispatcher::run`) — no new
/// timer, same cadence `recover_stuck_resumes` already runs on. Returns the number of rows
/// deleted, for the dispatcher's own logging.
pub async fn sweep_expired_session_grants(db: &PgPool) -> Result<u64> {
    let result = sqlx::query("DELETE FROM mcp_session_tool_grants WHERE expires_at < now()")
        .execute(db)
        .await?;
    Ok(result.rows_affected())
}

/// Default lease/staleness window for a resume-dispatcher claim before it is
/// considered abandoned. Exposed so callers (the dispatcher's recovery sweep) don't need to
/// hardcode the same number twice.
///
/// Not `0007_hitl.sql`'s illustrative 2 minutes any more: `dispatch_one` holds its claim across
/// every in-process retry, so a whole delivery can legitimately run ~15 minutes on the defaults and
/// a 2-minute lease let the recovery sweep quarantine one still in flight (found in review). See
/// `DispatcherConfig::effective_lease_minutes`, which derives the real floor and would otherwise
/// have to override this on every default-constructed config.
pub const DEFAULT_RESUME_LEASE_MINUTES: i64 = 16;

/// Atomically claim exactly one row whose resolved decision has never been
/// pushed anywhere yet, for the resume dispatcher (`crate::dispatcher`).
///
/// Deliberately claims only *virgin* rows (`resume_claimed_at IS NULL`) —
/// unlike `build_jobs.picked_at`, an expired lease here is never silently
/// reclaimed for another attempt. A resume push is not naturally idempotent
/// the way a rebuild is: retrying a call whose outcome is unknown risks
/// delivering the same "retry `<tool>`" nudge twice. So once a row is
/// claimed, it is claimed for good — `finish_resume` records its one
/// definitive outcome, and a claim that never reaches `finish_resume` (the
/// dispatcher process died mid-attempt) is later quarantined by
/// `recover_stuck_resumes` as `delivery_outcome_unknown`, not retried.
///
/// Concurrency-safe via `FOR UPDATE SKIP LOCKED` in the inner subquery: two
/// concurrent callers racing this same query can never claim the same row,
/// mirroring `build_worker::claim_next_job`'s own claim pattern.
/// Scoped to `origin = 'mcp_tool'` — this dispatcher's `RuntimeResumeNotifier` sends a
/// standalone nudge message, not a resumed A2A task, so it's only correct for MCP-originated
/// rows. `direct_chat`/`agent_proxy` rows have their own dispatcher
/// (`oss/server/src/hitl/mod.rs`) with a different delivery mechanism (a true A2A task resume,
/// which requires a `task_id` this origin never has) — without this filter, the two dispatchers
/// would race to claim each other's rows and fail whichever they won incorrectly.
pub async fn claim_for_resume(db: &PgPool) -> Result<Option<HitlRequest>> {
    let row = sqlx::query_as::<_, HitlRequestRow>(
        r#"
        UPDATE hitl_requests
           SET resume_claimed_at = now()
         WHERE id = (
             SELECT id FROM hitl_requests
              -- `rejected` included alongside `resolved`: `build_resume_message`'s
              -- `ToolApproval` branch has a dedicated "denied" message for exactly this case
              -- (found dead — never reachable — until this was widened; a human's reject
              -- decision must still reach the paused agent, just with "do not retry" instead
              -- of "you may retry").
              WHERE status IN ('resolved', 'rejected') AND resume_status = 'not_started'
                AND resume_claimed_at IS NULL
                AND origin = 'mcp_tool'
              ORDER BY created_at
              FOR UPDATE SKIP LOCKED
              LIMIT 1
         )
        RETURNING *
        "#,
    )
    .fetch_optional(db)
    .await?;
    row.map(HitlRequest::try_from).transpose()
}

/// Record the definitive outcome of a claimed row's resume attempt(s) —
/// called exactly once per claim, after the dispatcher's own in-process retry
/// loop (see `crate::dispatcher::dispatch_one`) either confirms delivery or
/// exhausts its retries. `attempts` is the total number of outbound pushes
/// actually made for this claim, recorded for observability even though the
/// claim itself only ever happens once.
///
/// The `WHERE resume_status = 'not_started'` guard means a row already
/// quarantined by `recover_stuck_resumes` (because its lease looked
/// abandoned) can never be clobbered back to `completed`/`failed` by a
/// late-finishing zombie attempt — the quarantine wins. Returns `Ok(None)`
/// in that case, not an error.
pub async fn finish_resume(
    db: &PgPool,
    id: Uuid,
    resume_status: ResumeStatus,
    attempts: i32,
    last_error: Option<&str>,
) -> Result<Option<HitlRequest>> {
    let row = sqlx::query_as::<_, HitlRequestRow>(
        r#"
        UPDATE hitl_requests
           SET resume_status = $2, resume_dispatch_attempts = $3, resume_last_error = $4
         WHERE id = $1 AND resume_status = 'not_started' AND resume_claimed_at IS NOT NULL
        RETURNING *
        "#,
    )
    .bind(id)
    .bind(resume_status.as_str())
    .bind(attempts)
    .bind(last_error)
    .fetch_optional(db)
    .await?;
    row.map(HitlRequest::try_from).transpose()
}

/// Marks an `mcp_tool`-origin row's resume as deliberately never attempted — for
/// the case where resolving it also auto-resolved a linked `direct_chat`/`agent_proxy`/
/// `maf`/`orchestrator` mirror row (`find_linked_direct_chat_row` + `auto_resolve_linked_direct_chat_row`,
/// `oss/server/src/router/hitl.rs`). That mirror has its own real `task_id` and
/// its own dispatcher (`oss/server/src/hitl/mod.rs`), which correctly resumes the
/// paused A2A task; the `mcp_tool` row's own delivery is not just redundant in
/// that case but actively harmful — `RuntimeResumeNotifier` has no `task_id` to
/// resume against (this origin never carries one), so its nudge starts the agent
/// on a brand-new, context-free task instead. Left standalone, `claim_for_resume`
/// would pick this row up and fire that nudge anyway, racing the mirror's own
/// correct resume — confirmed live: the agent received both, the fresh nudge
/// re-asked its own clarifying question with no memory of the original one, and
/// every retry re-triggered exactly the same fork, forever.
///
/// `ResumeStatus::Skipped`, not `Completed` — no delivery attempt is ever made for this
/// row, so recording it as `completed` would misreport a successful delivery that never
/// happened (found in review).
///
/// Sets both `resume_claimed_at` and `resume_status` in one step (rather than
/// composing `claim_for_resume` + `finish_resume`) so `claim_for_resume`'s own
/// `resume_status = 'not_started' AND resume_claimed_at IS NULL` filter can never
/// select this row for real delivery — no separate claim step to race against.
pub async fn skip_resume_for_mirrored_row(db: &PgPool, id: Uuid) -> Result<()> {
    sqlx::query(
        r#"
        UPDATE hitl_requests
           SET resume_status = $2, resume_claimed_at = now()
         WHERE id = $1 AND resume_status = 'not_started' AND resume_claimed_at IS NULL
        "#,
    )
    .bind(id)
    .bind(ResumeStatus::Skipped.as_str())
    .execute(db)
    .await?;
    Ok(())
}

/// Quarantine claims whose lease looks abandoned: claimed
/// (`resume_claimed_at` set) more than `lease_minutes` ago, but still
/// `resume_status = 'not_started'` — meaning `finish_resume` was never
/// called, most likely because the dispatcher process that claimed it died
/// mid-attempt. Transitions those rows to `delivery_outcome_unknown`, which
/// `claim_for_resume` can never select (it only claims `not_started` rows)
/// and which a human must investigate — per `ResumeStatus`'s own doc
/// comment, this state is "never auto-retried". Returns the number of rows
/// quarantined, for the dispatcher's own logging.
///
/// Scoped to `origin = 'mcp_tool'`, matching `claim_for_resume` above exactly — this sweep exists
/// to catch a claim made by THIS dispatcher (a standalone nudge, no `task_id`) that never
/// finished, and `resume_claimed_at`/`resume_status` are columns every origin shares on the same
/// `hitl_requests` table. `direct_chat`/`agent_proxy`/`orchestrator` rows are claimed by the
/// OTHER dispatcher (`oss/server/src/hitl/mod.rs`) with its own multi-minute lease for a real A2A
/// round-trip; at the default `lease_minutes = 2`, an unscoped sweep here would quarantine that
/// dispatcher's legitimately in-flight claims out from under it mid-delivery.
///
/// `status IN ('resolved', 'rejected')`, not just `'resolved'` — `claim_for_resume` above claims
/// both (a reject still needs its "denied" nudge delivered). A `rejected` row whose dispatcher
/// died after claiming was previously invisible to this sweep (`status = 'resolved'` never
/// matched) and to `claim_for_resume` (blocked by its own non-NULL `resume_claimed_at`) alike —
/// stuck forever, with the agent never told it was denied.
pub async fn recover_stuck_resumes(db: &PgPool, lease_minutes: i64) -> Result<u64> {
    let result = sqlx::query(
        r#"
        UPDATE hitl_requests
           SET resume_status = 'delivery_outcome_unknown'
         WHERE status IN ('resolved', 'rejected') AND resume_status = 'not_started'
           AND resume_claimed_at IS NOT NULL
           AND resume_claimed_at < now() - make_interval(mins => $1::int)
           AND origin = 'mcp_tool'
        "#,
    )
    .bind(lease_minutes as i32)
    .execute(db)
    .await?;
    Ok(result.rows_affected())
}

/// Manually reset one `delivery_outcome_unknown` row back to claimable — the operator remediation
/// path `recover_stuck_resumes`'s own doc comment says doesn't exist: that sweep quarantines an
/// abandoned claim into `delivery_outcome_unknown` precisely because a resume push isn't naturally
/// idempotent, so `claim_for_resume` never re-selects it on its own (found in review — a process
/// restart mid-delivery otherwise permanently loses the human's answer, with no requeue, no admin
/// endpoint, and no way back short of a direct SQL edit).
///
/// Deliberately requires an explicit human decision per row (never automatic, never bulk) — the
/// caller is asserting "I've confirmed the original nudge either never reached the agent or is
/// safe to repeat," the same judgment call `recover_stuck_resumes`'s own doc comment says only a
/// human can make. Resets both `resume_status` and `resume_claimed_at` so `claim_for_resume`'s
/// `resume_status = 'not_started' AND resume_claimed_at IS NULL` filter can select it again, and
/// clears `resume_last_error`/`resume_dispatch_attempts` so a subsequent `finish_resume` starts a
/// fresh attempt count rather than accumulating across the two lifetimes. Scoped to
/// `resume_status = 'delivery_outcome_unknown'` so it can never touch a row still legitimately
/// claimed and in flight, or one already `completed`/`failed`.
pub async fn requeue_resume(db: &PgPool, id: Uuid) -> Result<Option<HitlRequest>> {
    let row = sqlx::query_as::<_, HitlRequestRow>(
        r#"
        UPDATE hitl_requests
           SET resume_status = 'not_started', resume_claimed_at = NULL,
               resume_dispatch_attempts = 0, resume_last_error = NULL
         WHERE id = $1 AND resume_status = 'delivery_outcome_unknown'
        RETURNING *
        "#,
    )
    .bind(id)
    .fetch_optional(db)
    .await?;
    row.map(HitlRequest::try_from).transpose()
}

#[cfg(test)]
mod tests {
    use super::*;

    // Row-hydration tests (valid rows, unknown kind/status) now live in
    // `store.rs::row_hydration_tests`, alongside the `HitlRequestRow`/`TryFrom` impl they test —
    // both were duplicated here before the two were consolidated. `authorize_hitl_action`'s own
    // test was removed along with the function itself (dead code, see its doc comment).

    #[test]
    fn resolve_decision_maps_to_the_expected_terminal_status() {
        assert_eq!(
            ResolveDecision::Approve.target_status(),
            HitlStatus::Resolved
        );
        assert_eq!(
            ResolveDecision::Reject.target_status(),
            HitlStatus::Rejected
        );
    }
}

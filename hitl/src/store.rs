use std::collections::HashMap;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

use crate::authz::HitlIdentity;
use crate::types::{HitlKind, HitlOrigin, HitlRequest, HitlStatus, NewHitlRequest};

#[derive(Debug, thiserror::Error)]
pub enum HitlError {
    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),
    #[error("row {0} vanished immediately after being written")]
    NotFound(Uuid),
    #[error("corrupt hitl_requests row: {0}")]
    CorruptRow(String),
}

/// The two outcomes `HitlStore::resolve` can produce — both are a *success* from the caller's
/// point of view (§5: a lost double-resolve race is an idempotent 200, never a 500 or a second
/// resume dispatch), so this is not a `Result` variant.
#[derive(Debug, Clone)]
pub enum ResolveOutcome {
    /// This call's `UPDATE` was the one that recorded the human's decision.
    Applied(HitlRequest),
    /// Someone else already decided this row first; here is its current state.
    AlreadyDecided(HitlRequest),
}

/// Whether a resume-delivery failure is worth retrying, or should be recorded as terminally
/// `failed` right now. Replaces a bare `max_attempts: i32` parameter to `HitlStore::mark_resume_failed`
/// that every call site set to one of exactly two values — `0` (immediately terminal) or
/// `MAX_RESUME_ATTEMPTS` (retryable under the cap) — with nothing naming either meaning at the
/// call site itself (found in review).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureKind {
    /// Fail this row right now, regardless of `resume_dispatch_attempts` — nothing about retrying
    /// would ever change the outcome (e.g. the row itself is malformed).
    Permanent,
    /// Retryable up to `max_attempts` total dispatch attempts — recorded as terminal `failed` only
    /// once `resume_dispatch_attempts` reaches this cap.
    Retryable { max_attempts: i32 },
}

impl FailureKind {
    /// The `resume_dispatch_attempts` threshold at or above which `mark_resume_failed` flips
    /// `resume_status` to `failed` — `0` for `Permanent` (any attempt count is already `>= 0`).
    fn threshold(self) -> i32 {
        match self {
            FailureKind::Permanent => 0,
            FailureKind::Retryable { max_attempts } => max_attempts,
        }
    }
}

/// Persistence + lifecycle operations for `hitl_requests` (§2/§3/§4/§5 of the HITL plan).
/// Authorization is a separate concern — see [`crate::authorize_hitl_action`] — deliberately not
/// a method on this trait, so a caller can never accidentally list/fetch without it (callers are
/// expected to filter via `list_pending_for`'s built-in scoping, and to call
/// `authorize_hitl_action` themselves for single-row `get`/`resolve` accesses).
#[async_trait]
pub trait HitlStore: Send + Sync {
    async fn create(&self, req: NewHitlRequest) -> Result<HitlRequest, HitlError>;
    async fn get(&self, id: Uuid) -> Result<Option<HitlRequest>, HitlError>;
    /// Filtered **inside the query** (§10) — never a post-fetch filter a future call site could
    /// forget to apply.
    async fn list_pending_for(
        &self,
        identity: &HitlIdentity,
    ) -> Result<Vec<HitlRequest>, HitlError>;
    /// Every HITL request (pending or already resolved/rejected/expired/canceled) tied to a web
    /// chat session, oldest first — the session-load discovery path (`chat/routes.rs::
    /// list_messages`), an alternative to `list_pending_for` for a caller that already knows
    /// which session it wants rather than "everything pending for this user". Filtered inside the
    /// query by BOTH `chat_session_id` and `owner_user_id`, same rule as `list_pending_for` (§10)
    /// — a session id alone is never sufficient to authorize the read.
    async fn list_for_chat_session(
        &self,
        chat_session_id: &str,
        owner_user_id: Uuid,
    ) -> Result<Vec<HitlRequest>, HitlError>;
    /// Every HITL request (pending or already resolved/rejected/expired/canceled) tied to a MAF
    /// execution, oldest first — the discovery path for `GET /api/maf/execution/{id}`, so the
    /// frontend never has to call `list_pending_for`/`GET /api/hitl/pending` to correlate a
    /// paused step back to its `hitl_requests.id`. `owner_user_id` is required and must be the
    /// SAME value the caller already validated against `maf_executions.user_id` — this method
    /// does not itself know whether the caller owns the execution, it only refuses to leak a
    /// different owner's rows for the same execution id.
    async fn list_for_maf_execution(
        &self,
        maf_execution_id: Uuid,
        owner_user_id: Uuid,
    ) -> Result<Vec<HitlRequest>, HitlError>;
    /// The same rows as [`HitlStore::list_for_maf_execution`], for a whole page of executions at
    /// once, keyed by `maf_execution_id` — the discovery path for the two execution *list*
    /// endpoints, which would otherwise issue one query per row. `owner_user_id` carries the same
    /// requirement as the single-execution method: it must be the value the caller already
    /// validated against `maf_executions.user_id`.
    ///
    /// The default implementation loops, which is correct but issues one query per id; a real
    /// store overrides it with a single batched query.
    async fn list_for_maf_executions(
        &self,
        maf_execution_ids: &[Uuid],
        owner_user_id: Uuid,
    ) -> Result<HashMap<Uuid, Vec<HitlRequest>>, HitlError> {
        let mut out: HashMap<Uuid, Vec<HitlRequest>> = HashMap::new();
        for id in maf_execution_ids {
            let rows = self.list_for_maf_execution(*id, owner_user_id).await?;
            if !rows.is_empty() {
                out.insert(*id, rows);
            }
        }
        Ok(out)
    }
    /// The exact `UPDATE ... WHERE status = 'pending' RETURNING *` from §5. `status` is the
    /// human's decision (`Resolved` or, for future `tool_approval` rejects, `Rejected`).
    async fn resolve(
        &self,
        id: Uuid,
        human_response: Value,
        resolved_by: Uuid,
        status: HitlStatus,
    ) -> Result<ResolveOutcome, HitlError>;
    /// The exact claim from §3.2: atomically leases one `resolved`/`not_started` row whose lease
    /// (if any) is older than `lease_secs`. Returns `None` when nothing is claimable.
    async fn claim_for_resume(&self, lease_secs: i64) -> Result<Option<HitlRequest>, HitlError>;
    /// Delivery succeeded — a response was received and classified, regardless of the agent's own
    /// business outcome (§3.2: "peer confirmed receipt").
    async fn mark_resume_completed(&self, id: Uuid) -> Result<(), HitlError>;
    /// Delivery failed before or during the attempt. Releases the lease for a retry unless
    /// `resume_dispatch_attempts` has reached `failure`'s threshold, in which case `resume_status`
    /// becomes the terminal `failed`.
    async fn mark_resume_failed(
        &self,
        id: Uuid,
        error: &str,
        failure: FailureKind,
    ) -> Result<(), HitlError>;
    /// Flips every `pending` row whose `expires_at` has passed to `expired`. Called once per
    /// dispatcher poll tick (`hitl/mod.rs::run`) — cheap, since `idx_hitl_pending_owner` already
    /// covers `status = 'pending'`. Returns the number of rows expired, for logging.
    async fn expire_stale(&self) -> Result<u64, HitlError>;
    /// Same shape as `resolve` (idempotent — cancelling an already-cancelled row is
    /// `AlreadyDecided`, not an error), but sets `status = 'canceled'` and leaves
    /// `human_response` null: the owner is withdrawing the request, not answering it.
    async fn cancel(&self, id: Uuid, canceled_by: Uuid) -> Result<ResolveOutcome, HitlError>;
    /// A row that survived past `claim_for_resume`'s reach without a clean `completed`/`failed`
    /// outcome — i.e. every attempt up to the cap crashed the dispatcher process mid-delivery
    /// rather than failing cleanly (a clean HTTP/parse failure already self-terminates via
    /// `mark_resume_failed`'s own cap check). Sets `resume_status = 'delivery_outcome_unknown'`
    /// and releases the lease so it stops being reclaimed.
    async fn mark_resume_unknown(&self, id: Uuid) -> Result<(), HitlError>;
    /// Records that the human clicked "start authorization" (§7/Phase 4) — an in-place,
    /// non-terminal annotation on `human_response.auth_outcome`, deliberately **not** a call to
    /// `resolve()`: the row stays `pending` (the human hasn't actually completed the external
    /// auth step yet, only begun it), so no resume is triggered. Idempotent — calling it again
    /// before `confirm` just re-writes the same marker. Only matches a still-`pending`
    /// `auth_required` row; `Ok(None)` means the row was already resolved/expired/canceled or was
    /// never `auth_required`, so the caller should fall back to reporting its current state.
    async fn record_auth_start(&self, id: Uuid) -> Result<Option<HitlRequest>, HitlError>;
}

/// Resolves the row that should actually be shown to a human through a **discovery** surface —
/// Direct Chat's live SSE stream frame, the session-load `hitl` array, or a MAF execution's own
/// `hitl` array (`maf.rs::hitl_rows_for_execution`) — for a row that's about to be surfaced
/// there. Never used by the direct `GET /api/hitl/{id}` / `resolve` / `cancel` / per-id `stream`
/// endpoints, which always operate on the exact row the caller already named by id; this is
/// purely a display-layer substitution for the surfaces that decide *which* row to show in the
/// first place.
///
/// Background: an agent that maps an MCP tool block onto its own A2A `AUTH_REQUIRED` task state
/// causes two rows to exist for one real event — the real `mcp_tool` row (created by the MCP
/// gateway, carries no `task_id`) and a `direct_chat`/`agent_proxy`/`maf`/`orchestrator` mirror of
/// it (created by `persist_direct_chat_pause`, MAF's own equivalent pause-persist step, or the
/// orchestrator's own sub-agent pause persist step, carries the
/// `task_id`/`context_id`/`chat_session_id`/`maf_execution_id` the resume mechanism needs to
/// continue that specific paused step — see `NewHitlRequest::mcp_tool`'s doc comment). A
/// sub-agent the orchestrator delegates to can use MCP tools just as a direct-chat agent can, so
/// this mirroring is not limited to direct chat/MAF. A discovery surface only ever sees the
/// mirror, since it's the only row with that correlation data; left unpatched it would show the
/// mirror's own generic question, and a human resolving that id would resume the chat
/// task/MAF step/orchestrator turn without ever touching the real MCP permission — the tool would
/// stay blocked.
///
/// The agent links the two by putting `hitl_request_id: <the mcp row's id>` in its own pause
/// metadata, which lands verbatim in the mirror's `question.metadata.hitl_request_id`
/// (`build_pause_question` in `oss/types/src/a2a.rs` forwards the agent's status-message metadata
/// through unmodified) — **fully agent-controlled**, not something the platform itself ever
/// writes. This returns `row.clone()` unchanged unless ALL of the following hold: `row.origin` is
/// `direct_chat`/`agent_proxy`/`maf`/`orchestrator`, `question.metadata.hitl_request_id` is
/// present and parses as a `Uuid`, that id resolves to a real row via `store.get()`, AND that
/// row is a genuine link target — `owner_user_id` matches `row`'s own (the mirror's owner is who
/// this substitution is being computed for — see `caller_owner_id` below), `origin` is
/// `mcp_tool` (the only kind of row this mirroring exists to surface — anything else is not a
/// real `mcp_tool`/mirror pair, just an agent-supplied id), `status` is still `pending` (a
/// resolved/rejected/expired/canceled row is stale — substituting it would show the human a
/// question for something already decided), and `agent_id` matches `row`'s own (the two halves of
/// one real pause always belong to the same agent; a cross-agent link can only be a forged or
/// stale one). Any failure at any step — no link, a malformed id, a stale/nonexistent id, a
/// lookup error, or any of the checks above — falls back to the mirror as-is: a broken (or
/// hostile) link must never turn into a broken or missing HITL prompt for the human. Without the
/// `owner_user_id` check, a buggy or malicious agent could point `hitl_request_id` at any other
/// user's row and have that row's real `question` (which can carry tool arguments, connector
/// names, auth URLs) substituted into this requester's view — every caller of this function
/// already scopes `row` itself to the right owner (`list_for_chat_session`/
/// `list_for_maf_execution`/the live stream's own row), so `caller_owner_id` is always
/// `row.owner_user_id` in practice, never a value the caller invents. Without the `origin`/
/// `status`/`agent_id` checks, an agent could still point the link at any OTHER pending or
/// already-resolved row this same user owns — including one from a different agent entirely —
/// and have its `id`/`kind`/`question` rendered into this session's panel.
///
/// Only `id`/`kind`/`question` come from the linked row; every other field — crucially
/// `task_id`/`context_id`/`chat_session_id` — stays the mirror's own, since those are what the
/// frontend needs to correlate the prompt back to the visible chat task/session.
pub async fn resolve_display_row(
    store: &dyn HitlStore,
    row: &HitlRequest,
    caller_owner_id: Uuid,
) -> HitlRequest {
    if !matches!(
        row.origin,
        HitlOrigin::DirectChat
            | HitlOrigin::AgentProxy
            | HitlOrigin::Maf
            | HitlOrigin::Orchestrator
    ) {
        return row.clone();
    }
    let Some(linked_id) = row
        .question
        .get("metadata")
        .and_then(|m| m.get("hitl_request_id"))
        .and_then(|v| v.as_str())
        .and_then(|s| Uuid::parse_str(s).ok())
    else {
        return row.clone();
    };
    match store.get(linked_id).await {
        Ok(Some(linked)) if is_valid_mcp_mirror_link(&linked, caller_owner_id, row.agent_id) => {
            HitlRequest {
                id: linked.id,
                kind: linked.kind,
                question: linked.question,
                ..row.clone()
            }
        }
        _ => row.clone(),
    }
}

/// Whether `linked` — a row fetched by an agent-controlled `hitl_request_id` pointer embedded in
/// someone else's `question` — is safe to treat as the real `mcp_tool` pause that pointer claims
/// to identify. `question`/its `metadata` is an untrusted A2A response echoed straight from the
/// agent, so an agent can stamp *any* UUID there; without this check, that UUID would resolve
/// straight to another user's row (the #383 mirror-hijack family). All four conditions are
/// load-bearing: owner scopes it to the same human, `origin == McpTool` and `status == Pending`
/// confirm it's genuinely the live MCP-gateway pause (not some other, already-settled, or
/// wrong-kind row that merely shares an id), and `agent_id` ties it to the same deployment that
/// raised `row`. Shared by every site that dereferences this pointer — currently
/// [`resolve_display_row`] (UI display) and `oss/server/src/hitl/mod.rs::deliver`'s continuation-
/// buffer aliasing — so there is exactly one place this predicate can drift from correct.
pub fn is_valid_mcp_mirror_link(linked: &HitlRequest, owner_user_id: Uuid, agent_id: Uuid) -> bool {
    linked.owner_user_id == owner_user_id
        && linked.origin == HitlOrigin::McpTool
        && linked.status == HitlStatus::Pending
        && linked.agent_id == agent_id
}

/// Mirrors the `hitl_requests` table with plain column types (`String` for the four CHECK-backed
/// enum columns) rather than deriving `sqlx::FromRow` directly on [`HitlRequest`] — this crate has
/// no custom `sqlx::Type`/`Decode` impls for its enums (the wire/DB format is `TEXT`, not a native
/// Postgres enum type), so decoding through `FromStr` here is both simpler and keeps a corrupt
/// value a typed error instead of a decode panic. `pub(crate)` (not private): `repo.rs` shares
/// this exact row shape and the `TryFrom` impl below rather than maintaining its own field-for-
/// field-identical copy, which is what this crate had until the two were consolidated.
#[derive(sqlx::FromRow)]
pub(crate) struct HitlRequestRow {
    id: Uuid,
    kind: String,
    origin: String,
    status: String,
    resume_status: String,
    agent_id: Uuid,
    owner_user_id: Uuid,
    resolved_by: Option<Uuid>,
    task_id: Option<String>,
    context_id: Option<String>,
    chat_session_id: Option<String>,
    maf_execution_id: Option<Uuid>,
    maf_step_index: Option<i32>,
    connector_id: Option<Uuid>,
    tool_name: Option<String>,
    arguments_hash: Option<String>,
    consumed_at: Option<DateTime<Utc>>,
    question: Value,
    human_response: Option<Value>,
    resume_state: Value,
    resume_claimed_at: Option<DateTime<Utc>>,
    resume_dispatch_attempts: i32,
    resume_last_error: Option<String>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    expires_at: Option<DateTime<Utc>>,
    resolved_at: Option<DateTime<Utc>>,
}

impl TryFrom<HitlRequestRow> for HitlRequest {
    type Error = HitlError;

    fn try_from(row: HitlRequestRow) -> Result<Self, HitlError> {
        fn corrupt(column: &str, value: &str) -> HitlError {
            HitlError::CorruptRow(format!(
                "{column} column holds unrecognized value {value:?}"
            ))
        }
        Ok(HitlRequest {
            id: row.id,
            kind: row.kind.parse().map_err(|_| corrupt("kind", &row.kind))?,
            origin: row
                .origin
                .parse()
                .map_err(|_| corrupt("origin", &row.origin))?,
            status: row
                .status
                .parse()
                .map_err(|_| corrupt("status", &row.status))?,
            resume_status: row
                .resume_status
                .parse()
                .map_err(|_| corrupt("resume_status", &row.resume_status))?,
            agent_id: row.agent_id,
            owner_user_id: row.owner_user_id,
            resolved_by: row.resolved_by,
            task_id: row.task_id,
            context_id: row.context_id,
            chat_session_id: row.chat_session_id,
            maf_execution_id: row.maf_execution_id,
            maf_step_index: row.maf_step_index,
            connector_id: row.connector_id,
            tool_name: row.tool_name,
            arguments_hash: row.arguments_hash,
            consumed_at: row.consumed_at,
            question: row.question,
            human_response: row.human_response,
            resume_state: row.resume_state,
            resume_claimed_at: row.resume_claimed_at,
            resume_dispatch_attempts: row.resume_dispatch_attempts,
            resume_last_error: row.resume_last_error,
            created_at: row.created_at,
            updated_at: row.updated_at,
            expires_at: row.expires_at,
            resolved_at: row.resolved_at,
        })
    }
}

pub struct PgHitlStore {
    pool: PgPool,
    /// Applied at row-creation time (`create`) as `now() + interval 'N days'`. Configurable so an
    /// operator can tune it without a code change — see `with_ttl_days`.
    ttl_days: i64,
}

impl PgHitlStore {
    /// Defaults to a 7-day TTL. Production code should use [`Self::with_ttl_days`] instead,
    /// sourced from `Config::hitl_request_ttl_days` (env: `HITL_REQUEST_TTL_DAYS`) — this
    /// constructor exists so call sites that don't care about the TTL (tests, mainly) don't have
    /// to thread a value through.
    pub fn new(pool: PgPool) -> Self {
        Self::with_ttl_days(pool, 7)
    }

    pub fn with_ttl_days(pool: PgPool, ttl_days: i64) -> Self {
        Self { pool, ttl_days }
    }

    /// Re-fetch the pending row a unique-violation on `create` must have collided with — either
    /// `uq_hitl_pending_per_task`, `uq_hitl_pending_per_tool_call`, or
    /// `uq_hitl_pending_per_connector_auth` (§5).
    ///
    /// Every branch is scoped by `owner_user_id` in addition to whatever the index's own matching
    /// columns are, matching `uq_hitl_pending_per_task`/`uq_hitl_pending_per_tool_call`/
    /// `uq_hitl_pending_per_connector_auth` themselves (`0025_hitl_task_id_scope.sql`,
    /// `0029_hitl_mcp_pending_owner_scope.sql`) — `task_id` and `context_id` (the McpTool
    /// branches' own matching value, which falls back to a raw trace id when no `session_traces`
    /// mapping exists) are both populated from agent-controlled data, never a Nasiko-minted id, so
    /// neither may be trusted alone as a database-wide key: without this scoping, a non-random or
    /// malicious agent's `taskId`/trace id could collide two different users' pauses onto the same
    /// row, and this recovery path would hand the wrong user's row back as if it were their own.
    ///
    /// The two McpTool branches are also kept kind-specific rather than one query covering both:
    /// `tool_approval` rows match on `tool_name` (part of `uq_hitl_pending_per_tool_call`'s own
    /// key), `auth_required` rows have no `tool_name` at all (`uq_hitl_pending_per_connector_auth`
    /// is a connector-level guard, not a per-tool one) — a single query using one column set for
    /// both would either miss a real `auth_required` collision (falling through to a raw
    /// constraint-violation error instead of returning the existing row) or match too loosely.
    async fn find_existing_pending(
        &self,
        req: &NewHitlRequest,
    ) -> Result<Option<HitlRequest>, HitlError> {
        let row: Option<HitlRequestRow> = match (req.origin, req.kind) {
            (HitlOrigin::McpTool, HitlKind::ToolApproval) => sqlx::query_as(
                "SELECT * FROM hitl_requests
                 WHERE status = 'pending' AND kind = 'tool_approval'
                   AND owner_user_id = $1 AND agent_id = $2 AND connector_id = $3
                   AND tool_name = $4 AND context_id = $5",
            )
            .bind(req.owner_user_id)
            .bind(req.agent_id)
            .bind(req.connector_id)
            .bind(&req.tool_name)
            .bind(&req.context_id)
            .fetch_optional(&self.pool)
            .await?,
            (HitlOrigin::McpTool, _) => sqlx::query_as(
                "SELECT * FROM hitl_requests
                 WHERE status = 'pending' AND kind = 'auth_required' AND origin = 'mcp_tool'
                   AND owner_user_id = $1 AND agent_id = $2 AND connector_id = $3
                   AND context_id = $4",
            )
            .bind(req.owner_user_id)
            .bind(req.agent_id)
            .bind(req.connector_id)
            .bind(&req.context_id)
            .fetch_optional(&self.pool)
            .await?,
            _ => sqlx::query_as(
                "SELECT * FROM hitl_requests
                 WHERE status = 'pending' AND owner_user_id = $1 AND agent_id = $2 AND task_id = $3",
            )
            .bind(req.owner_user_id)
            .bind(req.agent_id)
            .bind(&req.task_id)
            .fetch_optional(&self.pool)
            .await?,
        };
        row.map(HitlRequest::try_from).transpose()
    }
}

#[async_trait]
impl HitlStore for PgHitlStore {
    async fn create(&self, req: NewHitlRequest) -> Result<HitlRequest, HitlError> {
        // Configurable via `HITL_REQUEST_TTL_DAYS` (`Config::hitl_request_ttl_days`, 7-day
        // default per the Implementation Plan §9) — a store-level policy, not something any
        // origin constructor needs to know about.
        let expires_at = Utc::now() + chrono::Duration::days(self.ttl_days);
        let result: Result<HitlRequestRow, sqlx::Error> = sqlx::query_as(
            "INSERT INTO hitl_requests
                (kind, origin, agent_id, owner_user_id, task_id, context_id, chat_session_id,
                 maf_execution_id, maf_step_index, connector_id, tool_name, arguments_hash, question,
                 expires_at)
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14)
             RETURNING *",
        )
        .bind(req.kind.as_str())
        .bind(req.origin.as_str())
        .bind(req.agent_id)
        .bind(req.owner_user_id)
        .bind(&req.task_id)
        .bind(&req.context_id)
        .bind(&req.chat_session_id)
        .bind(req.maf_execution_id)
        .bind(req.maf_step_index)
        .bind(req.connector_id)
        .bind(&req.tool_name)
        .bind(&req.arguments_hash)
        .bind(&req.question)
        .bind(expires_at)
        .fetch_one(&self.pool)
        .await;

        match result {
            Ok(row) => row.try_into(),
            Err(sqlx::Error::Database(db_err)) if db_err.is_unique_violation() => {
                match self.find_existing_pending(&req).await? {
                    Some(existing) => Ok(existing),
                    None => Err(HitlError::Db(sqlx::Error::Database(db_err))),
                }
            }
            Err(e) => Err(e.into()),
        }
    }

    async fn get(&self, id: Uuid) -> Result<Option<HitlRequest>, HitlError> {
        let row: Option<HitlRequestRow> =
            sqlx::query_as("SELECT * FROM hitl_requests WHERE id = $1")
                .bind(id)
                .fetch_optional(&self.pool)
                .await?;
        row.map(HitlRequest::try_from).transpose()
    }

    async fn list_pending_for(
        &self,
        identity: &HitlIdentity,
    ) -> Result<Vec<HitlRequest>, HitlError> {
        // Excludes a `direct_chat`/`agent_proxy`/`maf`/`orchestrator` row that only mirrors a
        // still-pending `mcp_tool` row (an agent — including one the orchestrator delegated to —
        // that maps MCP's `ask_required` onto the A2A `AUTH_REQUIRED` task state — see
        // `NewHitlRequest::mcp_tool`'s doc comment and `auto_resolve_linked_direct_chat_row` in
        // `router/hitl.rs`). Resolving the mirror directly triggers a real (but premature) resume
        // without granting the actual MCP permission, so it must never be offered as its own
        // actionable pending item — only the linked `mcp_tool` row is the one that does real
        // work. The regex guards the `::uuid` cast: `hitl_request_id` is caller-supplied agent
        // metadata, so a malformed value must not error the whole listing, just fail to match.
        // The `linked.*` conditions here must match `resolve_display_row`'s own definition of a
        // genuine link exactly — otherwise a row this filter treats as "has a real mirror target"
        // could be one `resolve_display_row` would reject (or vice versa), and the two surfaces
        // disagree on whether the same row is a real mirror. In particular `linked.owner_user_id`
        // is NOT implicitly `h.owner_user_id` — `hitl_request_id` is agent-controlled metadata on
        // `h`, not a value scoped to `h`'s own owner — so without this check an agent could stamp
        // any OTHER user's real pending `mcp_tool` row's id into its own pause's metadata and make
        // its own genuine pause (`h`, already scoped to the correct owner by the caller's WHERE
        // clause) match this EXISTS and vanish from that owner's `/api/hitl` listing entirely, with
        // no dispatch and no visible error — silently stuck until the 7-day TTL expires it.
        const MIRROR_FILTER: &str = "
            AND NOT (
                h.origin IN ('direct_chat', 'agent_proxy', 'maf', 'orchestrator')
                AND h.question->'metadata'->>'hitl_request_id' ~* '^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$'
                AND EXISTS (
                    SELECT 1 FROM hitl_requests linked
                     WHERE linked.id = (h.question->'metadata'->>'hitl_request_id')::uuid
                       AND linked.status = 'pending'
                       AND linked.owner_user_id = h.owner_user_id
                       AND linked.origin = 'mcp_tool'
                       AND linked.agent_id = h.agent_id
                )
            )";
        let rows: Vec<HitlRequestRow> = if identity.is_superuser {
            sqlx::query_as(&format!(
                "SELECT h.* FROM hitl_requests h WHERE h.status = 'pending' {MIRROR_FILTER} ORDER BY h.created_at"
            ))
            .fetch_all(&self.pool)
            .await?
        } else {
            sqlx::query_as(&format!(
                "SELECT h.* FROM hitl_requests h WHERE h.status = 'pending' AND h.owner_user_id = $1 {MIRROR_FILTER} ORDER BY h.created_at"
            ))
            .bind(identity.user_id)
            .fetch_all(&self.pool)
            .await?
        };
        rows.into_iter().map(HitlRequest::try_from).collect()
    }

    async fn list_for_chat_session(
        &self,
        chat_session_id: &str,
        owner_user_id: Uuid,
    ) -> Result<Vec<HitlRequest>, HitlError> {
        // `LIMIT 200`, unlike `chat_messages`'s own paginated fetch alongside this call
        // (`chat/routes.rs::list_messages`) — a long-running session accumulating thousands of
        // resolved HITL rows over time would otherwise make this array grow without bound on
        // every single page load of that session, unlike the messages themselves. 200 is a
        // generous cap for what is, per row, one human decision in one conversation — the inner
        // query takes the most recent 200 by `created_at`, then the outer re-sorts them oldest
        // first to preserve this method's documented ordering.
        //
        // `chat_session_id` (the column) is only ever populated for an `Orchestrator`-origin row
        // — the stable top-level session a sub-agent dispatch was mirrored under; its `context_id`
        // is that sub-agent's own unstable per-dispatch context, never a real session, and must
        // never be treated as one. `AgentProxy`/`DirectChat`-origin rows never set
        // `chat_session_id` at all; for those, `context_id` already IS the stable, caller-facing
        // session id (no separate orchestrator-level session to distinguish it from — same
        // reasoning as `oss/server/src/hitl/mod.rs::stable_session_id`). Matching only the
        // `chat_session_id` column here meant this query returned empty for *every* direct-chat
        // session, no matter how many real HITL rows it had (confirmed live: two resolved
        // `input_required` rows for a real `archive` direct-chat session, zero returned) — fixed
        // by falling back to `context_id` only when `chat_session_id` is absent, mirroring
        // `stable_session_id`'s `unwrap_or` precisely: never both, never neither's absence
        // silently matching the wrong column.
        // `(chat_session_id = $1 OR (chat_session_id IS NULL AND context_id = $1))`, not
        // `COALESCE(chat_session_id, context_id) = $1` — equivalent, but sargable: `COALESCE`
        // over a column defeats any index on either column, forcing `owner_user_id` alone to
        // carry the whole query.
        let rows: Vec<HitlRequestRow> = sqlx::query_as(
            "SELECT * FROM (
                 SELECT * FROM hitl_requests
                  WHERE (chat_session_id = $1 OR (chat_session_id IS NULL AND context_id = $1))
                    AND owner_user_id = $2
                  ORDER BY created_at DESC LIMIT 200
             ) recent ORDER BY created_at ASC",
        )
        .bind(chat_session_id)
        .bind(owner_user_id)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(HitlRequest::try_from).collect()
    }

    async fn list_for_maf_execution(
        &self,
        maf_execution_id: Uuid,
        owner_user_id: Uuid,
    ) -> Result<Vec<HitlRequest>, HitlError> {
        let rows: Vec<HitlRequestRow> = sqlx::query_as(
            "SELECT * FROM hitl_requests WHERE maf_execution_id = $1 AND owner_user_id = $2 ORDER BY created_at",
        )
        .bind(maf_execution_id)
        .bind(owner_user_id)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(HitlRequest::try_from).collect()
    }

    /// One query for the whole page, rather than the trait's default loop. Empty in, empty out —
    /// `= ANY('{}')` would be a pointless round trip for a page of runs that never paused.
    async fn list_for_maf_executions(
        &self,
        maf_execution_ids: &[Uuid],
        owner_user_id: Uuid,
    ) -> Result<HashMap<Uuid, Vec<HitlRequest>>, HitlError> {
        if maf_execution_ids.is_empty() {
            return Ok(HashMap::new());
        }
        let rows: Vec<HitlRequestRow> = sqlx::query_as(
            "SELECT * FROM hitl_requests \
             WHERE maf_execution_id = ANY($1) AND owner_user_id = $2 ORDER BY created_at",
        )
        .bind(maf_execution_ids)
        .bind(owner_user_id)
        .fetch_all(&self.pool)
        .await?;

        let mut out: HashMap<Uuid, Vec<HitlRequest>> = HashMap::new();
        for row in rows {
            let row = HitlRequest::try_from(row)?;
            // `maf_execution_id` is the column just filtered on, so it is never NULL here.
            if let Some(exec_id) = row.maf_execution_id {
                out.entry(exec_id).or_default().push(row);
            }
        }
        Ok(out)
    }

    async fn resolve(
        &self,
        id: Uuid,
        human_response: Value,
        resolved_by: Uuid,
        status: HitlStatus,
    ) -> Result<ResolveOutcome, HitlError> {
        let row: Option<HitlRequestRow> = sqlx::query_as(
            "UPDATE hitl_requests
                SET status = $1, human_response = $2, resolved_by = $3, resolved_at = now()
              WHERE id = $4 AND status = 'pending'
          RETURNING *",
        )
        .bind(status.as_str())
        .bind(&human_response)
        .bind(resolved_by)
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;

        match row {
            Some(row) => Ok(ResolveOutcome::Applied(row.try_into()?)),
            None => {
                let current = self.get(id).await?.ok_or(HitlError::NotFound(id))?;
                Ok(ResolveOutcome::AlreadyDecided(current))
            }
        }
    }

    async fn claim_for_resume(&self, lease_secs: i64) -> Result<Option<HitlRequest>, HitlError> {
        let stale_before = Utc::now() - chrono::Duration::seconds(lease_secs);

        let mut tx = self.pool.begin().await?;

        // Scoped to the four origins this dispatcher actually knows how to deliver — `deliver()`
        // branches on `HitlOrigin::Maf` to `deliver_maf` (an XADD continuation job for the MAF
        // worker), and on `direct_chat`/`agent_proxy`/`orchestrator` to a real A2A task resume via
        // `task_id` (`deliver()` fails outright on anything else). `mcp_tool` rows have no
        // `task_id` at all and are claimed by `oss/hitl`'s own dispatcher instead
        // (`nasiko_hitl::repo::claim_for_resume`, scoped the other way) — without this filter the
        // two dispatchers would race on the same rows and fail whichever they claimed by mistake.
        //
        // `status IN ('resolved', 'rejected')`, not just `'resolved'`: a `tool_approval`/
        // `auth_required` REJECT auto-resolves this row's own mirror to `Rejected` too
        // (`auto_resolve_linked_direct_chat_row`, `router/hitl.rs`) — it still needs to be
        // delivered so the paused agent actually learns the decision (`answer_text` turns
        // `auth_outcome: "denied"` into the resume message), not left hanging forever. Mirrors the
        // identical widening already done on `oss/hitl::repo::claim_for_resume`.
        let row: Option<HitlRequestRow> = sqlx::query_as(
            "SELECT * FROM hitl_requests
              WHERE status IN ('resolved', 'rejected') AND resume_status = 'not_started'
                AND (resume_claimed_at IS NULL OR resume_claimed_at < $1)
                AND origin IN ('direct_chat', 'agent_proxy', 'maf', 'orchestrator')
              ORDER BY resolved_at
              FOR UPDATE SKIP LOCKED
              LIMIT 1",
        )
        .bind(stale_before)
        .fetch_optional(&mut *tx)
        .await?;

        let Some(row) = row else {
            tx.rollback().await?;
            return Ok(None);
        };

        sqlx::query(
            "UPDATE hitl_requests
                SET resume_claimed_at = now(), resume_dispatch_attempts = resume_dispatch_attempts + 1
              WHERE id = $1",
        )
        .bind(row.id)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;

        let mut claimed: HitlRequest = row.try_into()?;
        claimed.resume_dispatch_attempts += 1;
        Ok(Some(claimed))
    }

    async fn mark_resume_completed(&self, id: Uuid) -> Result<(), HitlError> {
        sqlx::query(
            "UPDATE hitl_requests SET resume_status = 'completed', resume_claimed_at = NULL WHERE id = $1",
        )
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn mark_resume_failed(
        &self,
        id: Uuid,
        error: &str,
        failure: FailureKind,
    ) -> Result<(), HitlError> {
        sqlx::query(
            "UPDATE hitl_requests
                SET resume_last_error = $2,
                    resume_claimed_at = NULL,
                    resume_status = CASE WHEN resume_dispatch_attempts >= $3 THEN 'failed' ELSE resume_status END
              WHERE id = $1",
        )
        .bind(id)
        .bind(error)
        .bind(failure.threshold())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn expire_stale(&self) -> Result<u64, HitlError> {
        let result = sqlx::query(
            "UPDATE hitl_requests SET status = 'expired' WHERE status = 'pending' AND expires_at < now()",
        )
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
    }

    async fn cancel(&self, id: Uuid, canceled_by: Uuid) -> Result<ResolveOutcome, HitlError> {
        let row: Option<HitlRequestRow> = sqlx::query_as(
            "UPDATE hitl_requests
                SET status = 'canceled', resolved_by = $1, resolved_at = now()
              WHERE id = $2 AND status = 'pending'
          RETURNING *",
        )
        .bind(canceled_by)
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;

        match row {
            Some(row) => Ok(ResolveOutcome::Applied(row.try_into()?)),
            None => {
                let current = self.get(id).await?.ok_or(HitlError::NotFound(id))?;
                Ok(ResolveOutcome::AlreadyDecided(current))
            }
        }
    }

    async fn mark_resume_unknown(&self, id: Uuid) -> Result<(), HitlError> {
        sqlx::query(
            "UPDATE hitl_requests
                SET resume_status = 'delivery_outcome_unknown', resume_claimed_at = NULL
              WHERE id = $1",
        )
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn record_auth_start(&self, id: Uuid) -> Result<Option<HitlRequest>, HitlError> {
        let row: Option<HitlRequestRow> = sqlx::query_as(
            "UPDATE hitl_requests
                SET human_response = jsonb_set(
                        coalesce(human_response, '{}'::jsonb), '{auth_outcome}', '\"started\"', true)
              WHERE id = $1 AND status = 'pending' AND kind = 'auth_required'
          RETURNING *",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        row.map(HitlRequest::try_from).transpose()
    }
}

#[cfg(test)]
mod row_hydration_tests {
    use super::*;
    use crate::types::{HitlKind, HitlOrigin, HitlStatus, ResumeStatus};

    fn valid_row() -> HitlRequestRow {
        let now = Utc::now();
        HitlRequestRow {
            id: Uuid::new_v4(),
            kind: "auth_required".to_string(),
            origin: "mcp_tool".to_string(),
            status: "pending".to_string(),
            resume_status: "not_started".to_string(),
            agent_id: Uuid::new_v4(),
            owner_user_id: Uuid::new_v4(),
            resolved_by: None,
            task_id: None,
            context_id: Some("ctx-1".to_string()),
            chat_session_id: None,
            maf_execution_id: None,
            maf_step_index: None,
            connector_id: Some(Uuid::new_v4()),
            tool_name: None,
            arguments_hash: None,
            consumed_at: None,
            question: serde_json::json!({"connector": "github"}),
            human_response: None,
            resume_state: serde_json::json!({}),
            resume_claimed_at: None,
            resume_dispatch_attempts: 0,
            resume_last_error: None,
            created_at: now,
            updated_at: now,
            expires_at: Some(now + chrono::Duration::days(7)),
            resolved_at: None,
        }
    }

    fn valid_tool_approval_row() -> HitlRequestRow {
        let mut row = valid_row();
        row.kind = "tool_approval".to_string();
        row.tool_name = Some("GITHUB_CREATE_ISSUE".to_string());
        row
    }

    #[test]
    fn valid_tool_approval_row_hydrates_into_domain_type() {
        let row = valid_tool_approval_row();
        let hitl = HitlRequest::try_from(row).expect("valid row must hydrate");
        assert_eq!(hitl.kind, HitlKind::ToolApproval);
        assert_eq!(hitl.origin, HitlOrigin::McpTool);
        assert_eq!(hitl.tool_name.as_deref(), Some("GITHUB_CREATE_ISSUE"));
        assert!(hitl.connector_id.is_some());
        assert_eq!(hitl.context_id.as_deref(), Some("ctx-1"));
    }

    #[test]
    fn valid_row_hydrates_into_domain_type() {
        let row = valid_row();
        let id = row.id;
        let hitl = HitlRequest::try_from(row).expect("valid row must hydrate");
        assert_eq!(hitl.id, id);
        assert_eq!(hitl.kind, HitlKind::AuthRequired);
        assert_eq!(hitl.origin, HitlOrigin::McpTool);
        assert_eq!(hitl.status, HitlStatus::Pending);
        assert_eq!(hitl.resume_status, ResumeStatus::NotStarted);
        assert_eq!(hitl.context_id.as_deref(), Some("ctx-1"));
    }

    #[test]
    fn unknown_kind_string_is_an_error_not_a_panic() {
        let mut row = valid_row();
        row.kind = "not_a_real_kind".to_string();
        let err = HitlRequest::try_from(row).unwrap_err();
        assert!(matches!(err, HitlError::CorruptRow(_)));
    }

    #[test]
    fn unknown_status_string_is_an_error_not_a_panic() {
        let mut row = valid_row();
        row.status = "not_a_real_status".to_string();
        let err = HitlRequest::try_from(row).unwrap_err();
        assert!(matches!(err, HitlError::CorruptRow(_)));
    }
}

#[cfg(test)]
mod resolve_display_row_tests {
    use super::*;
    use crate::authz::HitlIdentity;
    use crate::types::{HitlKind, HitlStatus, ResumeStatus};
    use serde_json::json;
    use std::collections::HashMap;
    use std::sync::Mutex;

    /// A trivial in-memory `HitlStore` — only `get` is exercised by `resolve_display_row`, so
    /// every other method is unreachable from these tests and left unimplemented rather than
    /// faked out with meaningless behavior.
    #[derive(Default)]
    struct FakeStore(Mutex<HashMap<Uuid, HitlRequest>>);

    #[async_trait]
    impl HitlStore for FakeStore {
        async fn create(&self, _req: NewHitlRequest) -> Result<HitlRequest, HitlError> {
            unimplemented!("not exercised by resolve_display_row")
        }
        async fn get(&self, id: Uuid) -> Result<Option<HitlRequest>, HitlError> {
            Ok(self.0.lock().unwrap().get(&id).cloned())
        }
        async fn list_pending_for(
            &self,
            _identity: &HitlIdentity,
        ) -> Result<Vec<HitlRequest>, HitlError> {
            unimplemented!("not exercised by resolve_display_row")
        }
        async fn list_for_chat_session(
            &self,
            _chat_session_id: &str,
            _owner_user_id: Uuid,
        ) -> Result<Vec<HitlRequest>, HitlError> {
            unimplemented!("not exercised by resolve_display_row")
        }
        async fn list_for_maf_execution(
            &self,
            _maf_execution_id: Uuid,
            _owner_user_id: Uuid,
        ) -> Result<Vec<HitlRequest>, HitlError> {
            unimplemented!("not exercised by resolve_display_row")
        }
        async fn resolve(
            &self,
            _id: Uuid,
            _human_response: Value,
            _resolved_by: Uuid,
            _status: HitlStatus,
        ) -> Result<ResolveOutcome, HitlError> {
            unimplemented!("not exercised by resolve_display_row")
        }
        async fn claim_for_resume(
            &self,
            _lease_secs: i64,
        ) -> Result<Option<HitlRequest>, HitlError> {
            unimplemented!("not exercised by resolve_display_row")
        }
        async fn mark_resume_completed(&self, _id: Uuid) -> Result<(), HitlError> {
            unimplemented!("not exercised by resolve_display_row")
        }
        async fn mark_resume_failed(
            &self,
            _id: Uuid,
            _error: &str,
            _failure: FailureKind,
        ) -> Result<(), HitlError> {
            unimplemented!("not exercised by resolve_display_row")
        }
        async fn expire_stale(&self) -> Result<u64, HitlError> {
            unimplemented!("not exercised by resolve_display_row")
        }
        async fn cancel(&self, _id: Uuid, _canceled_by: Uuid) -> Result<ResolveOutcome, HitlError> {
            unimplemented!("not exercised by resolve_display_row")
        }
        async fn mark_resume_unknown(&self, _id: Uuid) -> Result<(), HitlError> {
            unimplemented!("not exercised by resolve_display_row")
        }
        async fn record_auth_start(&self, _id: Uuid) -> Result<Option<HitlRequest>, HitlError> {
            unimplemented!("not exercised by resolve_display_row")
        }
    }

    fn row(id: Uuid, origin: HitlOrigin, kind: HitlKind, question: Value) -> HitlRequest {
        let now = Utc::now();
        HitlRequest {
            id,
            kind,
            origin,
            status: HitlStatus::Pending,
            resume_status: ResumeStatus::NotStarted,
            agent_id: Uuid::new_v4(),
            owner_user_id: Uuid::new_v4(),
            resolved_by: None,
            task_id: Some("task-1".to_string()),
            context_id: Some("ctx-1".to_string()),
            chat_session_id: Some("ses-1".to_string()),
            maf_execution_id: None,
            maf_step_index: None,
            connector_id: None,
            tool_name: None,
            arguments_hash: None,
            consumed_at: None,
            question,
            human_response: None,
            resume_state: Value::Null,
            resume_claimed_at: None,
            resume_dispatch_attempts: 0,
            resume_last_error: None,
            created_at: now,
            updated_at: now,
            expires_at: None,
            resolved_at: None,
        }
    }

    // (1) A genuine, non-MCP-linked pause is shown unchanged.
    #[tokio::test]
    async fn non_linked_pause_is_shown_unchanged() {
        let mirror = row(
            Uuid::new_v4(),
            HitlOrigin::DirectChat,
            HitlKind::InputRequired,
            json!({ "message": "Which movie would you like to watch?" }),
        );
        let store = FakeStore::default();

        let display = resolve_display_row(&store, &mirror, mirror.owner_user_id).await;

        assert_eq!(display.id, mirror.id);
        assert_eq!(display.kind, mirror.kind);
        assert_eq!(display.question, mirror.question);
        assert_eq!(display.task_id, mirror.task_id);
        assert_eq!(display.context_id, mirror.context_id);
        assert_eq!(display.chat_session_id, mirror.chat_session_id);
    }

    // (2) An MCP-linked mirror shows the real row's id/kind/question instead of its own, with
    // the mirror's own task_id/context_id/chat_session_id preserved — when both rows share the
    // same owner (the ordinary case: one human's agent mapped one real MCP event onto its own
    // pause).
    #[tokio::test]
    async fn mcp_linked_mirror_shows_the_real_row() {
        let mcp_id = Uuid::new_v4();
        let mirror = row(
            Uuid::new_v4(),
            HitlOrigin::DirectChat,
            HitlKind::AuthRequired,
            json!({
                "message": "Please authorize with GitHub",
                "metadata": { "hitl_request_id": mcp_id.to_string() },
            }),
        );
        let mut mcp_row = row(
            mcp_id,
            HitlOrigin::McpTool,
            HitlKind::ToolApproval,
            json!({ "message": "Approve creating a GitHub issue?" }),
        );
        mcp_row.owner_user_id = mirror.owner_user_id;
        mcp_row.agent_id = mirror.agent_id;
        let store = FakeStore::default();
        store.0.lock().unwrap().insert(mcp_id, mcp_row.clone());

        let display = resolve_display_row(&store, &mirror, mirror.owner_user_id).await;

        assert_eq!(display.id, mcp_row.id);
        assert_eq!(display.kind, mcp_row.kind);
        assert_eq!(display.question, mcp_row.question);
        // The mirror's own identity — what the frontend correlates to the visible task — is
        // untouched.
        assert_eq!(display.task_id, mirror.task_id);
        assert_eq!(display.context_id, mirror.context_id);
        assert_eq!(display.chat_session_id, mirror.chat_session_id);
    }

    // (3) A stale/bogus hitl_request_id doesn't break the event — it falls back to the mirror.
    #[tokio::test]
    async fn stale_or_malformed_link_falls_back_to_the_mirror() {
        let store = FakeStore::default();

        let missing_link = row(
            Uuid::new_v4(),
            HitlOrigin::DirectChat,
            HitlKind::AuthRequired,
            json!({
                "message": "Please authorize",
                "metadata": { "hitl_request_id": Uuid::new_v4().to_string() },
            }),
        );
        let display = resolve_display_row(&store, &missing_link, missing_link.owner_user_id).await;
        assert_eq!(display.id, missing_link.id);
        assert_eq!(display.kind, missing_link.kind);
        assert_eq!(display.question, missing_link.question);

        let malformed_link = row(
            Uuid::new_v4(),
            HitlOrigin::AgentProxy,
            HitlKind::InputRequired,
            json!({
                "message": "What's next?",
                "metadata": { "hitl_request_id": "not-a-uuid" },
            }),
        );
        let display =
            resolve_display_row(&store, &malformed_link, malformed_link.owner_user_id).await;
        assert_eq!(display.id, malformed_link.id);
        assert_eq!(display.question, malformed_link.question);

        // No metadata at all — the ordinary agent_proxy/direct_chat case — is just as safe.
        let no_metadata = row(
            Uuid::new_v4(),
            HitlOrigin::DirectChat,
            HitlKind::InputRequired,
            json!({ "message": "Plain pause, no MCP involved" }),
        );
        let display = resolve_display_row(&store, &no_metadata, no_metadata.owner_user_id).await;
        assert_eq!(display.id, no_metadata.id);
    }

    // (3b) The security fix itself: a linked id that resolves to a REAL row owned by someone
    // else must never substitute that row's identity/question into this requester's view — an
    // agent-controlled `hitl_request_id` pointed at another user's row is exactly the scenario
    // `caller_owner_id` exists to close off.
    #[tokio::test]
    async fn cross_user_link_does_not_leak_another_users_row() {
        let mcp_id = Uuid::new_v4();
        let mcp_row = row(
            mcp_id,
            HitlOrigin::McpTool,
            HitlKind::ToolApproval,
            json!({ "message": "Approve creating a GitHub issue in someone else's private repo?" }),
        );
        let mirror = row(
            Uuid::new_v4(),
            HitlOrigin::DirectChat,
            HitlKind::AuthRequired,
            json!({
                "message": "Please authorize with GitHub",
                "metadata": { "hitl_request_id": mcp_id.to_string() },
            }),
        );
        // Deliberately NOT aligned — `mcp_row`'s own `owner_user_id` (from `row()`) is a
        // different random user than `mirror`'s.
        assert_ne!(mcp_row.owner_user_id, mirror.owner_user_id);
        let store = FakeStore::default();
        store.0.lock().unwrap().insert(mcp_id, mcp_row.clone());

        let display = resolve_display_row(&store, &mirror, mirror.owner_user_id).await;

        // Falls back to the mirror unchanged — never the other user's real question/id.
        assert_eq!(display.id, mirror.id);
        assert_eq!(display.question, mirror.question);
        assert_ne!(display.id, mcp_row.id);
        assert_ne!(display.question, mcp_row.question);
    }

    // origin=mcp_tool is never a mirror itself — it's the real row every mirror links back to.
    // Even one that happens to carry a (meaningless) metadata.hitl_request_id must pass through
    // unchanged rather than recursing into another lookup.
    #[tokio::test]
    async fn non_mirror_origins_are_never_substituted() {
        let other_id = Uuid::new_v4();
        let store = FakeStore::default();
        store.0.lock().unwrap().insert(
            other_id,
            row(
                other_id,
                HitlOrigin::DirectChat,
                HitlKind::InputRequired,
                json!({ "message": "unrelated" }),
            ),
        );

        // `mcp_tool` is the *real* row a mirror links to — it's never itself a mirror of
        // another row, even one that happens to carry a (meaningless) metadata.hitl_request_id.
        let mcp_row = row(
            Uuid::new_v4(),
            HitlOrigin::McpTool,
            HitlKind::ToolApproval,
            json!({ "message": "needs approval", "metadata": { "hitl_request_id": other_id.to_string() } }),
        );
        let display = resolve_display_row(&store, &mcp_row, mcp_row.owner_user_id).await;
        assert_eq!(display.id, mcp_row.id);
        assert_eq!(display.question, mcp_row.question);
    }

    // `maf` is a mirror-capable origin too — a MAF step's underlying agent call can map an MCP
    // tool block onto its own pause exactly like direct_chat/agent_proxy can, and its discovery
    // surface (`maf.rs::hitl_rows_for_execution`) needs the same substitution.
    #[tokio::test]
    async fn maf_linked_mirror_shows_the_real_row() {
        let mcp_id = Uuid::new_v4();
        let mirror = row(
            Uuid::new_v4(),
            HitlOrigin::Maf,
            HitlKind::AuthRequired,
            json!({
                "message": "Please authorize with GitHub",
                "metadata": { "hitl_request_id": mcp_id.to_string() },
            }),
        );
        let mut mcp_row = row(
            mcp_id,
            HitlOrigin::McpTool,
            HitlKind::ToolApproval,
            json!({ "message": "Approve creating a GitHub issue?" }),
        );
        mcp_row.owner_user_id = mirror.owner_user_id;
        mcp_row.agent_id = mirror.agent_id;
        let store = FakeStore::default();
        store.0.lock().unwrap().insert(mcp_id, mcp_row.clone());

        let display = resolve_display_row(&store, &mirror, mirror.owner_user_id).await;

        assert_eq!(display.id, mcp_row.id);
        assert_eq!(display.kind, mcp_row.kind);
        assert_eq!(display.question, mcp_row.question);
        // The mirror's own identity — what MAF correlates back to the paused step — is untouched.
        assert_eq!(display.task_id, mirror.task_id);
        assert_eq!(display.context_id, mirror.context_id);
    }

    // `orchestrator` is a mirror-capable origin too — a sub-agent the orchestrator delegates to
    // can map an MCP tool block onto its own pause exactly like a direct-chat agent can, and the
    // orchestrator's own live-stream discovery (`build_hitl_stream_event` in
    // `router/a2a_dispatch.rs`) needs the same substitution.
    #[tokio::test]
    async fn orchestrator_linked_mirror_shows_the_real_row() {
        let mcp_id = Uuid::new_v4();
        let mirror = row(
            Uuid::new_v4(),
            HitlOrigin::Orchestrator,
            HitlKind::AuthRequired,
            json!({
                "message": "Please authorize with GitHub",
                "metadata": { "hitl_request_id": mcp_id.to_string() },
            }),
        );
        let mut mcp_row = row(
            mcp_id,
            HitlOrigin::McpTool,
            HitlKind::ToolApproval,
            json!({ "message": "Approve creating a GitHub issue?" }),
        );
        mcp_row.owner_user_id = mirror.owner_user_id;
        mcp_row.agent_id = mirror.agent_id;
        let store = FakeStore::default();
        store.0.lock().unwrap().insert(mcp_id, mcp_row.clone());

        let display = resolve_display_row(&store, &mirror, mirror.owner_user_id).await;

        assert_eq!(display.id, mcp_row.id);
        assert_eq!(display.kind, mcp_row.kind);
        assert_eq!(display.question, mcp_row.question);
        // The mirror's own identity — what the orchestrator correlates back to the paused
        // sub-agent turn — is untouched.
        assert_eq!(display.task_id, mirror.task_id);
        assert_eq!(display.context_id, mirror.context_id);
    }

    // ─── is_valid_mcp_mirror_link (the extracted predicate, tested directly) ───────────────
    //
    // `resolve_display_row`'s own tests above already exercise this indirectly, but each check
    // here isolates exactly one of the four conditions failing — the direct way to prove the
    // predicate rejects an attacker who gets everything right except one field, which is the
    // realistic shape of the #383 hijack: a real, pending, same-agent `mcp_tool` row that just
    // doesn't belong to the caller.

    fn mcp_row(owner_user_id: Uuid, agent_id: Uuid) -> HitlRequest {
        let mut r = row(
            Uuid::new_v4(),
            HitlOrigin::McpTool,
            HitlKind::ToolApproval,
            json!({ "message": "Approve creating a GitHub issue?" }),
        );
        r.owner_user_id = owner_user_id;
        r.agent_id = agent_id;
        r
    }

    #[test]
    fn matching_row_is_a_valid_link() {
        let owner = Uuid::new_v4();
        let agent = Uuid::new_v4();
        assert!(is_valid_mcp_mirror_link(
            &mcp_row(owner, agent),
            owner,
            agent
        ));
    }

    #[test]
    fn wrong_owner_is_rejected() {
        let agent = Uuid::new_v4();
        let linked = mcp_row(Uuid::new_v4(), agent);
        assert!(!is_valid_mcp_mirror_link(&linked, Uuid::new_v4(), agent));
    }

    #[test]
    fn wrong_agent_is_rejected() {
        let owner = Uuid::new_v4();
        let linked = mcp_row(owner, Uuid::new_v4());
        assert!(!is_valid_mcp_mirror_link(&linked, owner, Uuid::new_v4()));
    }

    #[test]
    fn wrong_origin_is_rejected() {
        let owner = Uuid::new_v4();
        let agent = Uuid::new_v4();
        let mut linked = mcp_row(owner, agent);
        linked.origin = HitlOrigin::DirectChat;
        assert!(!is_valid_mcp_mirror_link(&linked, owner, agent));
    }

    #[test]
    fn wrong_status_is_rejected() {
        let owner = Uuid::new_v4();
        let agent = Uuid::new_v4();
        let mut linked = mcp_row(owner, agent);
        linked.status = HitlStatus::Resolved;
        assert!(!is_valid_mcp_mirror_link(&linked, owner, agent));
    }
}

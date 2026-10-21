//! HITL human-facing API: `GET /api/hitl/pending`,
//! `GET /api/hitl/{id}`, `POST /api/hitl/{id}/resolve`, `POST /api/hitl/{id}/cancel`,
//! `GET /api/hitl/{id}/stream`, `POST /api/hitl/{id}/requeue` (superuser-only operator
//! remediation for a stuck `delivery_outcome_unknown` resume — see `requeue_resume`'s own doc
//! comment).

use std::convert::Infallible;
use std::time::Duration;

use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    response::{
        IntoResponse, Response,
        sse::{Event, KeepAlive, Sse},
    },
    routing::{get, post},
};
use serde::Deserialize;
use serde_json::{Value, json};
use utoipa::ToSchema;
use uuid::Uuid;

use nasiko_hitl::{
    AUTH_ACTION_CONFIRM, AUTH_ACTION_START, AUTH_OUTCOME_CONFIRMED, AUTH_OUTCOME_DENIED,
    DECISION_APPROVE, DECISION_REJECT, GRANT_SCOPE_ONCE, GRANT_SCOPE_SESSION, HitlAction,
    HitlIdentity, HitlKind, HitlRequest, HitlStatus, ResolveDecision, ResolveOutcome, ResumeStatus,
    authorize_hitl_action,
};

use crate::auth::Claims;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/hitl/pending", get(list_pending))
        .route("/hitl/{id}", get(get_one))
        .route("/hitl/{id}/resolve", post(resolve))
        .route("/hitl/{id}/cancel", post(cancel))
        .route("/hitl/{id}/stream", get(stream_one))
        .route("/hitl/{id}/requeue", post(requeue_resume))
}

/// Covers every `HitlKind`'s resolve shape: `answer` for `input_required`, `auth_action` for
/// `auth_required`'s two-click start/confirm, `decision`/`scope`/`note` for `tool_approval`'s
/// approve-once/approve-session/reject. `message` is unused — reserved, not yet part of any kind's
/// contract.
///
/// `answer` is polymorphic (selectable-options extension, additive to `input_required` — never
/// used by `auth_required`/`tool_approval`): a bare string for the pre-existing plain-text shape
/// and for a single-select structured question (`{"answer": "Summary"}`, whether that string
/// matches a predefined option's label or — when `question.allow_custom_input` is set — is custom
/// text), or a string array for a multi-select structured question's predefined selections
/// (`{"answer": ["Introduction", "Conclusion"]}`). `custom_answer` carries multi-select's "Something
/// else" text alongside `answer`, kept as a separate field rather than folded into the array so a
/// custom answer containing a comma or matching another option's label can never be confused with a
/// predefined selection (§11 of the request: no comma-joining, no ambiguity). Neither field is used
/// by a plain (non-structured) `input_required` question, which keeps working exactly as before —
/// see `resolve`'s branching on `row.question`.
#[derive(Deserialize, ToSchema)]
pub(crate) struct HitlResolveRequest {
    answer: Option<HitlAnswer>,
    /// Multi-select's "Something else" text — always alongside `answer`, never in place of it (an
    /// empty/absent `answer` is how "no predefined selections" is expressed for multi-select).
    /// Single-select's custom answer is submitted as a plain `answer` string instead (see the
    /// struct doc comment) — this field is meaningless there and silently ignored, matching this
    /// handler's existing convention of only reading the fields relevant to the row's own shape
    /// (e.g. `tool_approval`'s `note` is read only on that kind's branch).
    custom_answer: Option<String>,
    auth_action: Option<String>,
    decision: Option<String>,
    scope: Option<String>,
    /// Free-form, audit-only note from the human — stored verbatim in `human_response` for
    /// `tool_approval`, never interpreted by this handler.
    note: Option<String>,
    #[allow(dead_code)]
    message: Option<String>,
}

/// `HitlResolveRequest.answer`'s wire shape — see that field's doc comment for what each variant
/// means. `#[serde(untagged)]` tries `String` then `Vec<String>`; the two are structurally
/// distinguishable in JSON, so this is never ambiguous.
#[derive(Deserialize, ToSchema)]
#[serde(untagged)]
pub(crate) enum HitlAnswer {
    Single(String),
    Multiple(Vec<String>),
}

/// A `question`'s selectable-options extension, parsed back out of the JSONB `hoist_structured_options`
/// (`oss/types/src/a2a.rs`) wrote at pause time. `None` from `parse` means this is a plain
/// (non-structured) `input_required` question — every existing pre-extension row, and any row whose
/// agent never set `options` — which must resolve exactly as it did before this feature existed.
struct StructuredOptions {
    labels: Vec<String>,
    multi_select: bool,
    allow_custom_input: bool,
}

impl StructuredOptions {
    fn parse(question: &Value) -> Option<Self> {
        let options = question.get("options")?.as_array()?;
        let labels: Vec<String> = options
            .iter()
            .filter_map(|o| o.get("label")?.as_str().map(str::to_string))
            .collect();
        if labels.is_empty() {
            return None;
        }
        Some(Self {
            labels,
            multi_select: question
                .get("multi_select")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
            allow_custom_input: question
                .get("allow_custom_input")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
        })
    }

    fn contains(&self, label: &str) -> bool {
        self.labels.iter().any(|l| l == label)
    }
}

/// Maximum length, in bytes, for any single human-supplied free-text field on a resolve request
/// (`answer`, each entry of a multi-select `answer`, `custom_answer`, `note`) — none of them had a
/// bound before (found in review): `resolve_structured_answer` validates membership and emptiness
/// but not size, and the plain `input_required` path had no check at all. Generous for genuine
/// free text while still bounding `human_response` JSONB row growth and the size of what gets
/// forwarded verbatim into an A2A message to the paused agent.
const MAX_ANSWER_LEN: usize = 8192;

/// Rejects a resolve request whose `answer`/`custom_answer`/`note` exceeds [`MAX_ANSWER_LEN`],
/// before any of them reach a kind-specific branch — so every kind gets the same bound rather than
/// each branch needing its own check.
fn validate_resolve_payload_lengths(payload: &HitlResolveRequest) -> Result<(), &'static str> {
    let too_long = |s: &str| s.len() > MAX_ANSWER_LEN;
    let answer_too_long = match &payload.answer {
        Some(HitlAnswer::Single(s)) => too_long(s),
        Some(HitlAnswer::Multiple(items)) => items.iter().any(|s| too_long(s)),
        None => false,
    };
    if answer_too_long
        || payload.custom_answer.as_deref().is_some_and(too_long)
        || payload.note.as_deref().is_some_and(too_long)
    {
        return Err("answer/custom_answer/note exceeds the maximum allowed length");
    }
    Ok(())
}

/// Validates and shapes a structured `input_required` answer into the exact `human_response` JSON
/// to persist — the single-select and multi-select rules from §5/§6/§8/§9/§12 of the request. Never
/// called for a plain (non-structured) question — see `resolve`'s branch on `StructuredOptions::parse`.
fn resolve_structured_answer(
    opts: &StructuredOptions,
    answer: Option<&HitlAnswer>,
    custom_answer: Option<&str>,
) -> Result<Value, &'static str> {
    let custom_answer = custom_answer.map(str::trim).filter(|c| !c.is_empty());

    if !opts.multi_select {
        // Single-select: `answer` is always a bare string — a predefined label, or (only when
        // allowed) custom text. There is no separate "is this custom" flag on the wire; membership
        // in `opts.labels` is what distinguishes the two, per §4/§8's "labels are the canonical
        // semantic answer value."
        let answer = match answer {
            Some(HitlAnswer::Single(s)) if !s.trim().is_empty() => s.trim(),
            Some(HitlAnswer::Multiple(_)) => {
                return Err("answer must be a single string for a single-select question");
            }
            _ => return Err("answer is required for input_required"),
        };
        if opts.contains(answer) || opts.allow_custom_input {
            return Ok(json!({ "answer": answer }));
        }
        return Err(
            "answer does not match any offered option, and custom input is not allowed for this question",
        );
    }

    // Multi-select: `answer` (if present at all) must be an array — §6's "no minimum, no maximum"
    // means an absent/empty array is valid on its own as long as `custom_answer` carries something.
    let selected: Vec<String> = match answer {
        None => Vec::new(),
        Some(HitlAnswer::Multiple(items)) => items.clone(),
        Some(HitlAnswer::Single(s)) if s.trim().is_empty() => Vec::new(),
        Some(HitlAnswer::Single(_)) => {
            return Err("answer must be an array of selected options for a multi-select question");
        }
    };

    // Dedupe, preserving first-occurrence order (§12: "duplicate selections should be rejected or
    // normalized safely" — normalizing is strictly more forgiving of e.g. a double-submitted
    // checkbox toggle than rejecting the whole request over it).
    let mut normalized = Vec::with_capacity(selected.len());
    for label in selected {
        let label = label.trim().to_string();
        if label.is_empty() {
            continue;
        }
        if !opts.contains(&label) {
            return Err("answer contains an option that was not offered by this question");
        }
        if !normalized.contains(&label) {
            normalized.push(label);
        }
    }

    if normalized.is_empty() && custom_answer.is_none() {
        return Err("at least one selected option or a custom answer is required");
    }

    let mut response = json!({ "answer": normalized });
    if let (Some(obj), Some(custom_answer)) = (response.as_object_mut(), custom_answer) {
        obj.insert("custom_answer".to_string(), json!(custom_answer));
    }
    Ok(response)
}

/// Logs the real error server-side and returns the generic `{error, correlation_id}` body every
/// caller in this router sends instead — an operator greps the logs for the id. `sqlx::Error`/
/// `HitlError::Db` stringify to constraint names, table names, and sometimes column values — none
/// of which belongs in an API response (found in review); the previous
/// `(StatusCode::INTERNAL_SERVER_ERROR, e.to_string())` shape leaked exactly that to every caller
/// of this router, including the SSE error frame. Returns the bare JSON body (not a `Response`) so
/// the SSE site (`stream_one`'s `async_stream::stream!` block) can wrap it in an `Event` instead of
/// an HTTP response — the two call shapes can't share a return type, but they share this logging +
/// body construction.
fn internal_error_body(context: &'static str, error: impl std::fmt::Display) -> Value {
    let correlation_id = Uuid::new_v4();
    tracing::error!(%correlation_id, %error, context, "hitl: internal error");
    json!({ "error": "internal error", "correlation_id": correlation_id })
}

fn internal_error(context: &'static str, error: impl std::fmt::Display) -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(internal_error_body(context, error)),
    )
        .into_response()
}

fn identity(claims: &Claims) -> Result<HitlIdentity, (StatusCode, &'static str)> {
    let user_id = claims.user_uuid()?;
    Ok(HitlIdentity {
        user_id,
        is_superuser: claims.is_superuser,
    })
}

fn allowed_actions(kind: HitlKind) -> &'static [&'static str] {
    match kind {
        HitlKind::InputRequired => &["answer", "cancel"],
        HitlKind::AuthRequired => &[AUTH_ACTION_START, AUTH_ACTION_CONFIRM, "cancel"],
        HitlKind::ToolApproval => &[DECISION_APPROVE, DECISION_REJECT, "cancel"],
    }
}

/// §11's response shape. `resume_state` is never included — `HitlRequest` isn't `Serialize` for
/// exactly this reason, so this DTO is the only path an API response can take.
///
/// `pub(crate)` (not private): both `chat/routes.rs::list_messages` (a session's HITL rows on
/// session-load) and `maf.rs::get_execution`/`get_result` (a MAF execution's HITL rows) reuse
/// this exact DTO rather than hand-rolling their own field selection that could drift from this
/// one (e.g. accidentally including `resume_state`).
pub(crate) fn to_response(row: &HitlRequest) -> Value {
    json!({
        "id": row.id,
        "kind": row.kind.as_str(),
        "status": row.status.as_str(),
        "resume_status": row.resume_status.as_str(),
        "question": row.question,
        "human_response": row.human_response,
        "execution": {
            "origin": row.origin.as_str(),
            "agent_id": row.agent_id,
            "task_id": row.task_id,
            "context_id": row.context_id,
            "chat_session_id": row.chat_session_id,
            "maf_execution_id": row.maf_execution_id,
            "maf_step_index": row.maf_step_index,
        },
        "allowed_actions": allowed_actions(row.kind),
        "expires_at": row.expires_at,
        "created_at": row.created_at,
        "resolved_at": row.resolved_at,
    })
}

async fn list_pending(State(state): State<AppState>, claims: Claims) -> Response {
    let identity = match identity(&claims) {
        Ok(i) => i,
        Err(e) => return e.into_response(),
    };
    match state.hitl_store.list_pending_for(&identity).await {
        Ok(rows) => Json(json!({ "data": rows.iter().map(to_response).collect::<Vec<_>>() }))
            .into_response(),
        Err(e) => internal_error("list_pending: store error", e),
    }
}

async fn get_one(State(state): State<AppState>, claims: Claims, Path(id): Path<Uuid>) -> Response {
    let identity = match identity(&claims) {
        Ok(i) => i,
        Err(e) => return e.into_response(),
    };
    let row = match state.hitl_store.get(id).await {
        Ok(Some(row)) => row,
        Ok(None) => return (StatusCode::NOT_FOUND, "hitl request not found").into_response(),
        Err(e) => return internal_error("get_one: store error", e),
    };
    // §11 is explicit that this is 403, not the 404-for-view convention used elsewhere in this
    // codebase for named/enumerable resources — `hitl_requests` ids are opaque UUIDs.
    if authorize_hitl_action(&identity, &row, HitlAction::View).is_err() {
        return (
            StatusCode::FORBIDDEN,
            "not authorized to view this HITL request",
        )
            .into_response();
    }
    Json(to_response(&row)).into_response()
}

async fn resolve(
    State(state): State<AppState>,
    claims: Claims,
    Path(id): Path<Uuid>,
    Json(payload): Json<HitlResolveRequest>,
) -> Response {
    let identity = match identity(&claims) {
        Ok(i) => i,
        Err(e) => return e.into_response(),
    };
    let user_id = identity.user_id;

    if let Err(msg) = validate_resolve_payload_lengths(&payload) {
        return (StatusCode::BAD_REQUEST, msg).into_response();
    }

    let row = match state.hitl_store.get(id).await {
        Ok(Some(row)) => row,
        Ok(None) => return (StatusCode::NOT_FOUND, "hitl request not found").into_response(),
        Err(e) => return internal_error("resolve: store error fetching row", e),
    };
    if authorize_hitl_action(&identity, &row, HitlAction::Resolve).is_err() {
        return (
            StatusCode::FORBIDDEN,
            "not authorized to resolve this HITL request",
        )
            .into_response();
    }
    // The finalized three-action dialog: allow once, allow for this session, deny — no `always`.
    // `scope` only matters on `approve`; defaults to `once` when omitted, so an existing caller
    // that never sends it keeps single-use behavior unchanged.
    if row.kind == HitlKind::ToolApproval {
        let decision = match payload.decision.as_deref().and_then(ResolveDecision::parse) {
            Some(d) => d,
            None => {
                return (
                    StatusCode::BAD_REQUEST,
                    "decision must be \"approve\" or \"reject\" for tool_approval",
                )
                    .into_response();
            }
        };
        let approve = decision == ResolveDecision::Approve;
        let scope = match payload.scope.as_deref() {
            None | Some(GRANT_SCOPE_ONCE) => GRANT_SCOPE_ONCE,
            Some(GRANT_SCOPE_SESSION) => GRANT_SCOPE_SESSION,
            Some(_) => {
                return (
                    StatusCode::BAD_REQUEST,
                    "scope must be \"once\" or \"session\"",
                )
                    .into_response();
            }
        };

        let status = decision.target_status();
        let human_response = json!({
            "decision": decision.as_str(),
            "scope": if approve { Some(scope) } else { None },
            "note": payload.note,
        });

        let outcome = match state
            .hitl_store
            .resolve(id, human_response, user_id, status)
            .await
        {
            Ok(outcome) => outcome,
            Err(e) => return internal_error("resolve: tool_approval resolve failed", e),
        };

        let (row, already_resolved) = match outcome {
            ResolveOutcome::Applied(row) => {
                if approve && scope == GRANT_SCOPE_SESSION {
                    grant_session_scope(&state, &row).await;
                }
                // Unconditional on scope/decision — see `auto_resolve_linked_direct_chat_row`'s
                // own doc comment for why a single approval action must resolve both rows.
                auto_resolve_linked_direct_chat_row(&state, &row, user_id, approve).await;
                // Best-effort latency optimization — the dispatcher's own poll loop is
                // the real delivery guarantee, same as the shared path below.
                let _ = state.hitl_resume_tx.try_send(());
                (row, false)
            }
            // `Expired`/`Canceled` both mean the row died WITHOUT ever being decided through
            // this resolve mechanism — 409, not the idempotent-double-resolve 200 below, since
            // there is no decision to be idempotent about and nothing will ever be delivered for
            // it. `Resolved`/`Rejected` stay 200: both are legitimate decisions this same
            // resolve() path already recorded, so a second call reporting the same outcome is a
            // real idempotent no-op, not a caller being told their answer vanished.
            ResolveOutcome::AlreadyDecided(row)
                if matches!(row.status, HitlStatus::Expired | HitlStatus::Canceled) =>
            {
                return (
                    StatusCode::CONFLICT,
                    format!(
                        "this HITL request was {} before it was answered",
                        row.status.as_str()
                    ),
                )
                    .into_response();
            }
            ResolveOutcome::AlreadyDecided(row) => (row, true),
        };

        let mut body = to_response(&row);
        if let Some(obj) = body.as_object_mut() {
            obj.insert("already_resolved".to_string(), json!(already_resolved));
        }
        return Json(body).into_response();
    }
    // `Some(row.question)` only for a structured `input_required` question (selectable-options
    // extension) — `None` for every plain `input_required` row (every row that predates this
    // feature, and any row whose agent never set `options`), which must validate and resolve
    // exactly as before.
    let structured_options = (row.kind == HitlKind::InputRequired)
        .then(|| StructuredOptions::parse(&row.question))
        .flatten();

    if row.kind == HitlKind::InputRequired && structured_options.is_none() {
        match &payload.answer {
            Some(HitlAnswer::Single(a)) if !a.trim().is_empty() => {}
            Some(HitlAnswer::Multiple(_)) => {
                return (
                    StatusCode::BAD_REQUEST,
                    "answer must be a single string for this question",
                )
                    .into_response();
            }
            _ => {
                return (
                    StatusCode::BAD_REQUEST,
                    "answer is required for input_required",
                )
                    .into_response();
            }
        }
    }

    // §7/Phase 4: `auth_required` is a two-click flow — "start" only records that the human
    // began the external auth step (row stays `pending`, nothing is sent to the agent yet);
    // only "confirm" is treated as the human's decision that triggers a resume. Anything else
    // is rejected outright rather than silently sent to the agent as an unvalidated string.
    if row.kind == HitlKind::AuthRequired {
        match payload.auth_action.as_deref() {
            Some(AUTH_ACTION_START) => {
                let current = match state.hitl_store.record_auth_start(id).await {
                    Ok(Some(row)) => row,
                    // Already resolved/expired/canceled by the time this landed — report
                    // current state rather than erroring, matching resolve/cancel's own
                    // idempotent-success convention (§5).
                    Ok(None) => match state.hitl_store.get(id).await {
                        Ok(Some(row)) => row,
                        Ok(None) => {
                            return (StatusCode::NOT_FOUND, "hitl request not found")
                                .into_response();
                        }
                        Err(e) => {
                            return internal_error("resolve: auth_action=start, refetch failed", e);
                        }
                    },
                    Err(e) => {
                        return internal_error("resolve: record_auth_start failed", e);
                    }
                };
                return Json(to_response(&current)).into_response();
            }
            Some(AUTH_ACTION_CONFIRM) => {} // falls through to the normal resolve path below
            _ => {
                return (
                    StatusCode::BAD_REQUEST,
                    "auth_action must be \"start\" or \"confirm\" for auth_required",
                )
                    .into_response();
            }
        }
    }

    let human_response = if row.kind == HitlKind::AuthRequired {
        // Only "confirm" reaches here (validated above). Intent, not proof — the agent's own
        // next response is what determines whether the external auth actually succeeded (§7).
        json!({ "auth_outcome": AUTH_OUTCOME_CONFIRMED })
    } else if let Some(opts) = &structured_options {
        match resolve_structured_answer(
            opts,
            payload.answer.as_ref(),
            payload.custom_answer.as_deref(),
        ) {
            Ok(response) => response,
            Err(message) => return (StatusCode::BAD_REQUEST, message).into_response(),
        }
    } else {
        // Plain (non-structured) `input_required` — already validated non-empty above; stored
        // verbatim, exactly as before this feature existed.
        let Some(HitlAnswer::Single(answer)) = &payload.answer else {
            // Should be impossible given the validation above, but a panic on a live request
            // costs a lot more than a logged 500 if a future change ever widens what reaches
            // here (found in review) — this whole match, unlike `unreachable!()`, degrades.
            tracing::error!(
                id = %row.id,
                "resolve: plain input_required reached the answer-building step without a \
                 single-string answer — this should be impossible given the validation above"
            );
            return internal_error(
                "resolve: input_required answer shape invariant violated",
                "answer was not HitlAnswer::Single despite earlier validation",
            );
        };
        json!({ "answer": answer })
    };

    let outcome = match state
        .hitl_store
        .resolve(id, human_response, user_id, HitlStatus::Resolved)
        .await
    {
        Ok(outcome) => outcome,
        Err(e) => return internal_error("resolve: input_required/auth_required resolve failed", e),
    };

    let (row, already_resolved) = match outcome {
        ResolveOutcome::Applied(row) => {
            // `auth_required`'s "confirm" is the manual counterpart to the ToolApproval
            // branch's own call above — a real broken-connector-credential pause (not just a
            // permission gate) gets exactly the same direct_chat mirror when an agent maps it
            // onto its own A2A AUTH_REQUIRED state, and it needs the same single-action
            // resolve. There's no reject path for auth_required (only "start"/"confirm" are
            // valid `auth_action`s), so this is always an approval.
            if row.kind == HitlKind::AuthRequired {
                auto_resolve_linked_direct_chat_row(&state, &row, user_id, true).await;
            }
            // Best-effort latency optimization — the dispatcher's own poll loop is the real
            // delivery guarantee (§ Phase 3 item 5).
            let _ = state.hitl_resume_tx.try_send(());
            (row, false)
        }
        // §11: 409 for resolve-after-expired-or-canceled — distinct from the
        // idempotent-double-resolve 200 below, which is for a row someone (possibly this same
        // caller) already answered. See the ToolApproval branch's own match above for why
        // Canceled joins Expired here while Resolved/Rejected stay 200.
        ResolveOutcome::AlreadyDecided(row)
            if matches!(row.status, HitlStatus::Expired | HitlStatus::Canceled) =>
        {
            return (
                StatusCode::CONFLICT,
                format!(
                    "this HITL request was {} before it was answered",
                    row.status.as_str()
                ),
            )
                .into_response();
        }
        ResolveOutcome::AlreadyDecided(row) => (row, true),
    };

    // Persist the human's own answer as a `chat_messages` turn — resolving a pause otherwise
    // left no trace in the session's own transcript at all: `hitl_requests.human_response` holds
    // it, but nothing ever wrote it into the table the web UI's session view (and `nasiko
    // sessions`/`history`) actually renders from, so a session with an answered pause silently
    // jumped from the agent's question straight to whatever it said after resuming, with the
    // human's own reply invisible. Only on a real resolution (`!already_resolved`), so a
    // duplicate/idempotent resolve of an already-answered row can't append it twice. Guarded the
    // same way `agent_proxy.rs`'s own user-message insert is (a short dedup window, not a plain
    // unconditional insert): defensive here too, since a client could in principle retry this
    // same POST. Fire-and-forget and silently a no-op for any origin with no
    // `chat_sessions`-registered session at all (e.g. MAF) — this is specifically for the chat
    // experience, not a correctness-critical write.
    if !already_resolved && let Some(session_id) = crate::hitl::stable_session_id(&row) {
        let answer = crate::hitl::answer_text(&row);
        if !answer.is_empty() {
            crate::chat::spawn_dedup_user_message_insert(
                state.db.clone(),
                session_id.to_string(),
                answer,
            );
        }
    }

    let mut body = to_response(&row);
    if let Some(obj) = body.as_object_mut() {
        obj.insert("already_resolved".to_string(), json!(already_resolved));
    }
    Json(body).into_response()
}

/// Best-effort: record the `mcp_session_tool_grants` row an approved `scope=session` decision
/// promises. `connector_id`/`tool_name`/`context_id` are guaranteed present by
/// `chk_hitl_tool_approval_identity` for any `kind=tool_approval` row, which the caller has
/// already confirmed `row` is. A failure here is logged but never turned into an error response —
/// the resolution itself already succeeded and is the authoritative outcome; worst case the
/// agent's retry finds no grant and gets asked again, which is safe (never silently
/// over-permissive), just not maximally convenient.
async fn grant_session_scope(state: &AppState, row: &HitlRequest) {
    let (Some(connector_id), Some(tool_name), Some(context_id)) = (
        row.connector_id,
        row.tool_name.clone(),
        row.context_id.clone(),
    ) else {
        tracing::error!(
            id = %row.id,
            "resolve: scope=session approved but tool_approval identity fields are missing — \
             this should be impossible under chk_hitl_tool_approval_identity"
        );
        return;
    };

    // `session`-scope grants are keyed by the stable chat-session identity,
    // not the row's own trace-derived `context_id` — see
    // `repo::resolve_stable_session_context`'s own doc comment for why (it's
    // the exact same lookup `resolve_tool_approval_retry`, MCP's own retry
    // path, uses to look this grant back up — create and lookup must agree).
    let session_context_id = match nasiko_hitl::repo::resolve_stable_session_context(
        &state.db,
        row.owner_user_id,
        row.agent_id,
        &context_id,
    )
    .await
    {
        Ok(Some(session_id)) => session_id,
        Ok(None) => context_id,
        Err(e) => {
            tracing::warn!(
                error = %e, id = %row.id,
                "stable session lookup failed — falling back to trace context for the new grant"
            );
            context_id
        }
    };

    if let Err(e) = nasiko_hitl::repo::create_session_grant(
        &state.db,
        nasiko_hitl::NewSessionGrant {
            agent_id: row.agent_id,
            connector_id,
            tool_name,
            context_id: session_context_id,
            // `has_active_session_grant` (the MCP gateway's own retry-matching lookup) filters
            // `granted_by = <the flow's real user>`, resolved off the traceparent — never the
            // HTTP caller who happened to click resolve. `authorize_hitl_action` lets a
            // superuser resolve another user's row, so binding the resolver's own id here (the
            // function's previous `granted_by` parameter) wrote a grant that lookup could never
            // match: the real user was re-prompted for the same tool on every subsequent
            // message, forever. `row.owner_user_id` is the same value
            // `resolve_stable_session_context` just above is already keyed on, and the same
            // value `NewSessionGrant::granted_by`'s own doc comment names as the invariant.
            granted_by: row.owner_user_id,
            hitl_request_id: Some(row.id),
        },
    )
    .await
    {
        tracing::error!(error = %e, id = %row.id, "resolve: failed to create session grant");
    }
}

/// Auto-resolves the `direct_chat`/`agent_proxy`-origin row (if any) mirroring
/// this just-resolved `mcp_tool` row's own event — see
/// `nasiko_hitl::repo::find_linked_direct_chat_row`'s own doc comment for the
/// full reasoning. Best-effort and silent on "nothing to link" (the ordinary
/// case for most `tool_approval` rows, which were never mirrored into a
/// direct-chat pause at all — e.g. a raw MCP integration outside any chat):
/// only a genuine resolve failure on an existing linked row is worth logging.
///
/// `human_response` intentionally doesn't try to carry the tool-approval
/// decision's own shape (`decision`/`scope`/`note`) into the linked row —
/// that row's own kind is `auth_required` (from the agent's `AUTH_REQUIRED`
/// mapping), whose dispatcher (`oss/server/src/hitl/mod.rs::answer_text`)
/// only ever reads `human_response.auth_outcome` to build its resume
/// message. This mirrors the exact shape the console's own two-click
/// `auth_action: confirm` flow already produces for a real auth_required
/// row, so the resume path this triggers is the same one already proven,
/// not a new one.
///
/// Once the mirror is resolved, `mcp_row`'s own resume is skipped
/// (`nasiko_hitl::repo::skip_resume_for_mirrored_row`) rather than left for the
/// `mcp_tool` dispatcher to also deliver: the mirror is the real, task_id-bearing
/// resume — `RuntimeResumeNotifier`'s standalone nudge on `mcp_row` would race it
/// with a context-free message the agent has no way to make sense of (confirmed
/// live: the agent lost the paused conversation and re-asked its own question
/// from scratch, on every single approval).
async fn auto_resolve_linked_direct_chat_row(
    state: &AppState,
    mcp_row: &HitlRequest,
    resolved_by: Uuid,
    approved: bool,
) {
    let linked = match nasiko_hitl::repo::find_linked_direct_chat_row(
        &state.db,
        mcp_row.id,
        mcp_row.owner_user_id,
        mcp_row.agent_id,
    )
    .await
    {
        Ok(Some(row)) => row,
        Ok(None) => return,
        Err(e) => {
            tracing::warn!(error = %e, id = %mcp_row.id, "failed to look up linked direct-chat pause");
            return;
        }
    };

    let status = if approved {
        HitlStatus::Resolved
    } else {
        HitlStatus::Rejected
    };
    let human_response = json!({
        "auth_outcome": if approved { AUTH_OUTCOME_CONFIRMED } else { AUTH_OUTCOME_DENIED },
    });

    match state
        .hitl_store
        .resolve(linked.id, human_response, resolved_by, status)
        .await
    {
        Ok(_) => {
            // The frontend reconnects using `mcp_row.id` (what `resolve_display_row` showed it),
            // but `deliver()` only ever runs on `linked.id` — alias the two so either id finds the
            // same continuation buffer (see `ContinuationRegistry::alias`'s own doc comment).
            state.continuation_events.alias(mcp_row.id, linked.id);
            let _ = state.hitl_resume_tx.try_send(());
            // `linked` is the real resume now — never let the `mcp_tool` dispatcher also fire
            // `mcp_row`'s own task_id-less nudge on top of it (see this function's own doc comment).
            if let Err(e) =
                nasiko_hitl::repo::skip_resume_for_mirrored_row(&state.db, mcp_row.id).await
            {
                tracing::warn!(
                    error = %e, mcp_row_id = %mcp_row.id,
                    "failed to skip the mirrored mcp_tool row's own resume — it may still race the linked row's resume"
                );
            }
        }
        Err(e) => {
            tracing::error!(
                error = %e, mcp_row_id = %mcp_row.id, linked_row_id = %linked.id,
                "failed to auto-resolve the linked direct-chat pause"
            );
        }
    }
}

/// Lets the row's owner withdraw a pending request they no longer want answered — e.g. they
/// abandoned the task, or the question no longer applies. No resume is triggered; the row simply
/// stops being `pending` and drops out of `list_pending_for`.
async fn cancel(State(state): State<AppState>, claims: Claims, Path(id): Path<Uuid>) -> Response {
    let identity = match identity(&claims) {
        Ok(i) => i,
        Err(e) => return e.into_response(),
    };

    let row = match state.hitl_store.get(id).await {
        Ok(Some(row)) => row,
        Ok(None) => return (StatusCode::NOT_FOUND, "hitl request not found").into_response(),
        Err(e) => return internal_error("cancel: store error fetching row", e),
    };
    if authorize_hitl_action(&identity, &row, HitlAction::Cancel).is_err() {
        return (
            StatusCode::FORBIDDEN,
            "not authorized to cancel this HITL request",
        )
            .into_response();
    }

    let outcome = match state.hitl_store.cancel(id, identity.user_id).await {
        Ok(outcome) => outcome,
        Err(e) => return internal_error("cancel: store error", e),
    };

    let (row, already_canceled) = match outcome {
        ResolveOutcome::Applied(row) => (row, false),
        // Matches `resolve`'s own idempotency convention (§5): a lost double-cancel race is a
        // 200, never a 500 or a second no-op — but a row already `resolved` (answered, not
        // canceled) or `expired` before the cancel landed is a real conflict, not a race.
        ResolveOutcome::AlreadyDecided(row) if row.status != HitlStatus::Canceled => {
            return (
                StatusCode::CONFLICT,
                format!(
                    "this HITL request is already {}, not pending",
                    row.status.as_str()
                ),
            )
                .into_response();
        }
        ResolveOutcome::AlreadyDecided(row) => (row, true),
    };

    let mut body = to_response(&row);
    if let Some(obj) = body.as_object_mut() {
        obj.insert("already_canceled".to_string(), json!(already_canceled));
    }
    Json(body).into_response()
}

/// `POST /api/hitl/{id}/requeue` — the operator remediation path
/// `nasiko_hitl::repo::recover_stuck_resumes`'s own doc comment says doesn't exist: a
/// `delivery_outcome_unknown` row (the resume dispatcher died mid-attempt, so whether the agent
/// actually received the answer is unknown) is otherwise stuck forever — `claim_for_resume` never
/// re-selects it, by design, since a resume push isn't naturally idempotent. Superuser-only: this
/// is an explicit "I've confirmed it's safe to retry" judgment call about a specific delivery, not
/// a row-ownership action, so the row's own owner is not authorized to make it themselves.
async fn requeue_resume(
    State(state): State<AppState>,
    claims: Claims,
    Path(id): Path<Uuid>,
) -> Response {
    if !claims.is_superuser {
        return (
            StatusCode::FORBIDDEN,
            "only a superuser may requeue a stuck resume",
        )
            .into_response();
    }

    match nasiko_hitl::repo::requeue_resume(&state.db, id).await {
        Ok(Some(row)) => Json(to_response(&row)).into_response(),
        Ok(None) => (
            StatusCode::CONFLICT,
            "row not found, or not in delivery_outcome_unknown",
        )
            .into_response(),
        Err(e) => internal_error("requeue_resume: store error", e),
    }
}

/// True once nothing further will ever happen to this row without a brand-new request from
/// somewhere: either `status` itself is a dead end (no resume will ever be attempted), or the
/// resume that was attempted has itself reached one of its own terminal states.
fn is_terminal(row: &HitlRequest) -> bool {
    match row.status {
        HitlStatus::Pending | HitlStatus::Resolved => matches!(
            row.resume_status,
            ResumeStatus::Completed
                | ResumeStatus::Failed
                | ResumeStatus::DeliveryOutcomeUnknown
                | ResumeStatus::Skipped
        ),
        HitlStatus::Rejected | HitlStatus::Expired | HitlStatus::Canceled => true,
    }
}

/// `GET /api/hitl/{id}/stream` — DB-poll-wrapped SSE, cloned from the existing
/// `deploy_status_sse`/`build_progress_sse` pattern (`oss/server/src/agents/upload.rs`,
/// `oss/server/src/build/routes.rs`): poll every 3s, emit an event only when `status`/
/// `resume_status`/`human_response` actually change, close once the row reaches a terminal state.
/// `human_response` is in the dedup key (not just the two status columns) because
/// `record_auth_start` writes into it without changing either status column — an `auth_required`
/// row's "start" step would otherwise never surface as its own event, only on the next unrelated
/// change or a fresh reconnect. The authorization check runs once up front — a 403 is a normal
/// HTTP response, not a stream — and every subsequent poll trusts that this connection is already
/// scoped to its owner.
async fn stream_one(
    State(state): State<AppState>,
    claims: Claims,
    Path(id): Path<Uuid>,
) -> Response {
    let identity = match identity(&claims) {
        Ok(i) => i,
        Err(e) => return e.into_response(),
    };
    let row = match state.hitl_store.get(id).await {
        Ok(Some(row)) => row,
        Ok(None) => return (StatusCode::NOT_FOUND, "hitl request not found").into_response(),
        Err(e) => return internal_error("stream_one: store error fetching row", e),
    };
    if authorize_hitl_action(&identity, &row, HitlAction::View).is_err() {
        return (
            StatusCode::FORBIDDEN,
            "not authorized to view this HITL request",
        )
            .into_response();
    }

    let hitl_store = state.hitl_store.clone();
    let stream = async_stream::stream! {
        let mut last: Option<(String, String, Option<String>)> = None;

        loop {
            let row = match hitl_store.get(id).await {
                Ok(Some(row)) => row,
                Ok(None) => {
                    yield Ok::<_, Infallible>(Event::default().data(
                        json!({ "status": "not_found" }).to_string(),
                    ));
                    break;
                }
                Err(e) => {
                    let body = internal_error_body("stream_one: store error (in-stream)", e);
                    yield Ok(Event::default().event("error").data(body.to_string()));
                    break;
                }
            };

            let key = (
                row.status.as_str().to_string(),
                row.resume_status.as_str().to_string(),
                row.human_response.as_ref().map(|v| v.to_string()),
            );
            if Some(&key) != last.as_ref() {
                yield Ok(Event::default().data(to_response(&row).to_string()));
                last = Some(key);
            }

            if is_terminal(&row) {
                break;
            }

            tokio::time::sleep(Duration::from_secs(3)).await;
        }
    };

    Sse::new(stream)
        .keep_alive(KeepAlive::default())
        .into_response()
}

#[cfg(test)]
mod structured_answer_tests {
    use super::*;

    fn single_select(allow_custom_input: bool) -> StructuredOptions {
        StructuredOptions {
            labels: vec!["Summary".to_string(), "Detailed".to_string()],
            multi_select: false,
            allow_custom_input,
        }
    }

    fn multi_select() -> StructuredOptions {
        StructuredOptions {
            labels: vec![
                "Introduction".to_string(),
                "Architecture".to_string(),
                "Security".to_string(),
                "Conclusion".to_string(),
            ],
            multi_select: true,
            allow_custom_input: true,
        }
    }

    // ── payload length bound ─────────────────────────────────────────────────────────────

    fn payload_with_answer(answer: Option<HitlAnswer>) -> HitlResolveRequest {
        HitlResolveRequest {
            answer,
            custom_answer: None,
            auth_action: None,
            decision: None,
            scope: None,
            note: None,
            message: None,
        }
    }

    #[test]
    fn validate_resolve_payload_lengths_accepts_a_normal_answer() {
        let payload = payload_with_answer(Some(HitlAnswer::Single("a reasonable answer".into())));
        assert!(validate_resolve_payload_lengths(&payload).is_ok());
    }

    #[test]
    fn validate_resolve_payload_lengths_rejects_an_oversized_single_answer() {
        let payload = payload_with_answer(Some(HitlAnswer::Single("a".repeat(MAX_ANSWER_LEN + 1))));
        assert!(validate_resolve_payload_lengths(&payload).is_err());
    }

    #[test]
    fn validate_resolve_payload_lengths_rejects_an_oversized_entry_in_a_multi_select_answer() {
        let payload = payload_with_answer(Some(HitlAnswer::Multiple(vec![
            "fine".to_string(),
            "b".repeat(MAX_ANSWER_LEN + 1),
        ])));
        assert!(validate_resolve_payload_lengths(&payload).is_err());
    }

    #[test]
    fn validate_resolve_payload_lengths_rejects_an_oversized_custom_answer() {
        let mut payload = payload_with_answer(None);
        payload.custom_answer = Some("c".repeat(MAX_ANSWER_LEN + 1));
        assert!(validate_resolve_payload_lengths(&payload).is_err());
    }

    #[test]
    fn validate_resolve_payload_lengths_rejects_an_oversized_note() {
        let mut payload = payload_with_answer(None);
        payload.note = Some("n".repeat(MAX_ANSWER_LEN + 1));
        assert!(validate_resolve_payload_lengths(&payload).is_err());
    }

    #[test]
    fn parse_returns_none_for_a_plain_question() {
        assert!(StructuredOptions::parse(&json!({ "message": "env name?" })).is_none());
    }

    #[test]
    fn parse_reads_labels_multi_select_and_allow_custom_input() {
        let question = json!({
            "message": "Which sections?",
            "options": [{"label": "Introduction"}, {"label": "Security", "description": "..."}],
            "multi_select": true,
            "allow_custom_input": true,
        });
        let opts = StructuredOptions::parse(&question).unwrap();
        assert_eq!(opts.labels, vec!["Introduction", "Security"]);
        assert!(opts.multi_select);
        assert!(opts.allow_custom_input);
    }

    #[test]
    fn parse_defaults_multi_select_and_allow_custom_input_to_false() {
        let question = json!({ "options": [{"label": "Yes"}] });
        let opts = StructuredOptions::parse(&question).unwrap();
        assert!(!opts.multi_select);
        assert!(!opts.allow_custom_input);
    }

    // ── single-select ────────────────────────────────────────────────────────────────────

    #[test]
    fn single_select_predefined_option_resolves_to_the_label() {
        let opts = single_select(false);
        let result =
            resolve_structured_answer(&opts, Some(&HitlAnswer::Single("Summary".into())), None)
                .unwrap();
        assert_eq!(result, json!({ "answer": "Summary" }));
    }

    #[test]
    fn single_select_rejects_an_unlisted_answer_when_custom_input_is_disallowed() {
        let opts = single_select(false);
        let err = resolve_structured_answer(
            &opts,
            Some(&HitlAnswer::Single("Delete everything".into())),
            None,
        )
        .unwrap_err();
        assert!(err.contains("does not match"));
    }

    #[test]
    fn single_select_accepts_custom_text_when_allowed() {
        let opts = single_select(true);
        let result = resolve_structured_answer(
            &opts,
            Some(&HitlAnswer::Single(
                "Give me a concise executive summary".into(),
            )),
            None,
        )
        .unwrap();
        assert_eq!(
            result,
            json!({ "answer": "Give me a concise executive summary" })
        );
    }

    #[test]
    fn single_select_requires_a_non_empty_answer() {
        let opts = single_select(true);
        assert!(resolve_structured_answer(&opts, None, None).is_err());
        assert!(
            resolve_structured_answer(&opts, Some(&HitlAnswer::Single("  ".into())), None).is_err()
        );
    }

    #[test]
    fn single_select_rejects_an_array_answer() {
        let opts = single_select(true);
        let err = resolve_structured_answer(
            &opts,
            Some(&HitlAnswer::Multiple(vec!["Summary".into()])),
            None,
        )
        .unwrap_err();
        assert!(err.contains("single string"));
    }

    // ── multi-select ─────────────────────────────────────────────────────────────────────

    #[test]
    fn multi_select_accepts_one_selection() {
        let opts = multi_select();
        let result = resolve_structured_answer(
            &opts,
            Some(&HitlAnswer::Multiple(vec!["Introduction".into()])),
            None,
        )
        .unwrap();
        assert_eq!(result, json!({ "answer": ["Introduction"] }));
    }

    #[test]
    fn multi_select_accepts_multiple_selections() {
        let opts = multi_select();
        let result = resolve_structured_answer(
            &opts,
            Some(&HitlAnswer::Multiple(vec![
                "Introduction".into(),
                "Security".into(),
            ])),
            None,
        )
        .unwrap();
        assert_eq!(result, json!({ "answer": ["Introduction", "Security"] }));
    }

    #[test]
    fn multi_select_accepts_all_selections() {
        let opts = multi_select();
        let all: Vec<String> = opts.labels.clone();
        let result =
            resolve_structured_answer(&opts, Some(&HitlAnswer::Multiple(all.clone())), None)
                .unwrap();
        assert_eq!(result, json!({ "answer": all }));
    }

    #[test]
    fn multi_select_zero_predefined_selections_with_custom_answer_is_valid() {
        let opts = multi_select();
        let result = resolve_structured_answer(
            &opts,
            Some(&HitlAnswer::Multiple(vec![])),
            Some("Only discuss security implications"),
        )
        .unwrap();
        assert_eq!(
            result,
            json!({ "answer": [], "custom_answer": "Only discuss security implications" })
        );
    }

    #[test]
    fn multi_select_selections_plus_custom_answer_preserves_both() {
        let opts = multi_select();
        let result = resolve_structured_answer(
            &opts,
            Some(&HitlAnswer::Multiple(vec![
                "Introduction".into(),
                "Security".into(),
            ])),
            Some("Also include deployment risks"),
        )
        .unwrap();
        assert_eq!(
            result,
            json!({
                "answer": ["Introduction", "Security"],
                "custom_answer": "Also include deployment risks",
            })
        );
    }

    #[test]
    fn multi_select_missing_answer_is_treated_as_zero_selections() {
        let opts = multi_select();
        let result =
            resolve_structured_answer(&opts, None, Some("Only discuss security implications"))
                .unwrap();
        assert_eq!(
            result,
            json!({ "answer": [], "custom_answer": "Only discuss security implications" })
        );
    }

    #[test]
    fn multi_select_zero_selections_and_no_custom_answer_is_rejected() {
        let opts = multi_select();
        let err = resolve_structured_answer(&opts, Some(&HitlAnswer::Multiple(vec![])), None)
            .unwrap_err();
        assert!(err.contains("at least one"));
    }

    #[test]
    fn multi_select_rejects_an_option_that_was_not_offered() {
        let opts = multi_select();
        let err = resolve_structured_answer(
            &opts,
            Some(&HitlAnswer::Multiple(vec!["Not a real option".into()])),
            None,
        )
        .unwrap_err();
        assert!(err.contains("not offered"));
    }

    #[test]
    fn multi_select_deduplicates_repeated_selections() {
        let opts = multi_select();
        let result = resolve_structured_answer(
            &opts,
            Some(&HitlAnswer::Multiple(vec![
                "Introduction".into(),
                "Introduction".into(),
                "Security".into(),
            ])),
            None,
        )
        .unwrap();
        assert_eq!(result, json!({ "answer": ["Introduction", "Security"] }));
    }

    #[test]
    fn multi_select_rejects_a_bare_string_answer() {
        let opts = multi_select();
        let err = resolve_structured_answer(
            &opts,
            Some(&HitlAnswer::Single("Introduction".into())),
            None,
        )
        .unwrap_err();
        assert!(err.contains("array"));
    }
}

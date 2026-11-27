//! `/api/harnesses` and `/api/sessions`: list harnesses, open a session on one (which creates the
//! user's harness agent for it if they have none and starts the session's sandbox, `super::start`),
//! read it back, list them, suspend a sandbox, and delete a session.
//!
//! A session is owner-only. Anyone else gets the same 404 as for a session that does not
//! exist, so ids cannot be probed. Bodies follow `docs/API_CONVENTIONS.md`: `{"data": ...}`
//! envelopes and `{"error", "code"}` failures.

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use base64::Engine;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use super::admission::refuse_session;
use super::catalog::{Harness, HarnessCatalog};
use super::model::{ListedSession, SessionFilter, SessionRow, SessionRuntimeState, SessionStore};
use super::own_agent;
use super::suspend::{ReapError, Suspend, claim_suspend, claim_unaccounted};
use crate::auth::Claims;
use crate::state::AppState;

const MAX_REPO_LEN: usize = 2048;
const DEFAULT_LIST_LIMIT: i64 = 50;
const MAX_LIST_LIMIT: i64 = 200;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/harnesses", get(list_harnesses))
        .route("/sessions", post(create_session).get(list_sessions))
        .route("/sessions/{id}", get(get_session).delete(delete_session))
        .route("/sessions/{id}/suspend", post(suspend_session))
}

/// A failed request: status, stable machine slug, human message.
///
/// TODO NAS-1098: replace with the shared `dto::ApiError` once that crate exists.
struct ApiError(StatusCode, &'static str, &'static str);

impl ApiError {
    const NOT_FOUND: Self = Self(
        StatusCode::NOT_FOUND,
        "session_not_found",
        "session not found",
    );

    /// A 5xx whose cause is logged, never echoed.
    fn internal(context: &str, cause: impl std::fmt::Display) -> Self {
        tracing::error!(%cause, "{context}");
        Self(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal_error",
            "internal error",
        )
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(json!({ "error": self.2, "code": self.1 }))).into_response()
    }
}

type ApiResult<T> = Result<T, ApiError>;

#[derive(Serialize)]
struct HarnessView {
    id: String,
    name: String,
    image: String,
    protocol_version: i32,
    enabled: bool,
}

impl From<Harness> for HarnessView {
    fn from(h: Harness) -> Self {
        Self {
            id: h.id,
            name: h.name,
            image: h.image,
            protocol_version: h.protocol_version,
            enabled: h.enabled,
        }
    }
}

/// The harnesses a session can be opened on: none while coding sessions are switched off or
/// cannot run on this server (`sessions::readiness`).
async fn list_harnesses(State(state): State<AppState>, _claims: Claims) -> ApiResult<Response> {
    let harnesses = if state.config.harness_sessions_enabled
        && super::readiness::unavailable(&state).is_none()
    {
        HarnessCatalog::new(state.db.clone())
            .list_enabled()
            .await
            .map_err(|e| ApiError::internal("list harnesses", e))?
    } else {
        Vec::new()
    };
    let data: Vec<HarnessView> = harnesses.into_iter().map(Into::into).collect();
    Ok(Json(json!({
        "data": data, "has_more": false, "next_cursor": null, "prev_cursor": null
    }))
    .into_response())
}

#[derive(Deserialize)]
struct CreateSession {
    harness: String,
    repo: Option<String>,
}

#[derive(Serialize)]
struct SessionView {
    session_id: String,
    harness: Option<String>,
    agent_id: Option<Uuid>,
    runtime_state: String,
    sandbox_ref: Option<String>,
    last_turn_at: Option<DateTime<Utc>>,
    suspended_at: Option<DateTime<Utc>>,
    workspace_repo: Option<String>,
}

impl SessionView {
    fn new(row: SessionRow, harness: Option<String>) -> Self {
        Self {
            session_id: row.session_id,
            harness,
            agent_id: row.agent_id,
            runtime_state: row.runtime_state,
            sandbox_ref: row.sandbox_ref,
            last_turn_at: row.last_turn_at,
            suspended_at: row.suspended_at,
            workspace_repo: row.workspace_repo,
        }
    }
}

async fn create_session(
    State(state): State<AppState>,
    claims: Claims,
    Json(body): Json<CreateSession>,
) -> ApiResult<Response> {
    let user_id = claims.user_uuid().map_err(unauthorized)?;
    let repo = valid_repo(body.repo.as_deref(), &state.config.git_clone_allowed_hosts)?;
    let harness = enabled_harness(&state, &body.harness).await?;
    if let Some(refusal) = refuse_session(&state, user_id, &harness.id).await {
        return Ok(refusal.into_response());
    }
    if super::readiness::unavailable(&state).is_some() {
        return Err(ApiError(
            StatusCode::SERVICE_UNAVAILABLE,
            "harness_not_ready",
            "coding sessions are not set up on this server",
        ));
    }
    let agent_id = own_harness_agent(&state, user_id, &harness).await?;

    let store = SessionStore::new(state.db.clone());
    let mut row = store
        .create(agent_id, user_id, repo)
        .await
        .map_err(|e| ApiError::internal("create harness session", e))?;
    // The click that opened the session starts its sandbox; the row is answered as the start
    // left it (`starting`, or `running` if it was quick).
    if super::start::begin(&state, &row.session_id, agent_id).await
        && let Ok(Some(started)) = store.get(&row.session_id).await
    {
        row = started;
    }
    Ok((
        StatusCode::CREATED,
        Json(json!({ "data": SessionView::new(row, Some(harness.id)) })),
    )
        .into_response())
}

fn unauthorized((status, message): (StatusCode, &'static str)) -> ApiError {
    ApiError(status, "unauthorized", message)
}

/// A repo the workspace may be cloned from: a URL (https or ssh) on an allowed git host, with
/// no embedded credentials (a password, or a username on anything but ssh, would land in
/// logs and the stored row). Absent or blank means no repo. The host list is the same
/// `GIT_CLONE_ALLOWED_HOSTS` the build paths use; their guard is https-only, so it is not
/// reused as is.
fn valid_repo<'a>(repo: Option<&'a str>, allowed_hosts: &[String]) -> ApiResult<Option<&'a str>> {
    let invalid = |msg| ApiError(StatusCode::BAD_REQUEST, "invalid_repo", msg);
    let Some(repo) = repo.map(str::trim).filter(|r| !r.is_empty()) else {
        return Ok(None);
    };
    if repo.len() > MAX_REPO_LEN {
        return Err(invalid("repo is too long"));
    }
    let Ok(url) = reqwest::Url::parse(repo) else {
        return Err(if repo.contains('@') && !repo.contains("://") {
            invalid("use ssh://git@host/org/repo.git")
        } else {
            invalid("repo must be a URL")
        });
    };
    if !matches!(url.scheme(), "https" | "ssh") {
        return Err(invalid("repo scheme must be https or ssh"));
    }
    if url.password().is_some() || (!url.username().is_empty() && url.scheme() != "ssh") {
        return Err(invalid("repo URL must not embed credentials"));
    }
    match url.host_str() {
        Some(host) if allowed_hosts.iter().any(|h| h == host) => Ok(Some(repo)),
        _ => Err(invalid("repo host is not in the allowed list")),
    }
}

async fn enabled_harness(state: &AppState, id: &str) -> ApiResult<Harness> {
    let found = HarnessCatalog::new(state.db.clone())
        .get(id)
        .await
        .map_err(|e| ApiError::internal("get harness", e))?;
    found.filter(|h| h.enabled).ok_or(ApiError(
        StatusCode::BAD_REQUEST,
        "unknown_harness",
        "unknown or disabled harness",
    ))
}

/// The user's own harness agent for `harness` (`own_agent`), created if they have none.
async fn own_harness_agent(state: &AppState, user_id: Uuid, harness: &Harness) -> ApiResult<Uuid> {
    own_agent::ensure(
        &state.db,
        user_id,
        harness,
        &state.config.harness_image_prefix,
    )
    .await
    .map_err(|e| match e {
        own_agent::OwnAgentError::NameTaken(_) => ApiError(
            StatusCode::CONFLICT,
            "harness_agent_name_taken",
            "you already have an agent named after this harness's agent \
                 (harness-<harness id>); rename it to open a session of this harness",
        ),
        own_agent::OwnAgentError::Db(e) => ApiError::internal("create the user's harness agent", e),
    })
}

/// The caller's own harness session and its harness id, or the not-found response.
async fn owned_session(
    state: &AppState,
    claims: &Claims,
    id: &str,
) -> ApiResult<(SessionRow, Option<String>)> {
    let user_id = claims.user_uuid().map_err(unauthorized)?;
    let found = SessionStore::new(state.db.clone())
        .get_with_harness(id)
        .await
        .map_err(|e| ApiError::internal("get harness session", e))?;
    found
        .filter(|(row, _)| row.user_id == user_id)
        .ok_or(ApiError::NOT_FOUND)
}

async fn get_session(
    State(state): State<AppState>,
    claims: Claims,
    Path(id): Path<String>,
) -> ApiResult<Response> {
    let (row, harness) = owned_session(&state, &claims, &id).await?;
    Ok(Json(json!({ "data": SessionView::new(row, harness) })).into_response())
}

#[derive(Deserialize)]
struct ListParams {
    harness: Option<String>,
    state: Option<String>,
    limit: Option<i64>,
    cursor: Option<String>,
    user_id: Option<Uuid>,
}

/// The caller's harness sessions, newest activity first. A superuser may pass `user_id` to list
/// someone else's; for anyone else that parameter is a 403.
async fn list_sessions(
    State(state): State<AppState>,
    claims: Claims,
    Query(params): Query<ListParams>,
) -> ApiResult<Response> {
    let caller = claims.user_uuid().map_err(unauthorized)?;
    let user_id = match params.user_id {
        Some(other) if other != caller && !claims.is_superuser => {
            return Err(ApiError(
                StatusCode::FORBIDDEN,
                "forbidden",
                "only a superuser may list another user's sessions",
            ));
        }
        Some(other) => other,
        None => caller,
    };
    let limit = list_limit(params.limit)?;
    let state_filter = valid_state(params.state.as_deref())?;
    let after = params.cursor.as_deref().map(decode_cursor).transpose()?;
    let filter = SessionFilter {
        harness: params.harness.as_deref(),
        state: state_filter,
    };
    let mut rows = SessionStore::new(state.db.clone())
        .list_for_user(user_id, &filter, after.as_ref(), limit)
        .await
        .map_err(|e| ApiError::internal("list harness sessions", e))?;
    let has_more = rows.len() as i64 > limit;
    rows.truncate(limit as usize);
    let next_cursor = rows.last().filter(|_| has_more).map(encode_cursor);
    let data: Vec<SessionView> = rows
        .into_iter()
        .map(|l| SessionView::new(l.row, l.harness_id))
        .collect();
    Ok(Json(json!({
        "data": data, "has_more": has_more, "next_cursor": next_cursor, "prev_cursor": null
    }))
    .into_response())
}

fn invalid_param(message: &'static str) -> ApiError {
    ApiError(StatusCode::BAD_REQUEST, "invalid_parameter", message)
}

fn list_limit(limit: Option<i64>) -> ApiResult<i64> {
    match limit {
        None => Ok(DEFAULT_LIST_LIMIT),
        Some(n) if (1..=MAX_LIST_LIMIT).contains(&n) => Ok(n),
        Some(_) => Err(invalid_param("limit must be between 1 and 200")),
    }
}

/// The `state` filter, if it names a runtime state.
fn valid_state(state: Option<&str>) -> ApiResult<Option<&'static str>> {
    let Some(state) = state else { return Ok(None) };
    [
        SessionRuntimeState::Parked,
        SessionRuntimeState::Starting,
        SessionRuntimeState::Running,
        SessionRuntimeState::Stopping,
    ]
    .into_iter()
    .map(SessionRuntimeState::as_str)
    .find(|s| *s == state)
    .map(Some)
    .ok_or_else(|| invalid_param("state must be parked, starting, running or stopping"))
}

/// An opaque cursor: the last row's sort key and id.
fn encode_cursor(last: &ListedSession) -> String {
    let raw = format!(
        "{}|{}",
        last.sort_at
            .to_rfc3339_opts(chrono::SecondsFormat::Micros, true),
        last.row.session_id
    );
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(raw)
}

fn decode_cursor(cursor: &str) -> ApiResult<(DateTime<Utc>, String)> {
    let bad = || invalid_param("cursor is not valid");
    let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(cursor)
        .map_err(|_| bad())?;
    let raw = String::from_utf8(raw).map_err(|_| bad())?;
    let (at, id) = raw.split_once('|').ok_or_else(bad)?;
    let at = DateTime::parse_from_rfc3339(at).map_err(|_| bad())?;
    Ok((at.with_timezone(&Utc), id.to_owned()))
}

/// Delete a session: its sandbox is released, its sandbox token revoked, the row removed. The
/// owner or a superuser may; anyone else, and an unknown id, gets 404. A session a turn (or a
/// suspend) holds is 409 `session_busy`: a running turn is not killed, the caller suspends first.
/// A session whose sandbox is still starting is deleted once the start ends (`start::wait_out`).
async fn delete_session(
    State(state): State<AppState>,
    claims: Claims,
    Path(id): Path<String>,
) -> ApiResult<Response> {
    let caller = claims.user_uuid().map_err(unauthorized)?;
    let store = SessionStore::new(state.db.clone());
    let row = store
        .get(&id)
        .await
        .map_err(|e| ApiError::internal("get harness session", e))?
        .filter(|row| claims.is_superuser || row.user_id == caller)
        .ok_or(ApiError::NOT_FOUND)?;
    let row = super::start::wait_out(&state, row)
        .await
        .map_err(|e| ApiError::internal("get harness session", e))?
        .ok_or(ApiError::NOT_FOUND)?;
    let deleted = store
        .delete_idle(&id, row.turn_fence)
        .await
        .map_err(|e| ApiError::internal("delete harness session", e))?;
    let Some(jti) = deleted else {
        return Err(if store.get(&id).await.ok().flatten().is_some() {
            busy_error()
        } else {
            ApiError::NOT_FOUND
        });
    };
    super::teardown::release_row_sandbox(&state, &row).await;
    if let Some(jti) = jti {
        super::teardown::revoke_deleted(&state, &id, &jti).await;
    }
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// Suspend the session's sandbox. A session with no sandbox, or one parked whose sandbox the
/// provider agrees is not running, has nothing to do (204), unless a turn is mid-start (409). A session a turn is using is `409`:
/// the claim is fenced on the turn counter, so a turn that starts meanwhile wins. A session
/// already being suspended is `202` while that claim's deadline holds; once it has lapsed the
/// claim is taken over and finished. A session whose sandbox is still starting is suspended once
/// the start ends (`start::wait_out`).
async fn suspend_session(
    State(state): State<AppState>,
    claims: Claims,
    Path(id): Path<String>,
) -> ApiResult<Response> {
    let (row, _) = owned_session(&state, &claims, &id).await?;
    let row = super::start::wait_out(&state, row)
        .await
        .map_err(|e| ApiError::internal("get harness session", e))?
        .ok_or(ApiError::NOT_FOUND)?;
    if let Some(done) = nothing_to_suspend(&row) {
        return Ok(done);
    }
    let handle = row
        .handle(state.sandbox.kind())
        .map_err(|e| ApiError::internal("session sandbox_ref", e))?;
    let store = SessionStore::new(state.db.clone());
    let claim = if row.runtime_state == "stopping" {
        store.reclaim_suspend(&id, row.turn_fence).await
    } else if row.runtime_state == "parked" {
        // Parked says nothing is running; only act if the provider says otherwise.
        if !sandbox_is_live(&state, &handle).await? {
            return Ok(StatusCode::NO_CONTENT.into_response());
        }
        claim_unaccounted(&store, &row, &handle).await
    } else {
        claim_suspend(&store, &row).await
    }
    .map_err(|e| ApiError::internal("claim suspend", e))?;
    let Some(claim) = claim else {
        return lost_claim_response(&store, &id).await;
    };
    let settled = Suspend::new(&state, &store, &row, &handle)
        .finish(claim)
        .await
        .map_err(|e| suspend_error(e, &id))?;
    Ok(if settled {
        StatusCode::NO_CONTENT.into_response()
    } else {
        suspending_response()
    })
}

/// The answer for a session with no sandbox to suspend: 409 while a turn is mid-start, else 204.
/// `None` when there is a sandbox to act on (or, for a parked row, to check).
fn nothing_to_suspend(row: &SessionRow) -> Option<Response> {
    if row.sandbox_ref.is_some() {
        return None;
    }
    Some(
        if row.runtime_state == "starting" && row.turn_owner.is_some() {
            busy()
        } else {
            StatusCode::NO_CONTENT.into_response()
        },
    )
}

/// How a failed suspend is reported: a settle (database) failure is ours, 500; a provider
/// failure is 502, with the cause logged and not echoed.
fn suspend_error(error: ReapError, session_id: &str) -> ApiError {
    match error {
        ReapError::Db(cause) => ApiError::internal("settle suspended session", cause),
        other => {
            tracing::error!(error = %other, %session_id, "suspend sandbox failed");
            ApiError(
                StatusCode::BAD_GATEWAY,
                "sandbox_error",
                "sandbox provider failed",
            )
        }
    }
}

/// Whether the provider reports the session's sandbox running or starting.
async fn sandbox_is_live(
    state: &AppState,
    handle: &nasiko_runtime::SandboxHandle,
) -> ApiResult<bool> {
    let status = state.sandbox.status(handle).await.map_err(|e| {
        tracing::error!(error = %e, "sandbox status failed");
        ApiError(
            StatusCode::BAD_GATEWAY,
            "sandbox_error",
            "sandbox provider failed",
        )
    })?;
    Ok(matches!(
        status,
        nasiko_runtime::SandboxState::Running | nasiko_runtime::SandboxState::Starting
    ))
}

/// The answer after losing the claim, from the row as it is now: another claimant is
/// suspending it (202), it is already parked (204), or a turn holds it (409).
async fn lost_claim_response(store: &SessionStore, id: &str) -> ApiResult<Response> {
    let fresh = store
        .get(id)
        .await
        .map_err(|e| ApiError::internal("re-read session", e))?;
    Ok(match fresh.as_ref().map(|r| r.runtime_state.as_str()) {
        Some("stopping") => suspending_response(),
        Some("parked") | None => StatusCode::NO_CONTENT.into_response(),
        Some(_) => busy(),
    })
}

fn busy_error() -> ApiError {
    ApiError(
        StatusCode::CONFLICT,
        "session_busy",
        "a turn is using this session",
    )
}

fn busy() -> Response {
    busy_error().into_response()
}

fn suspending_response() -> Response {
    (
        StatusCode::ACCEPTED,
        Json(json!({ "data": { "status": "suspending" } })),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hosts() -> Vec<String> {
        vec!["github.com".to_owned()]
    }

    fn code(r: ApiResult<Option<&str>>) -> Option<&'static str> {
        r.err().map(|e| e.1)
    }

    #[test]
    fn repo_on_an_allowed_host_over_https_or_ssh_is_accepted() {
        for url in ["https://github.com/o/r.git", "ssh://git@github.com/o/r.git"] {
            assert_eq!(valid_repo(Some(url), &hosts()).ok(), Some(Some(url)));
        }
        assert!(matches!(valid_repo(None, &hosts()), Ok(None)));
        assert!(matches!(valid_repo(Some("  "), &hosts()), Ok(None)));
    }

    #[test]
    fn repo_with_a_bad_scheme_host_credentials_or_shape_is_rejected() {
        for url in [
            "http://github.com/o/r",
            "git://github.com/o/r.git",
            "file:///etc/passwd",
            "https://evil.example/o/r",
            "https://user@github.com/o/r",
            "https://user:secret@github.com/o/r",
            "ssh://git:secret@github.com/o/r",
            "not a url",
        ] {
            assert_eq!(
                code(valid_repo(Some(url), &hosts())),
                Some("invalid_repo"),
                "{url}"
            );
        }
        let long = format!("https://github.com/{}", "a".repeat(MAX_REPO_LEN));
        assert_eq!(
            code(valid_repo(Some(&long), &hosts())),
            Some("invalid_repo")
        );
    }

    #[test]
    fn scp_style_repo_gets_the_ssh_url_hint() {
        let err = valid_repo(Some("git@github.com:o/r.git"), &hosts()).unwrap_err();
        assert_eq!(err.2, "use ssh://git@host/org/repo.git");
    }
}

//! The agent state routes. A handler takes its inputs already validated, makes one repository
//! call, and shapes the result; the rules live in the extractors and the repository.

use axum::extract::{DefaultBodyLimit, State};
use axum::http::StatusCode;
use axum::routing::{get, post, put};
use axum::{Json, Router, middleware};

use super::auth::{AgentAuth, AuthenticatedAgent, authenticate};
use super::error::StateError;
use super::page::{ListedRow, Page, build_page};
use super::payload::{BoundedPayload, PayloadCipher, sealed_bytes_bound};
use super::peer::{PeerLimit, limit_by_peer};
use super::precondition::Precondition;
use super::repo::{
    self, DeleteOutcome, Entry, KvQuery, NewEntry, NewTask, PutOutcome, Stamp, Store, TaskQuery,
};
use super::request::{KvKey, StateJson, StateQuery, TaskId};
use super::wire::{
    KvStamp, KvView, ListKvQuery, ListTasksQuery, PutKvRequest, PutTaskRequest, RegisterRequest,
    TaskStamp, TaskView,
};
use crate::state::AppState;

/// Room the transport-level body limit leaves beyond the value cap for the JSON around the value
/// (field names, `state`, `context_id`). The cap on the value itself is exact and enforced once
/// the request is read (`BoundedPayload`); this only bounds how much of a request is buffered
/// before that can be checked.
const ENVELOPE_ALLOWANCE_BYTES: usize = 16 * 1024;

/// The agent state API, to be nested under `/api`.
///
/// Mounted outside the session auth, like the MCP gateway: callers are deployed agents holding
/// their own credential, which the router authenticates and rate-limits itself.
pub fn router(state: &AppState) -> Router<AppState> {
    let auth = AgentAuth::new(
        state.db.clone(),
        state.config.agent_state_rate_limit_per_minute,
        state.genai_metrics.agent_ready_replicas(),
    );
    let peer_limit = PeerLimit::new(state.config.agent_state_rate_limit_per_minute);
    let transport_limit = state
        .config
        .agent_state_max_body_bytes
        .saturating_add(ENVELOPE_ALLOWANCE_BYTES);
    Router::new()
        .route("/agent-state/register", post(register))
        .route("/agent-state/tasks", get(list_tasks))
        .route(
            "/agent-state/tasks/{task_id}",
            put(put_task).get(get_task).delete(delete_task),
        )
        .route("/agent-state/kv", get(list_kv))
        .route(
            "/agent-state/kv/{key}",
            put(put_kv).get(get_kv).delete(delete_kv),
        )
        .method_not_allowed_fallback(method_not_allowed)
        .layer(DefaultBodyLimit::max(transport_limit))
        .layer(middleware::from_fn_with_state(auth, authenticate))
        .layer(middleware::from_fn_with_state(peer_limit, limit_by_peer))
}

/// A known route asked for with a method it does not take: the API's JSON error rather than the
/// framework's empty body. The framework still adds the `Allow` header.
async fn method_not_allowed() -> StateError {
    StateError::MethodNotAllowed
}

// ─── Registration ───────────────────────────────────────────────────────────

async fn register(
    State(state): State<AppState>,
    AuthenticatedAgent(agent_id): AuthenticatedAgent,
    StateJson(request): StateJson<RegisterRequest>,
) -> Result<StatusCode, StateError> {
    request.check()?;
    // Becoming durable can make the agent eligible to autoscale, and it was deployed while it
    // was not, so its bounds are re-applied now rather than at its next deploy.
    if request.stores_tasks() && repo::mark_durable(&state.db, agent_id).await? {
        state.agent_scaling.agent_became_durable(agent_id);
    }
    tracing::info!(
        %agent_id,
        client = ?request.client,
        capabilities = ?request.capabilities,
        "agent state store registered"
    );
    Ok(StatusCode::NO_CONTENT)
}

// ─── Tasks ──────────────────────────────────────────────────────────────────

async fn put_task(
    State(state): State<AppState>,
    AuthenticatedAgent(agent_id): AuthenticatedAgent,
    task_id: TaskId,
    precondition: Precondition,
    StateJson(request): StateJson<PutTaskRequest>,
) -> Result<Json<TaskStamp>, StateError> {
    request.check()?;
    let payload = BoundedPayload::new(&request.body, state.config.agent_state_max_body_bytes)?;
    let sealed_body =
        PayloadCipher::for_agent(agent_id)?.seal(Store::Tasks, task_id.as_str(), &payload);
    let task = NewTask {
        task_id: task_id.as_str(),
        context_id: request.context_id.as_deref(),
        state: &request.state,
        sealed_body: &sealed_body,
    };
    let outcome = repo::put_task(
        &state.db,
        agent_id,
        &task,
        precondition,
        state.config.agent_state_ttl_days,
    )
    .await?;
    Ok(Json(TaskStamp::new(
        task_id.into_string(),
        stored(outcome)?,
    )))
}

async fn get_task(
    State(state): State<AppState>,
    AuthenticatedAgent(agent_id): AuthenticatedAgent,
    task_id: TaskId,
) -> Result<Json<TaskView>, StateError> {
    let row = repo::get_task(&state.db, agent_id, task_id.as_str())
        .await?
        .ok_or(StateError::NotFound { what: "task" })?;
    let body =
        PayloadCipher::for_agent(agent_id)?.open(Store::Tasks, &row.task_id, &row.sealed_body)?;
    Ok(Json(row.into_view(body)))
}

async fn delete_task(
    State(state): State<AppState>,
    AuthenticatedAgent(agent_id): AuthenticatedAgent,
    task_id: TaskId,
    precondition: Precondition,
) -> Result<StatusCode, StateError> {
    let entry = Entry {
        agent_id,
        store: Store::Tasks,
        name: task_id.as_str(),
    };
    let ttl_days = state.config.agent_state_ttl_days;
    let outcome = repo::delete(&state.db, entry, precondition, ttl_days).await?;
    deleted(outcome, "task")
}

async fn list_tasks(
    State(state): State<AppState>,
    AuthenticatedAgent(agent_id): AuthenticatedAgent,
    StateQuery(query): StateQuery<ListTasksQuery>,
) -> Result<Json<Page<TaskView>>, StateError> {
    let query = TaskQuery::try_from(query)?;
    let budget = state.config.agent_state_list_bytes_budget;
    let fetch_bound = listing_fetch_bound(budget, query.page_size);
    let rows = repo::list_tasks(&state.db, agent_id, &query, fetch_bound).await?;
    let cipher = PayloadCipher::for_agent(agent_id)?;
    Ok(Json(build_page(rows, budget, |row| {
        cipher.open(Store::Tasks, &row.task_id, &row.sealed_body)
    })))
}

// ─── Key-value ──────────────────────────────────────────────────────────────

async fn put_kv(
    State(state): State<AppState>,
    AuthenticatedAgent(agent_id): AuthenticatedAgent,
    key: KvKey,
    precondition: Precondition,
    StateJson(request): StateJson<PutKvRequest>,
) -> Result<Json<KvStamp>, StateError> {
    let payload = BoundedPayload::new(&request.value, state.config.agent_state_max_body_bytes)?;
    let sealed_value = PayloadCipher::for_agent(agent_id)?.seal(Store::Kv, key.as_str(), &payload);
    let entry = NewEntry {
        key: key.as_str(),
        sealed_value: &sealed_value,
    };
    let outcome = repo::put_kv(
        &state.db,
        agent_id,
        &entry,
        precondition,
        state.config.agent_state_ttl_days,
    )
    .await?;
    Ok(Json(KvStamp::new(key.into_string(), stored(outcome)?)))
}

async fn get_kv(
    State(state): State<AppState>,
    AuthenticatedAgent(agent_id): AuthenticatedAgent,
    key: KvKey,
) -> Result<Json<KvView>, StateError> {
    let row = repo::get_kv(&state.db, agent_id, key.as_str())
        .await?
        .ok_or(StateError::NotFound { what: "key" })?;
    let value = PayloadCipher::for_agent(agent_id)?.open(Store::Kv, &row.key, &row.sealed_value)?;
    Ok(Json(row.into_view(value)))
}

async fn delete_kv(
    State(state): State<AppState>,
    AuthenticatedAgent(agent_id): AuthenticatedAgent,
    key: KvKey,
    precondition: Precondition,
) -> Result<StatusCode, StateError> {
    let entry = Entry {
        agent_id,
        store: Store::Kv,
        name: key.as_str(),
    };
    let ttl_days = state.config.agent_state_ttl_days;
    let outcome = repo::delete(&state.db, entry, precondition, ttl_days).await?;
    deleted(outcome, "key")
}

async fn list_kv(
    State(state): State<AppState>,
    AuthenticatedAgent(agent_id): AuthenticatedAgent,
    StateQuery(query): StateQuery<ListKvQuery>,
) -> Result<Json<Page<KvView>>, StateError> {
    let query = KvQuery::try_from(query)?;
    let budget = state.config.agent_state_list_bytes_budget;
    let fetch_bound = listing_fetch_bound(budget, query.page_size);
    let rows = repo::list_kv(&state.db, agent_id, &query, fetch_bound).await?;
    let cipher = PayloadCipher::for_agent(agent_id)?;
    Ok(Json(build_page(rows, budget, |row| {
        cipher.open(Store::Kv, &row.key, &row.sealed_value)
    })))
}

/// The stored bytes a listing may fetch ahead of a row: what the rows before it can take while
/// their payloads stay within `budget`. A listing considers at most `page_size + 1` rows, so at
/// most `page_size` precede any of them, and a row whose predecessors' payloads fit the budget is
/// always fetched. `build_page` still applies the budget itself to what was fetched. (A row that
/// does not decrypt counts here though `build_page` skips it, which can only shorten a page.)
fn listing_fetch_bound(budget: usize, page_size: i64) -> i64 {
    sealed_bytes_bound(budget, page_size)
}

// ─── Outcomes to responses ──────────────────────────────────────────────────

fn stored(outcome: PutOutcome) -> Result<Stamp, StateError> {
    match outcome {
        PutOutcome::Stored(stamp) => Ok(stamp),
        PutOutcome::VersionConflict { current_version } => {
            Err(StateError::VersionConflict { current_version })
        }
    }
}

fn deleted(outcome: DeleteOutcome, what: &'static str) -> Result<StatusCode, StateError> {
    match outcome {
        DeleteOutcome::Deleted => Ok(StatusCode::NO_CONTENT),
        DeleteOutcome::NotFound => Err(StateError::NotFound { what }),
        DeleteOutcome::VersionConflict { current_version } => {
            Err(StateError::VersionConflict { current_version })
        }
    }
}

#[cfg(test)]
mod tests {
    use chrono::{DateTime, Utc};

    use super::*;

    fn at(micros: i64) -> DateTime<Utc> {
        DateTime::from_timestamp_micros(micros).unwrap()
    }

    #[test]
    fn a_put_that_stored_is_its_stamp_and_a_conflict_carries_the_current_version() {
        let stamp = Stamp {
            version: 3,
            updated_at: at(1),
            expires_at: at(2),
        };
        assert_eq!(stored(PutOutcome::Stored(stamp)).unwrap().version, 3);
        assert!(matches!(
            stored(PutOutcome::VersionConflict { current_version: 9 }),
            Err(StateError::VersionConflict { current_version: 9 })
        ));
    }

    #[test]
    fn each_delete_outcome_maps_to_its_status() {
        assert_eq!(
            deleted(DeleteOutcome::Deleted, "task").unwrap(),
            StatusCode::NO_CONTENT
        );
        assert!(matches!(
            deleted(DeleteOutcome::NotFound, "task"),
            Err(StateError::NotFound { what: "task" })
        ));
        assert!(matches!(
            deleted(
                DeleteOutcome::VersionConflict { current_version: 2 },
                "task"
            ),
            Err(StateError::VersionConflict { current_version: 2 })
        ));
    }
}

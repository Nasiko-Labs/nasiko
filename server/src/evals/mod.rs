//! `/api/evals/*` — replay evals: coding sessions a person picked become a private,
//! SWE-bench-style benchmark, replayed on other models and costed by the router.
//!
//! The server does the whole pipeline (`oss/docs/EVALS.md`): it links each session to the
//! commit it produced on GitHub (the repository comes from the session), builds and tests each
//! task in a container, screens it with reviewer models, and runs the picked tasks on the picked
//! models (each one of the caller's LLM configs), grading every attempt. The work is a durable
//! job queue (`jobs`) on a container backend the composition root installs (`runner`).
//!
//! - harnesses, sessions, datasets, tasks, runs: `api`
//! - `GET /evals/usage?run=<run>` — the caller's own attempts whose tag starts with `<run>:`,
//!   one entry per attempt, in tag order, cursor-paged.
//! - `POST /evals/price` — the cost of a token count the router never saw (the original
//!   session an eval replays), priced by the same engine the router prices with.
//!
//! Responses use the `{data}` envelopes; errors are `{error, code}`.

pub(crate) mod api;
mod attempt;
mod build;
/// Public within the crate: code-review evals reuses `Forge` for PR/diff/file reads instead of
/// re-implementing GitHub access.
pub(crate) mod forge;
mod harvest;
mod identity;
pub mod jobs;
/// Public within the crate: code-review evals reuses `complete` for its single-shot review calls
/// instead of the harness/container pipeline this module otherwise runs.
pub(crate) mod llm;
mod price;
mod review;
pub mod runner;
mod sessions;
mod steps;
mod store;
mod usage;

use axum::{
    Json, Router,
    extract::{Query, State, rejection::JsonRejection},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::Serialize;
use serde_json::json;

use crate::auth::Claims;
use crate::state::AppState;

pub use api::{
    CreateDataset, CreateRun, DatasetSummary, DatasetView, HarnessList, ModelView, OriginalView,
    Progress, ResultView, ReviewBody, RunSummary, RunView, SessionList, TaskView,
};
pub use price::{PriceRequest, PricedTokens};
pub use sessions::{Harness, Ineligible, SessionSummary};
pub use store::RunModel;
pub use usage::{AttemptUsage, UsageParams};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/evals/usage", get(get_usage))
        .route("/evals/price", post(post_price))
        .route("/evals/harnesses", get(api::list_harnesses))
        .route("/evals/harnesses/{id}/sessions", get(api::list_sessions))
        .route(
            "/evals/datasets",
            get(api::list_datasets).post(api::create_dataset),
        )
        .route(
            "/evals/datasets/{id}",
            get(api::get_dataset).delete(api::delete_dataset),
        )
        .route("/evals/datasets/{id}/runs", post(api::create_run))
        .route("/evals/tasks/{id}", get(api::get_task))
        .route("/evals/tasks/{id}/review", post(api::review_task))
        .route(
            "/evals/specs/{owner}/{repo}",
            get(api::get_spec).put(api::put_spec),
        )
        .route("/evals/runs", get(api::list_runs))
        .route(
            "/evals/runs/{id}",
            get(api::get_run).delete(api::delete_run),
        )
        .route("/evals/runs/{id}/cancel", post(api::cancel_run))
        .route(
            "/evals/runs/{id}/attempts/{attempt_id}",
            get(api::get_attempt),
        )
}

/// Starts the eval worker when this server runs evals. The composition root installs the
/// batch runner first (`runner`).
pub fn spawn_worker(state: &AppState) {
    if state.config.evals.enabled {
        jobs::spawn_worker(state.clone());
    }
}

/// A failed request: status, stable machine slug, human message.
///
/// TODO NAS-1098: replace with the shared `dto::ApiError` once that crate exists.
#[derive(Debug, PartialEq, Eq)]
struct ApiError(StatusCode, &'static str, &'static str);

impl ApiError {
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

/// The list envelope (`oss/docs/API_CONVENTIONS.md` §1).
#[derive(Serialize, utoipa::ToSchema)]
pub struct UsagePage {
    pub data: Vec<AttemptUsage>,
    pub has_more: bool,
    pub next_cursor: Option<String>,
    pub prev_cursor: Option<String>,
}

/// The single-object envelope.
#[derive(Serialize, utoipa::ToSchema)]
pub struct PriceEnvelope {
    pub data: PricedTokens,
}

#[utoipa::path(
    get,
    path = "/api/evals/usage",
    tag = "evals",
    params(UsageParams),
    responses(
        (status = 200, description = "The caller's attempts of the run, in tag order", body = UsagePage),
        (status = 400, description = "`invalid_run`, `invalid_cursor` or `invalid_limit`"),
    ),
)]
pub async fn get_usage(
    State(state): State<AppState>,
    claims: Claims,
    Query(params): Query<UsageParams>,
) -> Response {
    let user_id = match claims.user_uuid() {
        Ok(id) => id,
        Err(rejection) => return rejection.into_response(),
    };
    let query = match usage::RunQuery::parse(params) {
        Ok(query) => query,
        Err(error) => return error.into_response(),
    };
    match usage::attempts(&state.db, user_id, &query).await {
        Ok(page) => Json(UsagePage {
            data: page.attempts,
            has_more: page.next_cursor.is_some(),
            next_cursor: page.next_cursor,
            prev_cursor: None,
        })
        .into_response(),
        Err(e) => ApiError::internal("eval usage read failed", e).into_response(),
    }
}

#[utoipa::path(
    post,
    path = "/api/evals/price",
    tag = "evals",
    request_body = PriceRequest,
    responses(
        (status = 200, description = "The tokens' cost at today's rates", body = PriceEnvelope),
        (status = 400, description = "`invalid_model`: the model is empty"),
        (status = 422, description = "`invalid_body`: not a price request"),
    ),
)]
pub async fn post_price(
    State(state): State<AppState>,
    _claims: Claims,
    body: Result<Json<PriceRequest>, JsonRejection>,
) -> Response {
    let Ok(Json(request)) = body else {
        return ApiError(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_body",
            "body must be {provider?, model, input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens}",
        )
        .into_response();
    };
    match price::price(&state.pricing, request).await {
        Ok(priced) => Json(PriceEnvelope { data: priced }).into_response(),
        Err(error) => error.into_response(),
    }
}

//! `/api/evals/code-review/*` — code review evals: one GitHub pull request read at a fixed
//! base/head commit, reviewed by chosen LLM configs with an identical prompt, and (once a person
//! has added validated reference findings) graded by deterministic finding-matching.
//!
//! This reuses two pieces of the replay-evals pipeline (`crate::evals`) rather than its
//! harness/container machinery, which a one-shot review does not need: `evals::llm::complete` for
//! a single router-attributed chat completion, and `evals::forge::Forge` for GitHub access. A
//! review run is not durably queued like a replay run — see
//! `oss/docs/proposals/evals/CODE_REVIEW_EVALS.md` for the grading methodology and the explicit
//! list of what this slice does and does not do.
//!
//! - tasks (import a PR, add reference findings): `api`
//! - runs (review it with chosen models, read graded or ungraded-preview results): `api`

pub(crate) mod api;
mod grader;
mod prompt;
mod store;

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::json;

use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/evals/code-review/tasks",
            get(api::list_tasks).post(api::create_task),
        )
        .route("/evals/code-review/tasks/{id}", get(api::get_task))
        .route(
            "/evals/code-review/tasks/{id}/reference-findings",
            post(api::add_reference_findings),
        )
        .route("/evals/code-review/tasks/{id}/runs", post(api::create_run))
        .route("/evals/code-review/runs/{id}", get(api::get_run))
        .route("/evals/code-review/universe", get(api::get_universe))
}

/// A failed request: status, stable machine slug, human message. Duplicated from
/// `evals::ApiError` rather than shared, following the same `TODO NAS-1098` the replay-evals
/// module already carries (a shared `dto::ApiError` crate does not exist yet).
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct ApiError(pub StatusCode, pub &'static str, pub &'static str);

impl ApiError {
    /// A 5xx whose cause is logged, never echoed.
    pub(crate) fn internal(context: &str, cause: impl std::fmt::Display) -> Self {
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

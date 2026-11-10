//! The supervisor's HTTP API. Every route but `/healthz` requires the bearer.

use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{Path, Query, Request, State};
use axum::http::StatusCode;
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;

use super::Supervisor;
use super::session::{self, EnsureRequest, SessionError, SessionView};
use crate::server::bearer_matches;

pub(super) fn router(sup: Arc<Supervisor>) -> Router {
    let sessions = Router::new()
        .route("/sessions", get(list_sessions))
        .route(
            "/sessions/{id}",
            get(get_session).put(put_session).delete(delete_session),
        )
        .route_layer(middleware::from_fn_with_state(
            Arc::clone(&sup),
            require_bearer,
        ));
    Router::new()
        .route("/healthz", get(healthz))
        .merge(sessions)
        .with_state(sup)
}

async fn require_bearer(
    State(sup): State<Arc<Supervisor>>,
    request: Request,
    next: Next,
) -> Response {
    if !bearer_matches(request.headers(), &sup.config.token) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    next.run(request).await
}

/// `GET /healthz`: the pod's readiness probe, so it never needs the bearer.
async fn healthz(State(sup): State<Arc<Supervisor>>) -> Response {
    Json(json!({"sessions": sup.running_count()})).into_response()
}

/// `PUT /sessions/{id}` `{"port", "shim_token"?}`: start the session in `<root>/<id>` or
/// confirm it runs.
async fn put_session(
    State(sup): State<Arc<Supervisor>>,
    Path(id): Path<String>,
    body: Bytes,
) -> Response {
    // Parsed by hand so a bad body is a 400 with the reason, like every other refusal here
    // (axum's `Json` extractor would answer 415 or 422 with its own text).
    let request = match serde_json::from_slice::<EnsureRequest>(&body) {
        Ok(request) => request,
        Err(err) => return SessionError::Invalid(format!("invalid body: {err}")).into_response(),
    };
    match session::ensure(&sup, &id, &request).await {
        Ok(placed) => Json(placed).into_response(),
        Err(err) => failure(&id, err),
    }
}

#[derive(Deserialize)]
struct DeleteParams {
    #[serde(default)]
    purge: bool,
}

/// `DELETE /sessions/{id}`: stop the session (suspend); with `?purge=true` also remove its
/// directory and forget it (release). An unknown session is already stopped, and its purge still
/// removes `<root>/<id>`.
async fn delete_session(
    State(sup): State<Arc<Supervisor>>,
    Path(id): Path<String>,
    Query(params): Query<DeleteParams>,
) -> Response {
    let outcome = if params.purge {
        session::purge(&sup, &id).await
    } else {
        session::stop(&sup, &id).await
    };
    match outcome {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(err) => failure(&id, err),
    }
}

/// `GET /sessions`.
async fn list_sessions(State(sup): State<Arc<Supervisor>>) -> Json<Vec<SessionView>> {
    Json(session::view_all(&sup).await)
}

/// `GET /sessions/{id}`.
async fn get_session(State(sup): State<Arc<Supervisor>>, Path(id): Path<String>) -> Response {
    match session::view(&sup, &id).await {
        Some(view) => Json(view).into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(json!({"error": format!("no session {id}")})),
        )
            .into_response(),
    }
}

/// Logs a failure on this pod (a refusal is the caller's to report) and answers it.
fn failure(id: &str, err: SessionError) -> Response {
    if let SessionError::Failed(reason) = &err {
        tracing::error!(session = %id, %reason, "session request failed");
    }
    err.into_response()
}

impl IntoResponse for SessionError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            SessionError::Invalid(message) => (StatusCode::BAD_REQUEST, message),
            SessionError::Conflict(message) => (StatusCode::CONFLICT, message),
            SessionError::Failed(message) => (StatusCode::INTERNAL_SERVER_ERROR, message),
        };
        (status, Json(json!({"error": message}))).into_response()
    }
}

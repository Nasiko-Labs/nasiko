//! The control routes: `configure` (handshake mode), `status`, and `stats`.
//!
//! `status` reports `env_hash` (the stored configuration), `running_env_hash` (what the live child
//! was launched with; null while waiting or exited) and `child_generation` (spawn counter), so a
//! caller can tell when a restart has applied a reconfigure.

use std::sync::Arc;

use axum::Router;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use serde_json::json;

use crate::handshake::{Applied, Configure, Mode};
use crate::procstats;
use crate::server::{Relay, authorized};

/// The shim's view of the child, as reported by `/healthz` and `/control/status`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ChildState {
    Waiting,
    Running,
    Exited,
}

impl ChildState {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            ChildState::Waiting => "waiting",
            ChildState::Running => "running",
            ChildState::Exited => "exited",
        }
    }
}

/// The child's state: exited once it is gone, waiting until the first spawn.
pub(crate) fn child_state(relay: &Relay) -> ChildState {
    if relay.has_exited() {
        ChildState::Exited
    } else if relay.handshake().is_spawned() {
        ChildState::Running
    } else {
        ChildState::Waiting
    }
}

pub(crate) fn routes() -> Router<Arc<Relay>> {
    Router::new()
        .route("/control/configure", post(configure))
        .route("/control/status", get(status))
        .route("/control/stats", get(stats))
}

/// `POST /control/configure`.
async fn configure(State(relay): State<Arc<Relay>>, headers: HeaderMap, body: Bytes) -> StatusCode {
    let handshake = relay.handshake();
    if handshake.mode() == Mode::Env {
        return StatusCode::CONFLICT;
    }
    // Once a bearer is set it is checked before the body is looked at.
    if !authorized(&relay, &headers) {
        return StatusCode::UNAUTHORIZED;
    }
    let Ok(configure) = serde_json::from_slice::<Configure>(&body) else {
        return StatusCode::BAD_REQUEST;
    };
    if configure.token.is_empty() || !handshake.command_available(&configure) {
        return StatusCode::BAD_REQUEST;
    }
    let candidate = configure.token.clone();
    // Claiming the bearer and checking it are one step, so two first calls cannot both win.
    if !relay.claim_or_authorize(&headers, &candidate) {
        return StatusCode::UNAUTHORIZED;
    }
    if relay.token().as_deref() != Some(candidate.as_str()) {
        tracing::warn!("configure carried a different token; rotation is not supported, ignored");
    }
    let applied = handshake.apply(configure);
    match applied {
        Applied::First => tracing::info!("shim configured"),
        Applied::Changed => tracing::info!("new configuration stored; applies on next restart"),
        Applied::Unchanged => {}
    }
    // A child that is gone (or never started) is (re)started from the stored configuration.
    if applied == Applied::First || relay.has_exited() {
        handshake.want_respawn();
    }
    StatusCode::NO_CONTENT
}

/// `GET /control/status`.
async fn status(State(relay): State<Arc<Relay>>, headers: HeaderMap) -> Response {
    if !authorized(&relay, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let handshake = relay.handshake();
    let child = child_state(&relay);
    axum::Json(json!({
        "mode": handshake.mode().as_str(),
        "configured": handshake.is_configured(),
        // `env_hash` is the stored configuration; the running child may still hold an older one.
        "env_hash": handshake.env_hash(),
        "running_env_hash": (child == ChildState::Running)
            .then(|| handshake.running_env_hash())
            .flatten(),
        "child_generation": handshake.generation(),
        "child": child.as_str(),
        "evicting": handshake.is_evicting(),
        "workspace": handshake.mount().display().to_string(),
    }))
    .into_response()
}

/// `GET /control/stats`. `processes` is `null` when `/proc` cannot be read.
async fn stats(State(relay): State<Arc<Relay>>, headers: HeaderMap) -> Response {
    if !authorized(&relay, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let root = std::process::id();
    let Ok(sample) = tokio::task::spawn_blocking(move || procstats::sample(root)).await else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    let cpu_millicores = relay.sampler().record(sample);
    axum::Json(json!({
        "processes": sample.processes,
        "cpu_millicores": cpu_millicores,
        "memory_bytes": sample.memory_bytes,
        "sampled_at": chrono::Utc::now().to_rfc3339(),
        "evicting": relay.handshake().is_evicting(),
    }))
    .into_response()
}

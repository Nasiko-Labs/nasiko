//! Whether a user may open a harness session, or start a turn on one.
//!
//! A session can cost a distribution more than its own sandbox: where each user's sessions of a
//! harness run in an agent of that user's, the first one adds an agent, which the distribution's
//! agent limits count. The open-source admission, [`AdmitEverySession`], admits everything; a
//! distribution with such limits installs its own on [`crate::state::AppState`] at its
//! composition root.
//!
//! Every caller asks before it writes anything for the session or its turn (no session row, no
//! transcript, no flow), and answers a refusal in its own protocol: the session API with the
//! refusal's status and body, an A2A request with a JSON-RPC error carrying the body.

use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};
use uuid::Uuid;

/// The JSON-RPC error code of an A2A request the session admission refused.
pub const SESSION_REFUSED: i64 = -32610;

/// Why a session may not open or run: the status and JSON body the refusal is answered with.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionRefusal {
    pub status: StatusCode,
    pub body: Value,
}

impl SessionRefusal {
    /// The answer to the A2A request `id`: a JSON-RPC error whose message is the body's `error`
    /// (else its `message`) and whose `data` is the body itself, so an A2A client reads the same
    /// code, limits and next steps the session API answers with.
    pub fn json_rpc_error(&self, id: &Value) -> Value {
        json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": { "code": SESSION_REFUSED, "message": self.message(), "data": self.body },
        })
    }

    fn message(&self) -> &str {
        ["error", "message"]
            .into_iter()
            .find_map(|key| self.body.get(key)?.as_str())
            .unwrap_or("the session was refused")
    }
}

impl SessionRefusal {
    /// Coding sessions are switched off on this server (`HARNESS_SESSIONS_ENABLED`).
    pub fn sessions_disabled() -> Self {
        Self {
            status: StatusCode::FORBIDDEN,
            body: json!({
                "error": "coding sessions are not enabled on this server",
                "code": "harness_sessions_disabled",
            }),
        }
    }
}

/// Whether `user_id` is refused a session of `harness_id`, or a turn on one: always while coding
/// sessions are switched off on this server, whatever the installed admission says; otherwise
/// that admission's answer. Every caller asks this, not the admission directly.
pub async fn refuse_session(
    state: &crate::state::AppState,
    user_id: Uuid,
    harness_id: &str,
) -> Option<SessionRefusal> {
    if !state.config.harness_sessions_enabled {
        return Some(SessionRefusal::sessions_disabled());
    }
    state.session_admission.refuse(user_id, harness_id).await
}

/// The answer on the session API: the status and body as they stand.
impl IntoResponse for SessionRefusal {
    fn into_response(self) -> Response {
        (self.status, Json(self.body)).into_response()
    }
}

#[async_trait]
pub trait SessionAdmission: Send + Sync {
    /// `Some(refusal)` refuses `user_id` a session of `harness_id`. Asked when a session is
    /// opened and before each turn on one, ahead of anything written for either.
    async fn refuse(&self, user_id: Uuid, harness_id: &str) -> Option<SessionRefusal>;
}

/// The open-source admission: never refuses.
pub struct AdmitEverySession;

#[async_trait]
impl SessionAdmission for AdmitEverySession {
    async fn refuse(&self, _user_id: Uuid, _harness_id: &str) -> Option<SessionRefusal> {
        None
    }
}

/// Holds a `SessionAdmission` behind a lock so a composition root can install its own after
/// `AppState` has been cloned into the routers, for the reason `SwappableAgentWake` exists: an
/// assignment would reach only the clones taken after it.
pub struct SwappableSessionAdmission(RwLock<Arc<dyn SessionAdmission>>);

impl SwappableSessionAdmission {
    pub fn new(initial: Arc<dyn SessionAdmission>) -> Self {
        Self(RwLock::new(initial))
    }

    /// Replaces the admission for every clone, past and future, of the `AppState` this cell
    /// lives in.
    pub fn install(&self, new: Arc<dyn SessionAdmission>) {
        *self.0.write().expect("session admission lock poisoned") = new;
    }
}

#[async_trait]
impl SessionAdmission for SwappableSessionAdmission {
    async fn refuse(&self, user_id: Uuid, harness_id: &str) -> Option<SessionRefusal> {
        let current = self
            .0
            .read()
            .expect("session admission lock poisoned")
            .clone();
        current.refuse(user_id, harness_id).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at_the_cap() -> SessionRefusal {
        SessionRefusal {
            status: StatusCode::PAYMENT_REQUIRED,
            body: json!({ "error": "You've reached 2 cloud agents", "code": "plan_agent_limit_reached" }),
        }
    }

    struct Refusing;

    #[async_trait]
    impl SessionAdmission for Refusing {
        async fn refuse(&self, _user_id: Uuid, _harness_id: &str) -> Option<SessionRefusal> {
            Some(at_the_cap())
        }
    }

    #[tokio::test]
    async fn the_open_source_admission_admits_every_session() {
        assert!(
            AdmitEverySession
                .refuse(Uuid::new_v4(), "opencode")
                .await
                .is_none()
        );
    }

    /// A clone taken before the composition root installs its admission still asks it.
    #[tokio::test]
    async fn install_is_visible_to_a_clone_taken_before_the_install_call() {
        let cell = Arc::new(SwappableSessionAdmission::new(Arc::new(AdmitEverySession)));
        let earlier = cell.clone();
        cell.install(Arc::new(Refusing));
        assert_eq!(
            earlier.refuse(Uuid::new_v4(), "opencode").await,
            Some(at_the_cap())
        );
    }

    #[test]
    fn an_a2a_refusal_is_a_json_rpc_error_carrying_the_body() {
        let refusal = at_the_cap();
        assert_eq!(
            refusal.json_rpc_error(&json!("7")),
            json!({
                "jsonrpc": "2.0",
                "id": "7",
                "error": {
                    "code": SESSION_REFUSED,
                    "message": "You've reached 2 cloud agents",
                    "data": refusal.body,
                },
            })
        );
        let bare = SessionRefusal {
            status: StatusCode::PAYMENT_REQUIRED,
            body: json!({ "message": "no", "code": "x" }),
        };
        assert_eq!(bare.json_rpc_error(&Value::Null)["error"]["message"], "no");
    }

    #[test]
    fn a_session_api_refusal_keeps_its_status() {
        assert_eq!(
            at_the_cap().into_response().status(),
            StatusCode::PAYMENT_REQUIRED
        );
    }
}

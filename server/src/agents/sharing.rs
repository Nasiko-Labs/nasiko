//! The refusal every share route gives a harness agent: one that would make it public, create an
//! `agent_grants` row for it, or transfer its ownership. A harness agent belongs to one user
//! (`nasiko_auth::sharing`). Making one private is refused only for the platform's own harness
//! agents ([`crate::sessions::PLATFORM_HARNESS_AGENT`]), whose catalog rows stay public; a user's
//! own harness agent may always be made private. Revoking a grant is always allowed. Both editions'
//! share routes call [`ensure_shareable`] (or, to make an agent private,
//! [`ensure_may_be_made_private`]) after their own authorization check, so a caller who may not
//! manage the agent still gets that answer.

use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

/// The `code` of the refusal (API_CONVENTIONS §2).
const NOT_SHAREABLE_CODE: &str = "agent_not_shareable";
const NOT_SHAREABLE_MESSAGE: &str = "harness agents belong to one user and cannot be shared";
const PLATFORM_HARNESS_MESSAGE: &str = "the platform's harness agents stay public";

/// Why a share was refused.
#[derive(Debug)]
pub enum ShareRefusal {
    /// The agent is a harness agent.
    HarnessAgent,
    /// The agent is one of the platform's harness agents, which stay public.
    PlatformHarnessAgent,
    /// The agent's kind could not be read.
    Lookup(sqlx::Error),
}

impl IntoResponse for ShareRefusal {
    fn into_response(self) -> Response {
        match self {
            Self::HarnessAgent => conflict(NOT_SHAREABLE_MESSAGE),
            Self::PlatformHarnessAgent => conflict(PLATFORM_HARNESS_MESSAGE),
            Self::Lookup(e) => {
                tracing::error!(error = %e, "share check: reading the agent's kind failed");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({ "error": "internal error", "code": "internal_error" })),
                )
                    .into_response()
            }
        }
    }
}

fn conflict(message: &str) -> Response {
    (
        StatusCode::CONFLICT,
        Json(json!({ "error": message, "code": NOT_SHAREABLE_CODE })),
    )
        .into_response()
}

/// Refuse to share `agent_id` when it is a harness agent. An agent that does not exist (or is
/// deleted) passes, so each route keeps reporting that case as it always has.
pub async fn ensure_shareable(db: &PgPool, agent_id: Uuid) -> Result<(), ShareRefusal> {
    let kind: Option<String> =
        sqlx::query_scalar("SELECT workload_kind FROM agents WHERE id = $1 AND deleted_at IS NULL")
            .bind(agent_id)
            .fetch_optional(db)
            .await
            .map_err(ShareRefusal::Lookup)?;
    refusal_for(kind.as_deref())
}

/// The decision for an agent of `workload_kind` (`None`: no such agent).
fn refusal_for(workload_kind: Option<&str>) -> Result<(), ShareRefusal> {
    match workload_kind {
        Some(kind) if !nasiko_auth::sharing::is_shareable(kind) => Err(ShareRefusal::HarnessAgent),
        _ => Ok(()),
    }
}

/// Refuse to make `agent_id` private when it is one of the platform's harness agents
/// ([`crate::sessions::PLATFORM_HARNESS_AGENT`]). Any other agent may be made private, a user's own
/// harness agent included (it is never public to begin with). An agent that does not exist (or is
/// deleted) passes, so each route keeps reporting that case as it always has.
pub async fn ensure_may_be_made_private(db: &PgPool, agent_id: Uuid) -> Result<(), ShareRefusal> {
    let platform_row: Option<bool> = sqlx::query_scalar(&format!(
        "SELECT ({}) IS TRUE FROM agents WHERE id = $1 AND deleted_at IS NULL",
        crate::sessions::PLATFORM_HARNESS_AGENT
    ))
    .bind(agent_id)
    .fetch_optional(db)
    .await
    .map_err(ShareRefusal::Lookup)?;
    private_refusal_for(platform_row)
}

/// The decision on making an agent private (`None`: no such agent).
fn private_refusal_for(platform_harness_agent: Option<bool>) -> Result<(), ShareRefusal> {
    match platform_harness_agent {
        Some(true) => Err(ShareRefusal::PlatformHarnessAgent),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_harness_agent_is_refused() {
        assert!(matches!(
            refusal_for(Some("harness")),
            Err(ShareRefusal::HarnessAgent)
        ));
    }

    #[test]
    fn a_framework_agent_and_a_missing_one_pass() {
        assert!(refusal_for(Some("agent")).is_ok());
        assert!(refusal_for(None).is_ok());
    }

    #[test]
    fn only_a_platform_harness_agent_may_not_be_made_private() {
        assert!(matches!(
            private_refusal_for(Some(true)),
            Err(ShareRefusal::PlatformHarnessAgent)
        ));
        assert!(private_refusal_for(Some(false)).is_ok());
        assert!(private_refusal_for(None).is_ok());
    }

    #[tokio::test]
    async fn the_platform_refusal_is_a_409_with_the_same_code() {
        let response = ShareRefusal::PlatformHarnessAgent.into_response();
        assert_eq!(response.status(), StatusCode::CONFLICT);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["code"], NOT_SHAREABLE_CODE);
        assert_eq!(body["error"], PLATFORM_HARNESS_MESSAGE);
    }

    #[tokio::test]
    async fn the_refusal_is_a_409_with_the_stable_code() {
        let response = ShareRefusal::HarnessAgent.into_response();
        assert_eq!(response.status(), StatusCode::CONFLICT);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["code"], NOT_SHAREABLE_CODE);
        assert_eq!(body["error"], NOT_SHAREABLE_MESSAGE);
    }
}

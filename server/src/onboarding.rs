//! `/api/me/onboarding` — first-run onboarding state for the calling user.
//!
//! A user is a first-time user until they pick a persona. The persona only
//! records who the user is (a developer, a finance person); which dashboard
//! that maps to is a UI decision, so no route or URL is stored or returned
//! here. It is not an access grant either — RBAC stays on `users.role`.
//!
//! Not superuser-gated — a user owns their own onboarding state. A persona
//! outside the enum never reaches the handler: Axum's `Json<_>` extractor
//! rejects it during deserialization (HTTP 422).

use axum::{Json, Router, extract::State, http::StatusCode, response::IntoResponse, routing::get};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

use crate::auth::Claims;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new().route(
        "/me/onboarding",
        get(get_onboarding).patch(update_onboarding),
    )
}

/// Who the user is, as picked during onboarding. Mirrors the Postgres
/// `user_persona` enum (migration 0048) — keep the two lists identical, or a
/// value the database holds will fail to decode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "user_persona", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum Persona {
    Developer,
    PlatformEngineer,
    /// FinOps / finance.
    Finance,
    EngineeringManager,
    ProductManager,
    DataAnalyst,
    SupportLead,
    /// SRE / on-call.
    Sre,
    Leadership,
}

#[derive(Debug, FromRow)]
struct OnboardingRow {
    persona: Option<Persona>,
    onboarding_completed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Serialize)]
struct OnboardingResponse {
    is_first_time_user: bool,
    persona: Option<Persona>,
}

impl From<OnboardingRow> for OnboardingResponse {
    fn from(row: OnboardingRow) -> Self {
        Self {
            is_first_time_user: row.onboarding_completed_at.is_none(),
            persona: row.persona,
        }
    }
}

#[derive(Debug, Deserialize)]
struct OnboardingUpdate {
    persona: Persona,
}

async fn get_onboarding(State(state): State<AppState>, claims: Claims) -> impl IntoResponse {
    let user_id = match claims.user_uuid() {
        Ok(id) => id,
        Err(e) => return e.into_response(),
    };

    let result = sqlx::query_as::<_, OnboardingRow>(
        "SELECT persona, onboarding_completed_at FROM users \
         WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(user_id)
    .fetch_optional(&state.db)
    .await;

    onboarding_response(result, "get_onboarding")
}

/// Saves the persona. The first call also completes onboarding; later calls
/// only change the persona and keep the original completion time.
async fn update_onboarding(
    State(state): State<AppState>,
    claims: Claims,
    Json(body): Json<OnboardingUpdate>,
) -> impl IntoResponse {
    let user_id = match claims.user_uuid() {
        Ok(id) => id,
        Err(e) => return e.into_response(),
    };

    let result = sqlx::query_as::<_, OnboardingRow>(
        "UPDATE users SET persona = $1, \
             onboarding_completed_at = COALESCE(onboarding_completed_at, now()) \
         WHERE id = $2 AND deleted_at IS NULL \
         RETURNING persona, onboarding_completed_at",
    )
    .bind(body.persona)
    .bind(user_id)
    .fetch_optional(&state.db)
    .await;

    onboarding_response(result, "update_onboarding")
}

fn onboarding_response(
    result: Result<Option<OnboardingRow>, sqlx::Error>,
    handler: &str,
) -> axum::response::Response {
    match result {
        Ok(Some(row)) => Json(OnboardingResponse::from(row)).into_response(),
        Ok(None) => (StatusCode::NOT_FOUND, "user not found").into_response(),
        Err(e) => {
            tracing::error!(%e, handler, "onboarding: db error");
            (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_without_completion_time_is_first_time() {
        let response = OnboardingResponse::from(OnboardingRow {
            persona: None,
            onboarding_completed_at: None,
        });
        assert!(response.is_first_time_user);
        assert_eq!(response.persona, None);
    }

    #[test]
    fn user_with_completion_time_is_not_first_time() {
        let response = OnboardingResponse::from(OnboardingRow {
            persona: Some(Persona::Finance),
            onboarding_completed_at: Some(Utc::now()),
        });
        assert!(!response.is_first_time_user);
        assert_eq!(response.persona, Some(Persona::Finance));
    }

    const ALL_PERSONAS: [Persona; 9] = [
        Persona::Developer,
        Persona::PlatformEngineer,
        Persona::Finance,
        Persona::EngineeringManager,
        Persona::ProductManager,
        Persona::DataAnalyst,
        Persona::SupportLead,
        Persona::Sre,
        Persona::Leadership,
    ];

    #[test]
    fn persona_uses_snake_case_wire_names() {
        let persona: Persona = serde_json::from_str("\"platform_engineer\"").unwrap();
        assert_eq!(persona, Persona::PlatformEngineer);
        assert_eq!(serde_json::to_string(&Persona::Sre).unwrap(), "\"sre\"");
        assert!(serde_json::from_str::<Persona>("\"admin\"").is_err());
    }

    /// The Rust enum and the Postgres enum must list the same values, or a
    /// stored persona fails to decode (500) or a valid one fails to insert.
    #[test]
    fn persona_matches_migration_enum() {
        let migration = include_str!("../../migrations/0051_user_onboarding.sql");
        let body = migration
            .split_once("user_persona AS ENUM (")
            .and_then(|(_, rest)| rest.split_once(')'))
            .map(|(values, _)| values)
            .expect("migration defines user_persona");
        let in_db: Vec<&str> = body
            .split(',')
            .map(|v| v.trim().trim_matches('\''))
            .collect();

        let in_rust: Vec<String> = ALL_PERSONAS
            .iter()
            .map(|p| {
                serde_json::to_value(p)
                    .unwrap()
                    .as_str()
                    .unwrap()
                    .to_owned()
            })
            .collect();

        assert_eq!(in_db, in_rust);
    }
}

//! `POST /api/assessments` — the free TokenOps assessment form (`/assessment` in the UI).
//!
//! The server checks a submission, adds who sent it, and forwards it to the Nasiko
//! assessment intake (`ASSESSMENT_INTAKE_URL`), which stores it. The server holds no
//! storage credential of its own on purpose: the desktop app runs this server on the
//! user's computer from a public installer, so any key it carried would be public. With
//! no intake configured the route answers 503 and the UI falls back to its
//! copy-into-the-booking-note step.
//!
//! `POST /api/assessments/{id}/booking` — the Calendly booking made on the form's last step
//! (the page hears Calendly's `calendly.event_scheduled`). Forwarded to the same intake URL as
//! `{"booking": {lead_id, user_id, event_uri, invitee_uri}}`, one URL so the old Lambda, which
//! forwards bodies unchanged, carries it too. The intake marks that user's assessment booked
//! and sends it to Clay once.

use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::post,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::auth::Claims;
use crate::rate_limit::{RateLimiter, limit_by_user};
use crate::state::AppState;

const MAX_FIELD_CHARS: usize = 200;
const MAX_NOTES_CHARS: usize = 5000;
const MAX_CHALLENGES: usize = 20;
/// A submission is one small write; a slow intake shouldn't hold the form for the
/// shared client's full minute.
const INTAKE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

/// Per caller, not global: one user scripting the form must not lock everyone else out,
/// and no human submits it more than a few times a minute.
pub fn router(limiter: RateLimiter) -> Router<AppState> {
    Router::new()
        .route("/assessments", post(submit_assessment))
        .route("/assessments/{id}/booking", post(record_booking))
        .layer(axum::middleware::from_fn_with_state(limiter, limit_by_user))
}

/// The form's answers, as the UI sends them.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct AssessmentSubmission {
    pub applying_as: ApplyingAs,
    pub name: String,
    pub email: String,
    #[serde(default)]
    pub company: Option<String>,
    pub agents: String,
    pub spend: String,
    #[serde(default)]
    pub challenges: Vec<String>,
    #[serde(default)]
    pub notes: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ApplyingAs {
    Individual,
    Team,
}

/// What the intake receives: the answers plus who sent them. The intake stamps the
/// id and time.
#[derive(Debug, Serialize)]
struct IntakeRequest {
    user_id: Uuid,
    #[serde(flatten)]
    answers: AssessmentSubmission,
}

/// Calendly's resource URIs, as the embed's `calendly.event_scheduled` message carries them.
const CALENDLY_EVENTS: &str = "https://api.calendly.com/scheduled_events/";
/// Calendly ids are short; anything longer isn't one.
const MAX_CALENDLY_ID_CHARS: usize = 64;

/// The booking, as the page sends it.
#[derive(Debug, Deserialize)]
pub struct BookingReport {
    pub event_uri: String,
    pub invitee_uri: String,
}

/// What the intake receives for a booking: the assessment, its sender and Calendly's URIs.
#[derive(Debug, Serialize)]
struct IntakeBooking<'a> {
    booking: BookingFields<'a>,
}

#[derive(Debug, Serialize)]
struct BookingFields<'a> {
    lead_id: Uuid,
    user_id: Uuid,
    event_uri: &'a str,
    invitee_uri: &'a str,
}

/// `…/scheduled_events/<id>` and `…/scheduled_events/<id>/invitees/<id>`, the invitee under that event.
fn is_calendly_booking(r: &BookingReport) -> bool {
    let id = |s: &str| {
        !s.is_empty()
            && s.len() <= MAX_CALENDLY_ID_CHARS
            && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
    };
    let Some(event) = r.event_uri.strip_prefix(CALENDLY_EVENTS) else {
        return false;
    };
    let Some(invitee) = r
        .invitee_uri
        .strip_prefix(&r.event_uri)
        .and_then(|rest| rest.strip_prefix("/invitees/"))
    else {
        return false;
    };
    id(event) && id(invitee)
}

#[derive(Debug, Deserialize)]
struct IntakeResponse {
    data: IntakeReceipt,
}

#[derive(Debug, Deserialize)]
struct IntakeReceipt {
    id: String,
}

/// The same rules the form enforces client-side (and the intake again), plus size
/// caps so a stored object stays small. Returns the message for the 400.
fn validate(s: &AssessmentSubmission) -> Result<(), &'static str> {
    let short = |v: &str| v.chars().count() <= MAX_FIELD_CHARS;
    if s.name.trim().is_empty() {
        return Err("name is required");
    }
    let email = s.email.trim();
    if email.is_empty() || !email.contains('@') {
        return Err("a valid email is required");
    }
    if s.applying_as == ApplyingAs::Team && s.company.as_deref().is_none_or(|c| c.trim().is_empty())
    {
        return Err("company is required when applying as a team");
    }
    if s.agents.trim().is_empty() || s.spend.trim().is_empty() {
        return Err("agents and spend are required");
    }
    let fields = [&s.name, &s.email, &s.agents, &s.spend];
    if !fields.iter().all(|f| short(f))
        || !s.company.as_deref().is_none_or(short)
        || s.challenges.len() > MAX_CHALLENGES
        || !s.challenges.iter().all(|c| short(c))
        || s.notes.chars().count() > MAX_NOTES_CHARS
    {
        return Err("an answer is too long");
    }
    Ok(())
}

fn error(status: StatusCode, message: &str, code: &str) -> Response {
    (status, Json(json!({ "error": message, "code": code }))).into_response()
}

async fn forward(
    client: &reqwest::Client,
    url: &str,
    request: &IntakeRequest,
) -> Result<String, String> {
    let resp = client
        .post(url)
        .timeout(INTAKE_TIMEOUT)
        .json(request)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("intake answered {status}: {body}"));
    }
    let receipt: IntakeResponse = resp.json().await.map_err(|e| e.to_string())?;
    Ok(receipt.data.id)
}

async fn submit_assessment(
    State(state): State<AppState>,
    claims: Claims,
    Json(answers): Json<AssessmentSubmission>,
) -> Response {
    let user_id = match claims.user_uuid() {
        Ok(id) => id,
        Err(e) => return e.into_response(),
    };
    if let Err(message) = validate(&answers) {
        return error(StatusCode::BAD_REQUEST, message, "invalid_assessment");
    }
    let Some(url) = state.config.assessment_intake_url.as_deref() else {
        return error(
            StatusCode::SERVICE_UNAVAILABLE,
            "assessment intake is not configured",
            "assessments_not_configured",
        );
    };
    let request = IntakeRequest { user_id, answers };
    match forward(&state.http_client, url, &request).await {
        Ok(id) => (StatusCode::CREATED, Json(json!({ "data": { "id": id } }))).into_response(),
        Err(e) => {
            tracing::error!(error = %e, "failed to forward assessment to the intake");
            error(
                StatusCode::BAD_GATEWAY,
                "could not save the assessment",
                "assessment_store_failed",
            )
        }
    }
}

async fn record_booking(
    State(state): State<AppState>,
    claims: Claims,
    Path(lead_id): Path<Uuid>,
    Json(report): Json<BookingReport>,
) -> Response {
    let user_id = match claims.user_uuid() {
        Ok(id) => id,
        Err(e) => return e.into_response(),
    };
    if !is_calendly_booking(&report) {
        return error(
            StatusCode::BAD_REQUEST,
            "not a Calendly booking",
            "invalid_booking",
        );
    }
    let Some(url) = state.config.assessment_intake_url.as_deref() else {
        return error(
            StatusCode::SERVICE_UNAVAILABLE,
            "assessment intake is not configured",
            "assessments_not_configured",
        );
    };
    let body = IntakeBooking {
        booking: BookingFields {
            lead_id,
            user_id,
            event_uri: &report.event_uri,
            invitee_uri: &report.invitee_uri,
        },
    };
    let sent = state
        .http_client
        .post(url)
        .timeout(INTAKE_TIMEOUT)
        .json(&body)
        .send()
        .await;
    match sent.map(|r| r.status()) {
        Ok(s) if s.is_success() => {
            (StatusCode::OK, Json(json!({ "data": { "id": lead_id } }))).into_response()
        }
        Ok(StatusCode::NOT_FOUND) => error(
            StatusCode::NOT_FOUND,
            "no such assessment",
            "assessment_not_found",
        ),
        Ok(s) => {
            tracing::error!(status = %s, %lead_id, "the intake refused an assessment booking");
            error(
                StatusCode::BAD_GATEWAY,
                "could not record the booking",
                "assessment_store_failed",
            )
        }
        Err(e) => {
            tracing::error!(error = %e, %lead_id, "failed to forward an assessment booking");
            error(
                StatusCode::BAD_GATEWAY,
                "could not record the booking",
                "assessment_store_failed",
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn answers() -> AssessmentSubmission {
        AssessmentSubmission {
            applying_as: ApplyingAs::Individual,
            name: "Ada".into(),
            email: "ada@example.com".into(),
            company: None,
            agents: "2-5".into(),
            spend: "unknown".into(),
            challenges: vec!["Other".into()],
            notes: String::new(),
        }
    }

    #[test]
    fn a_complete_individual_submission_is_valid() {
        assert!(validate(&answers()).is_ok());
    }

    #[test]
    fn a_team_submission_needs_a_company() {
        let mut s = answers();
        s.applying_as = ApplyingAs::Team;
        assert!(validate(&s).is_err());
        s.company = Some("Acme".into());
        assert!(validate(&s).is_ok());
    }

    #[test]
    fn missing_or_malformed_required_fields_are_rejected() {
        for broken in [
            AssessmentSubmission {
                name: "  ".into(),
                ..answers()
            },
            AssessmentSubmission {
                email: "nope".into(),
                ..answers()
            },
            AssessmentSubmission {
                spend: String::new(),
                ..answers()
            },
        ] {
            assert!(validate(&broken).is_err());
        }
    }

    #[test]
    fn oversized_answers_are_rejected() {
        let s = AssessmentSubmission {
            notes: "x".repeat(MAX_NOTES_CHARS + 1),
            ..answers()
        };
        assert_eq!(validate(&s), Err("an answer is too long"));
    }

    #[test]
    fn the_intake_gets_the_answers_flat_beside_the_sender() {
        let body = serde_json::to_value(IntakeRequest {
            user_id: Uuid::nil(),
            answers: answers(),
        })
        .unwrap();
        assert_eq!(body["user_id"], "00000000-0000-0000-0000-000000000000");
        assert_eq!(body["applying_as"], "individual");
        assert_eq!(body["agents"], "2-5");
    }

    const EVENT: &str = "https://api.calendly.com/scheduled_events/AB12-cd34";

    fn report(event_uri: &str, invitee_uri: String) -> BookingReport {
        BookingReport {
            event_uri: event_uri.into(),
            invitee_uri,
        }
    }

    #[test]
    fn a_calendly_booking_is_its_event_and_an_invitee_under_it() {
        assert!(is_calendly_booking(&report(
            EVENT,
            format!("{EVENT}/invitees/EF56-gh78")
        )));
        let long = "a".repeat(MAX_CALENDLY_ID_CHARS + 1);
        for (event, invitee) in [
            (
                "https://evil.example/scheduled_events/AB12",
                format!("{EVENT}/invitees/a"),
            ),
            (
                EVENT,
                "https://api.calendly.com/scheduled_events/OTHER/invitees/EF56".into(),
            ),
            (EVENT, format!("{EVENT}/invitees/")),
            (EVENT, format!("{EVENT}/invitees/a?b=c")),
            (
                "https://api.calendly.com/scheduled_events/",
                "https://api.calendly.com/scheduled_events//invitees/a".into(),
            ),
            (
                &*format!("https://api.calendly.com/scheduled_events/{long}"),
                format!("https://api.calendly.com/scheduled_events/{long}/invitees/a"),
            ),
        ] {
            assert!(
                !is_calendly_booking(&report(event, invitee.clone())),
                "{event} {invitee}"
            );
        }
    }

    #[test]
    fn the_intake_gets_the_booking_under_its_own_key() {
        let body = serde_json::to_value(IntakeBooking {
            booking: BookingFields {
                lead_id: Uuid::nil(),
                user_id: Uuid::nil(),
                event_uri: EVENT,
                invitee_uri: "x",
            },
        })
        .unwrap();
        assert_eq!(
            body["booking"]["lead_id"],
            "00000000-0000-0000-0000-000000000000"
        );
        assert_eq!(body["booking"]["event_uri"], EVENT);
        assert!(body.get("name").is_none());
    }
}

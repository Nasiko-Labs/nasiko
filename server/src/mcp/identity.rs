//! Agent-call identity resolution, shared.
//!
//! An agent proves *itself* with its deploy-time `MCP_GATEWAY_TOKEN`; the *user* it is working
//! for is never taken from the request. It is resolved from the `traceparent` against
//! `flows.user_id` with a `flow_participants` membership check, requiring a live flow — see
//! `docs/MCP_GATEWAY_AGENT_AUTH.md` §2.4. Presence is not the check, resolution is: there is
//! nothing an agent can fabricate to pass.
//!
//! A coding agent (a harness session, or a local coding agent) presents a session token instead,
//! whose signature proves both the agent and the user ([`session_identity`]); every layer tries
//! it first and falls back to the agent's token.
//!
//! This lives in its own public module rather than inside the gateway handler because more than
//! one layer has to make this decision, and each one making it *itself* is how the rule drifts.
//! That is not hypothetical: a request-interception layer elsewhere in this codebase once kept its
//! own copy of token extraction and JWT decode, and every rule later added to the shared validator
//! silently failed to apply to the endpoints it covered, leaving a stale identity authenticated on
//! them. One implementation, called from everywhere, is the fix.

use axum::{
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use uuid::Uuid;

use crate::state::AppState;

/// The `Bearer` value of the `Authorization` header, if any.
pub fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(axum::http::header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
        .map(str::trim)
        .filter(|t| !t.is_empty())
}

/// Resolve the flow named by `traceparent` to its user, requiring the flow to be live
/// (`status = 'running'`, younger than `NASIKO_FLOW_TIMEOUT_SECS` — a flow cannot legitimately
/// outlive it) and the authenticated agent to be a recorded participant.
///
/// Every failure is a 403 with a descriptive body, including an identity-store outage: failing
/// closed is the posture for anything that authorizes a *tool call*, where a denial costs the
/// call and nothing else.
///
/// A caller that sits **inline on a chat forward** must not reuse that posture. There, the same
/// failed lookup has to mean "carry on with what could be resolved and relay the call", never a
/// denial — denying a forward takes the chat down on a transient flow-lookup blip. Fail closed on
/// the decision, open on the forward. The two postures are deliberately opposite, so a new caller
/// has to pick one on purpose rather than inherit this one.
///
/// Returns the flow id alongside the user: it's the one thing this function actually verified
/// against `flows`/`flow_participants`, so callers that need to key anything on the flow (e.g.
/// publishing a `FlowEvent`, or minting a memory session id) must use this, never a fresh,
/// unverified re-parse of the raw `traceparent` — that would let a caller launder an unverified
/// trace id through a path that was supposed to require proof of participation.
#[allow(clippy::result_large_err)]
pub async fn flow_user(
    state: &AppState,
    traceparent: Option<&str>,
    agent_id: Uuid,
) -> Result<(Uuid, String), Response> {
    let flow_id = traceparent
        .and_then(nasiko_flow::FlowContext::from_traceparent)
        .map(|ctx| ctx.flow_id)
        .ok_or_else(|| {
            deny(format!(
                "traceparent missing or malformed (received: {:?}) — tools/call must carry the W3C trace context of the flow it serves",
                traceparent.unwrap_or("<none>")
            ))
        })?;

    let row: Option<(Uuid, bool)> = sqlx::query_as(
        "SELECT f.user_id,
                EXISTS(SELECT 1 FROM flow_participants fp
                       WHERE fp.flow_id = f.flow_id AND fp.agent_id = $2)
         FROM flows f
         WHERE f.flow_id = $1
           AND f.status = 'running'
           AND f.created_at > now() - make_interval(secs => $3)",
    )
    .bind(&flow_id)
    .bind(agent_id)
    .bind(f64::from(state.config.flow_timeout_secs))
    .fetch_optional(&state.db)
    .await
    .map_err(|e| {
        // Fail closed on identity-store failure.
        tracing::error!(error = %e, %flow_id, "mcp gateway: flow lookup failed — failing closed");
        deny("identity store unavailable".to_string())
    })?;

    match row {
        Some((user_id, true)) => Ok((user_id, flow_id)),
        Some((_, false)) => Err(deny(format!(
            "agent {agent_id} is not a participant of flow {flow_id}"
        ))),
        None => Err(deny(format!(
            "traceparent does not resolve to a live flow (trace_id {flow_id})"
        ))),
    }
}

pub(crate) fn deny(body: String) -> Response {
    (StatusCode::FORBIDDEN, body).into_response()
}

/// Outcome of trying to read a coding-agent session token off the credential.
pub enum SessionResolution {
    /// A valid, non-revoked session token: use its signed identity directly.
    Session {
        agent_id: Uuid,
        user_id: Uuid,
        flow_id: String,
    },
    /// Not a session token (the ngt_ gateway token, or anything else) — the
    /// caller should fall back to the deploy-time-token + traceparent path.
    NotSession,
    /// A session token that must be rejected (revoked, malformed claims, or the
    /// store could not be reached).
    Deny(Response),
}

/// Try to authenticate the credential as a coding-agent session token.
///
/// The signature proves `{agent_id, user_id}`, so — unlike the A2A path — this needs no
/// live-flow freshness window (a long turn is never rejected for age). The DB touches are the
/// `jti` revocation check, so a token can still be killed before its `exp`, and the flow
/// attribution: a later turn's flow (the traceparent's, else the session's newest) is used only
/// when that user owns it and the agent is a recorded participant, else the claim.
pub async fn session_identity(
    state: &AppState,
    token: &str,
    traceparent: Option<&str>,
) -> SessionResolution {
    let secret = &state.config.agent_jwt_secret;
    // Unconfigured secret ⇒ this deployment doesn't issue session tokens; leave
    // the legacy path untouched.
    if secret.is_empty() {
        return SessionResolution::NotSession;
    }
    let algorithm = nasiko_session_token::parse_algorithm(&state.config.agent_jwt_algorithm);

    let claims = match nasiko_session_token::verify_session_token(token, secret, algorithm) {
        Ok(claims) => claims,
        Err(nasiko_session_token::SessionTokenError::NotSessionToken) => {
            return SessionResolution::NotSession;
        }
        // Expired or otherwise unusable session token: reject rather than fall
        // back (a fallback would just 401 on the ngt_ lookup anyway).
        Err(e) => {
            return SessionResolution::Deny(
                (
                    StatusCode::UNAUTHORIZED,
                    format!("session token rejected: {e}"),
                )
                    .into_response(),
            );
        }
    };

    let agent_id = match Uuid::parse_str(&claims.agent_id) {
        Ok(id) => id,
        Err(_) => {
            return SessionResolution::Deny(deny("session token agent_id is not a UUID".into()));
        }
    };
    let user_id = match Uuid::parse_str(&claims.user_id) {
        Ok(id) => id,
        Err(_) => {
            return SessionResolution::Deny(deny("session token user_id is not a UUID".into()));
        }
    };

    match is_session_token_revoked(&state.db, &claims.jti).await {
        Ok(false) => {
            let trace_flow = traceparent
                .and_then(nasiko_flow::FlowContext::from_traceparent)
                .map(|ctx| ctx.flow_id);
            let attributed = nasiko_session_token::attribution::attributed_flow(
                &state.db,
                trace_flow.as_deref(),
                &claims,
            )
            .await;
            SessionResolution::Session {
                agent_id,
                user_id,
                flow_id: attributed.flow_id,
            }
        }
        Ok(true) => SessionResolution::Deny(deny("session token revoked".into())),
        Err(e) => {
            // Rule 5: fail closed on identity-store failure.
            tracing::error!(error = %e, "mcp gateway: revocation check failed — failing closed");
            SessionResolution::Deny(deny("identity store unavailable".into()))
        }
    }
}

/// True if this `jti` has been revoked.
async fn is_session_token_revoked(db: &sqlx::PgPool, jti: &str) -> sqlx::Result<bool> {
    let found: Option<i32> =
        sqlx::query_scalar("SELECT 1 FROM revoked_session_tokens WHERE jti = $1")
            .bind(jti)
            .fetch_optional(db)
            .await?;
    Ok(found.is_some())
}

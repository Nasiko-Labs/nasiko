//! Authentication for `/api/agents/{id}/*`: a user's session, or an agent calling a peer.
//!
//! An in-container agent that discovered a peer through the registry (`/a2a/v1`) is handed that
//! peer's proxy URL and calls it with its own deploy-time `MCP_GATEWAY_TOKEN`. The token proves
//! *which agent* is calling; *which user* it is working for is never taken from the request but
//! resolved from the `traceparent` against `flows.user_id`, with the agent required to be a
//! recorded participant of that live flow (`mcp::identity::flow_user`, the same rule the MCP
//! gateway and LLM router apply). The request then carries that user's [`Claims`] (so every
//! user-level ACL still applies) and a [`CallingAgent`] (so `agent_acl` is enforced).

use axum::{
    extract::{Request, State},
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
};
use uuid::Uuid;

use crate::acl::CallingAgent;
use crate::auth::{Claims, require_auth};
use crate::rate_limit::RateLimiter;
use crate::state::AppState;

/// Prefix of a gateway credential; a user's session JWT never has it.
const GATEWAY_TOKEN_PREFIX: &str = "ngt_";

/// State for [`require_user_or_agent`]: the app, and the per-agent budget for calls to peers
/// (`AGENT_PEER_CALL_RATE_LIMIT_PER_MINUTE`, keyed by the calling agent's id).
#[derive(Clone)]
pub struct AgentCallerAuth {
    pub app: AppState,
    pub limiter: RateLimiter,
}

/// [`require_auth`] for the agent proxy, which also accepts an agent's gateway credential.
pub async fn require_user_or_agent(
    State(auth): State<AgentCallerAuth>,
    mut req: Request,
    next: Next,
) -> Response {
    let state = auth.app;
    let Some(token) = agent_credential(&req) else {
        return require_auth(State(state), req, next).await;
    };
    let traceparent = req
        .headers()
        .get("traceparent")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    match authenticate_agent(&state, &auth.limiter, &token, traceparent.as_deref()).await {
        Ok((claims, caller)) => {
            req.extensions_mut().insert(claims);
            req.extensions_mut().insert(caller);
            next.run(req).await
        }
        Err(rejection) => rejection,
    }
}

/// The bearer value when it is a gateway credential.
fn agent_credential(req: &Request) -> Option<String> {
    crate::mcp::identity::bearer_token(req.headers())
        .filter(|token| token.starts_with(GATEWAY_TOKEN_PREFIX))
        .map(str::to_owned)
}

async fn authenticate_agent(
    state: &AppState,
    limiter: &RateLimiter,
    token: &str,
    traceparent: Option<&str>,
) -> Result<(Claims, CallingAgent), Response> {
    let caller = match nasiko_mcp_gateway::agent_tokens::authenticate(&state.db, token).await {
        Ok(Some(agent_id)) => agent_id,
        Ok(None) => return Err(reject(StatusCode::UNAUTHORIZED, "invalid agent credential")),
        Err(e) => {
            tracing::error!(error = %e, "agent proxy: credential lookup failed; failing closed");
            return Err(reject(
                StatusCode::UNAUTHORIZED,
                "agent credential unavailable",
            ));
        }
    };
    // Charged as soon as the caller is known, before the flow and user lookups it would cost.
    if let Err(retry_after) = limiter.check(&format!("agent:{caller}")).await {
        return Err(crate::rate_limit::too_many_requests_after(retry_after));
    }
    // 403 unless the traceparent names a live flow this agent participates in.
    let (user_id, _flow_id) = crate::mcp::identity::flow_user(state, traceparent, caller).await?;
    let claims = user_claims(state, user_id).await?;
    Ok((claims, CallingAgent(caller)))
}

async fn user_claims(state: &AppState, user_id: Uuid) -> Result<Claims, Response> {
    let row: Option<(String, bool)> = sqlx::query_as(
        "SELECT username, is_superuser FROM users WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(user_id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "agent proxy: flow user lookup failed; failing closed");
        reject(StatusCode::UNAUTHORIZED, "flow user unavailable")
    })?;
    let (username, is_superuser) =
        row.ok_or_else(|| reject(StatusCode::UNAUTHORIZED, "the flow's user no longer exists"))?;
    Ok(Claims {
        sub: user_id.to_string(),
        username,
        is_superuser,
    })
}

fn reject(status: StatusCode, message: &'static str) -> Response {
    (status, message).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::Request as HttpRequest;

    fn request(authorization: Option<&str>) -> Request {
        let mut builder = HttpRequest::builder().uri("/api/agents/x");
        if let Some(value) = authorization {
            builder = builder.header("authorization", value);
        }
        builder.body(axum::body::Body::empty()).unwrap()
    }

    #[test]
    fn only_a_gateway_credential_is_taken_as_an_agent_credential() {
        assert_eq!(
            agent_credential(&request(Some("Bearer ngt_abc"))).as_deref(),
            Some("ngt_abc")
        );
        // A session JWT, a state credential and no header at all go the user route.
        assert_eq!(
            agent_credential(&request(Some("Bearer eyJhbGci.x.y"))),
            None
        );
        assert_eq!(agent_credential(&request(Some("Bearer nst_abc"))), None);
        assert_eq!(agent_credential(&request(None)), None);
    }
}

//! Who is calling: the agent behind a bearer credential, charged against its request budget,
//! and the issuing of that credential.

use std::time::{Duration, Instant};

use axum::extract::{FromRequestParts, Request, State};
use axum::http::request::Parts;
use axum::http::{HeaderMap, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use nasiko_mcp_gateway::agent_tokens::{self, TokenScope};
use sqlx::PgPool;
use uuid::Uuid;

use super::error::StateError;
use crate::rate_limit::RateLimiter;
use crate::telemetry::agent_replicas::ReadyReplicas;

/// Length of the fixed window the per-agent request budget is counted over, and so also the
/// longest a rejected caller can be told to wait (`Retry-After` is what is left of it).
pub(super) const RATE_WINDOW: Duration = Duration::from_secs(60);

/// Mints, or rotates, the agent's state API credential and returns its plaintext, once.
///
/// This is the only credential the state API accepts: a state-scoped agent token, meant to be
/// injected into the agent's environment as `NASIKO_AGENT_TOKEN` at deploy. Never the MCP gateway
/// credential, which also travels in URL paths and can end up in logs. Rotation keeps the previous
/// plaintext working for the gateway's grace window (`agent_tokens::ROTATION_GRACE_SECS`), so a
/// rollout in progress is not cut off. Generic over the executor for the same reason as
/// `agent_tokens::mint`: a deploy may run inside the transaction that created the agent row.
pub async fn mint_state_token(
    db: impl sqlx::PgExecutor<'_>,
    agent_id: Uuid,
) -> Result<String, sqlx::Error> {
    agent_tokens::mint_scoped(db, agent_id, TokenScope::State).await
}

/// The agent a request was authenticated as. Only [`authenticate`] puts one on a request.
#[derive(Clone, Copy, Debug)]
pub(super) struct AuthenticatedAgent(pub(super) Uuid);

/// State for [`authenticate`]: the credential store and the per-agent request budget.
#[derive(Clone)]
pub(super) struct AgentAuth {
    db: PgPool,
    limiter: RateLimiter,
    requests_per_replica_per_minute: u32,
    ready_replicas: ReadyReplicas,
}

impl AgentAuth {
    /// An agent's budget is `requests_per_replica_per_minute` for each of its ready replicas, so
    /// an autoscaled agent is not held to the allowance of one replica while its load, and so its
    /// state traffic, grows with the replica count.
    pub(super) fn new(
        db: PgPool,
        requests_per_replica_per_minute: u32,
        ready_replicas: ReadyReplicas,
    ) -> Self {
        Self {
            db,
            // The limiter's own limit is unused: every check passes the agent's current budget.
            limiter: RateLimiter::new(requests_per_replica_per_minute, RATE_WINDOW),
            requests_per_replica_per_minute,
            ready_replicas,
        }
    }

    /// Charge one request to `agent_id` against its budget now.
    ///
    /// The replica count is read from the cache only, never from the runtime, so this stays off
    /// the slow path; an agent not in the cache yet, or reporting no ready replica, counts as one.
    /// Noting the request also keeps the agent's count refreshed while it is calling.
    fn charge(&self, agent_id: Uuid) -> Result<(), Duration> {
        let key = agent_id.to_string();
        let replicas = self.ready_replicas.note_request(&key, Instant::now());
        self.limiter
            .check_with_limit(&key, self.budget_for(replicas))
    }

    fn budget_for(&self, ready_replicas: u64) -> u32 {
        let budget = u64::from(self.requests_per_replica_per_minute).saturating_mul(ready_replicas);
        u32::try_from(budget).unwrap_or(u32::MAX)
    }
}

/// Middleware for every agent state route.
///
/// Only a state-scoped credential ([`mint_state_token`]) is accepted; the agent's MCP gateway
/// credential is refused like an unknown one. The lookup also refuses a revoked credential and one
/// whose agent has been deleted. Authentication comes first, so a caller without a valid
/// credential is refused before its body is read and never spends anyone's budget.
pub(super) async fn authenticate(
    State(auth): State<AgentAuth>,
    mut request: Request,
    next: Next,
) -> Response {
    match identify(&auth, request.headers()).await {
        Ok(agent) => {
            request.extensions_mut().insert(agent);
            next.run(request).await
        }
        Err(error) => error.into_response(),
    }
}

async fn identify(auth: &AgentAuth, headers: &HeaderMap) -> Result<AuthenticatedAgent, StateError> {
    let token = bearer_token(headers).ok_or(StateError::Unauthorized)?;
    let agent_id = agent_tokens::authenticate_scoped(&auth.db, token, TokenScope::State)
        .await
        .map_err(|e| StateError::Internal(format!("agent token lookup failed: {e}")))?
        .ok_or(StateError::Unauthorized)?;
    auth.charge(agent_id).map_err(StateError::rate_limited)?;
    Ok(AuthenticatedAgent(agent_id))
}

/// `Authorization: Bearer <token>`, read exactly as the MCP gateway reads its own.
fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
        .map(str::trim)
        .filter(|token| !token.is_empty())
}

impl<S: Send + Sync> FromRequestParts<S> for AuthenticatedAgent {
    type Rejection = StateError;

    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, StateError> {
        // A handler reached without the middleware must fail closed, not act for nobody.
        parts
            .extensions
            .get::<Self>()
            .copied()
            .ok_or(StateError::Unauthorized)
    }
}

#[cfg(test)]
mod tests {
    use axum::http::HeaderValue;

    use super::*;

    fn headers_with(authorization: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_str(authorization).unwrap(),
        );
        headers
    }

    #[test]
    fn a_bearer_token_is_read_and_trimmed() {
        assert_eq!(
            bearer_token(&headers_with("Bearer ngt_abc")),
            Some("ngt_abc")
        );
        assert_eq!(
            bearer_token(&headers_with("Bearer   ngt_abc  ")),
            Some("ngt_abc")
        );
    }

    #[test]
    fn anything_but_a_non_empty_bearer_token_is_no_credential() {
        assert_eq!(bearer_token(&HeaderMap::new()), None);
        for bad in [
            "Bearer ",
            "Bearer    ",
            "Basic dXNlcjpwYXNz",
            "ngt_abc",
            "bearer ngt_abc",
        ] {
            assert_eq!(bearer_token(&headers_with(bad)), None, "{bad:?}");
        }
    }

    fn auth_with(per_replica: u32, ready_replicas: ReadyReplicas) -> AgentAuth {
        let db = PgPool::connect_lazy("postgres://hermetic.invalid/none").unwrap();
        AgentAuth::new(db, per_replica, ready_replicas)
    }

    fn requests_admitted(auth: &AgentAuth, agent: Uuid, attempts: u32) -> u32 {
        (0..attempts)
            .filter(|_| auth.charge(agent).is_ok())
            .count()
            .try_into()
            .unwrap()
    }

    #[tokio::test]
    async fn an_agent_with_one_replica_gets_the_per_replica_budget() {
        let auth = auth_with(5, ReadyReplicas::default());
        assert_eq!(requests_admitted(&auth, Uuid::new_v4(), 50), 5);
    }

    #[tokio::test]
    async fn two_ready_replicas_double_the_budget() {
        let replicas = ReadyReplicas::default();
        let agent = Uuid::new_v4();
        replicas.note_request(&agent.to_string(), Instant::now());
        replicas.set_ready(&agent.to_string(), 2);
        let auth = auth_with(5, replicas);
        assert_eq!(requests_admitted(&auth, agent, 50), 10);
    }

    #[tokio::test]
    async fn a_budget_too_large_for_u32_saturates() {
        let auth = auth_with(u32::MAX, ReadyReplicas::default());
        assert_eq!(auth.budget_for(4), u32::MAX);
    }

    #[tokio::test]
    async fn a_request_the_middleware_never_saw_is_unauthorized() {
        let (mut parts, _) = axum::http::Request::new(()).into_parts();
        let result = AuthenticatedAgent::from_request_parts(&mut parts, &()).await;
        assert!(matches!(result, Err(StateError::Unauthorized)));
    }

    #[tokio::test]
    async fn the_identity_the_middleware_attached_is_the_one_extracted() {
        let agent = Uuid::new_v4();
        let (mut parts, _) = axum::http::Request::new(()).into_parts();
        parts.extensions.insert(AuthenticatedAgent(agent));
        let extracted = AuthenticatedAgent::from_request_parts(&mut parts, &())
            .await
            .unwrap();
        assert_eq!(extracted.0, agent);
    }
}

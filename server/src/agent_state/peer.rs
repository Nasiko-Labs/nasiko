//! A coarse per-address request limit in front of the credential check.
//!
//! Identifying a caller costs a database lookup, and a caller without a valid credential never
//! reaches the per-agent budget (`auth`), so without this nothing bounds how fast one source can
//! make the server look tokens up. It is deliberately loose, a multiple of one agent's budget,
//! because several agents can share an address: behind a NAT, or on one host.
//!
//! The address is the TCP peer, known when the server is served with connection info
//! (`into_make_service_with_connect_info`, as the OSS binary does). Without it there is nothing
//! per-source to key on, and one shared bucket would cap every agent together at one address's
//! allowance, so the limit stands aside and says so once in the log.

use std::net::SocketAddr;
use std::sync::Once;

use axum::extract::{ConnectInfo, Request, State};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use super::auth::RATE_WINDOW;
use super::error::StateError;
use crate::rate_limit::RateLimiter;

/// How many agents' budgets one address may spend before its requests are refused unread.
const AGENTS_PER_ADDRESS: u32 = 10;

/// State for [`limit_by_peer`].
#[derive(Clone)]
pub(super) struct PeerLimit(RateLimiter);

impl PeerLimit {
    pub(super) fn new(agent_requests_per_minute: u32) -> Self {
        Self(RateLimiter::new(
            per_address_allowance(agent_requests_per_minute),
            RATE_WINDOW,
        ))
    }
}

fn per_address_allowance(agent_requests_per_minute: u32) -> u32 {
    agent_requests_per_minute.saturating_mul(AGENTS_PER_ADDRESS)
}

/// Middleware for every agent state route, outside the credential check: an address over its
/// allowance gets a 429 before any token is looked up.
pub(super) async fn limit_by_peer(
    State(PeerLimit(limiter)): State<PeerLimit>,
    request: Request,
    next: Next,
) -> Response {
    let Some(ConnectInfo(peer)) = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .copied()
    else {
        warn_once_that_peers_are_unknown();
        return next.run(request).await;
    };
    match limiter.check(&peer.ip().to_string()) {
        Ok(()) => next.run(request).await,
        Err(wait) => StateError::rate_limited(wait).into_response(),
    }
}

fn warn_once_that_peers_are_unknown() {
    static WARNED: Once = Once::new();
    WARNED.call_once(|| {
        tracing::warn!(
            "agent state: the server is not given peer addresses, so the per-address request \
             limit is off; only the per-agent limit applies"
        );
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_address_may_spend_several_agents_budgets() {
        assert_eq!(per_address_allowance(600), 6000);
        assert_eq!(per_address_allowance(1), AGENTS_PER_ADDRESS);
        assert_eq!(per_address_allowance(u32::MAX), u32::MAX);
    }
}

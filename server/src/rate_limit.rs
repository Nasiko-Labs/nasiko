//! Minimal in-memory rate limiting.
//!
//! Gateway removal (see `docs/ROUTER_MIGRATION.md`) left this server with no
//! rate limiting anywhere, on any route — including `/api/orchestrator/a2a`
//! (LLM cost), `/v2/*` (OCI storage cost), and `/api/auth/login` (auth-bypass
//! brute force / bcrypt-cost-12 CPU exhaustion). This is a fixed-window
//! counter, not a distributed/production-grade limiter — appropriate for a
//! single-process, self-hosted deployment; a multi-replica deployment would
//! need a shared store (Redis) instead, since each replica keeps its own
//! in-memory buckets.

use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use dashmap::DashMap;

use crate::auth::Claims;

#[derive(Clone)]
pub struct RateLimiter {
    buckets: Arc<DashMap<String, (Instant, u32)>>,
    limit: u32,
    window: Duration,
}

impl RateLimiter {
    pub fn new(limit: u32, window: Duration) -> Self {
        Self { buckets: Arc::new(DashMap::new()), limit, window }
    }

    /// Records one request for `key`; returns `false` if it exceeds the
    /// limit for the current window.
    pub(crate) fn allow(&self, key: &str) -> bool {
        let now = Instant::now();
        let mut entry = self.buckets.entry(key.to_owned()).or_insert((now, 0));
        if now.duration_since(entry.0) > self.window {
            *entry = (now, 1);
            true
        } else if entry.1 < self.limit {
            entry.1 += 1;
            true
        } else {
            false
        }
    }
}

pub(crate) fn too_many_requests() -> Response {
    (StatusCode::TOO_MANY_REQUESTS, "rate limit exceeded, try again shortly").into_response()
}

/// Rate-limit by authenticated caller. Must be layered so it runs AFTER
/// `require_auth` (needs `Claims` already populated).
pub async fn limit_by_user(
    State(limiter): State<RateLimiter>,
    claims: Claims,
    req: Request,
    next: Next,
) -> Response {
    if limiter.allow(&claims.sub) {
        next.run(req).await
    } else {
        too_many_requests()
    }
}

/// Rate-limit a route with no authenticated identity yet (e.g. login) against
/// one shared, global bucket. Cruder than per-caller limiting — there's no
/// caller identity to key on before credentials are verified, and this
/// process doesn't see the real client IP without reverse-proxy trust
/// configuration — but it bounds worst-case bcrypt-cost-12 CPU burn from a
/// single runaway loop, which is the concern in this threat model
/// (self-hosted single-org PaaS, not an internet-facing multi-tenant login).
pub async fn limit_globally(State(limiter): State<RateLimiter>, req: Request, next: Next) -> Response {
    if limiter.allow("global") {
        next.run(req).await
    } else {
        too_many_requests()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allows_up_to_the_limit_then_rejects() {
        let limiter = RateLimiter::new(3, Duration::from_secs(60));
        assert!(limiter.allow("a"));
        assert!(limiter.allow("a"));
        assert!(limiter.allow("a"));
        assert!(!limiter.allow("a"), "4th request in the window must be rejected");
    }

    #[test]
    fn keys_are_independent() {
        let limiter = RateLimiter::new(1, Duration::from_secs(60));
        assert!(limiter.allow("a"));
        assert!(limiter.allow("b"), "a different key must have its own budget");
        assert!(!limiter.allow("a"));
    }

    #[test]
    fn resets_after_the_window_elapses() {
        let limiter = RateLimiter::new(1, Duration::from_millis(20));
        assert!(limiter.allow("a"));
        assert!(!limiter.allow("a"));
        std::thread::sleep(Duration::from_millis(30));
        assert!(limiter.allow("a"), "a new window must reset the budget");
    }
}

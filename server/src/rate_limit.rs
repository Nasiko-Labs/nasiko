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
        Self {
            buckets: Arc::new(DashMap::new()),
            limit,
            window,
        }
    }

    /// Units allowed per window.
    pub fn limit(&self) -> u32 {
        self.limit
    }

    /// Records one request for `key`; returns `false` if it exceeds the
    /// limit for the current window.
    pub fn allow(&self, key: &str) -> bool {
        self.try_consume(key, 1).is_ok()
    }

    /// [`allow`](Self::allow), saying on refusal how long remains until
    /// `key`'s window resets, which is when a retry can first succeed.
    pub(crate) fn check(&self, key: &str) -> Result<(), Duration> {
        self.check_with_limit(key, self.limit)
    }

    /// [`check`](Self::check) against `limit` instead of the one the limiter was built with, for
    /// a caller whose budget changes while its window is open (it is compared with the count so
    /// far, which is kept).
    pub(crate) fn check_with_limit(&self, key: &str, limit: u32) -> Result<(), Duration> {
        let now = Instant::now();
        let mut entry = self.buckets.entry(key.to_owned()).or_insert((now, 0));
        let elapsed = now.duration_since(entry.0);
        if elapsed > self.window {
            *entry = (now, 1);
            Ok(())
        } else if entry.1 < limit {
            entry.1 += 1;
            Ok(())
        } else {
            Err(self.window.saturating_sub(elapsed))
        }
    }

    /// Records `cost` units for `key` at once — for a route whose expense is
    /// the items in a request rather than the request itself. All or nothing:
    /// when the current window cannot take them, nothing is recorded and the
    /// error is how long until the window resets.
    ///
    /// A cost above the whole limit is charged as the limit, so a request the
    /// route accepts can always get through on an empty window.
    pub fn try_consume(&self, key: &str, cost: u32) -> Result<(), Duration> {
        let cost = cost.min(self.limit);
        let now = Instant::now();
        let mut entry = self.buckets.entry(key.to_owned()).or_insert((now, 0));
        let elapsed = now.duration_since(entry.0);
        if elapsed > self.window {
            *entry = (now, cost);
            Ok(())
        } else if entry.1.saturating_add(cost) <= self.limit {
            entry.1 += cost;
            Ok(())
        } else {
            Err(self.window - elapsed)
        }
    }
}

pub(crate) fn too_many_requests() -> Response {
    (
        StatusCode::TOO_MANY_REQUESTS,
        "rate limit exceeded, try again shortly",
    )
        .into_response()
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
pub async fn limit_globally(
    State(limiter): State<RateLimiter>,
    req: Request,
    next: Next,
) -> Response {
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
        assert!(
            !limiter.allow("a"),
            "4th request in the window must be rejected"
        );
    }

    #[test]
    fn keys_are_independent() {
        let limiter = RateLimiter::new(1, Duration::from_secs(60));
        assert!(limiter.allow("a"));
        assert!(
            limiter.allow("b"),
            "a different key must have its own budget"
        );
        assert!(!limiter.allow("a"));
    }

    #[test]
    fn weighted_cost_is_all_or_nothing_and_reports_the_reset() {
        let limiter = RateLimiter::new(10, Duration::from_secs(60));
        assert!(limiter.try_consume("a", 6).is_ok());
        let retry_after = limiter.try_consume("a", 5).unwrap_err();
        assert!(retry_after <= Duration::from_secs(60) && retry_after > Duration::from_secs(55));
        assert!(
            limiter.try_consume("a", 4).is_ok(),
            "a rejected cost must not be recorded"
        );
        assert!(limiter.try_consume("a", 1).is_err());
    }

    #[test]
    fn cost_above_the_limit_is_charged_as_the_limit() {
        let limiter = RateLimiter::new(3, Duration::from_secs(60));
        assert!(limiter.try_consume("a", 100).is_ok());
        assert!(limiter.try_consume("a", 1).is_err());
    }

    #[test]
    fn a_refusal_names_the_time_left_in_the_window() {
        let window = Duration::from_secs(60);
        let limiter = RateLimiter::new(1, window);
        assert_eq!(limiter.check("a"), Ok(()));
        std::thread::sleep(Duration::from_millis(20));
        let wait = limiter.check("a").expect_err("over the limit");
        assert!(wait <= window - Duration::from_millis(20), "{wait:?}");
        assert!(wait > window - Duration::from_secs(5), "{wait:?}");
    }

    #[test]
    fn a_raised_limit_applies_to_the_window_already_open() {
        let limiter = RateLimiter::new(1, Duration::from_secs(60));
        assert_eq!(limiter.check_with_limit("a", 1), Ok(()));
        assert!(limiter.check_with_limit("a", 1).is_err());
        assert_eq!(limiter.check_with_limit("a", 2), Ok(()));
        assert!(limiter.check_with_limit("a", 2).is_err());
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

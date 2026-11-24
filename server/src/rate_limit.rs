//! Fixed-window rate limiting, counted in Redis so every server replica draws on one budget.
//!
//! Gateway removal (see `docs/ROUTER_MIGRATION.md`) left this server with no
//! rate limiting anywhere, on any route — including `/api/orchestrator/a2a`
//! (LLM cost), `/v2/*` (OCI storage cost), and `/api/auth/login` (auth-bypass
//! brute force / bcrypt-cost-12 CPU exhaustion). Each limiter is a fixed-window
//! counter per key: the window opens at a key's first request and its budget
//! resets when the window closes.
//!
//! Counted in one process, each of N replicas behind a load balancer would grant
//! the full budget, so a 30/min login limit would be 30N/min. The counts therefore
//! live in Redis (one atomic script per check, key `nasiko:rl:{group}:{key}` expiring
//! with the window, so a refusal's `Retry-After` is the key's remaining TTL). When
//! Redis cannot answer, a check is counted in this process instead and the outage
//! is logged: the limit degrades to per replica rather than failing open or
//! refusing every request (login included) while Redis is down.

use std::sync::Arc;
use std::sync::LazyLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use dashmap::DashMap;
use nasiko_redis::SharedRedis;

use crate::auth::Claims;

/// How long a check waits for Redis before counting in this process instead. Redis answers in
/// well under a millisecond; an unreachable or hung one must not hold every limited request for
/// the shared connection's own (seconds-long) timeouts.
const REDIS_BUDGET: Duration = Duration::from_millis(250);
/// At most one "Redis unavailable" warning per this long, however many checks fall back.
const OUTAGE_WARNING_INTERVAL_SECS: u64 = 60;

/// Atomically charges `ARGV[1]` units to `KEYS[1]` against a limit of `ARGV[2]` in a window of
/// `ARGV[3]` ms. Returns `-1` when admitted, otherwise the milliseconds until the window closes.
/// All or nothing: a refused charge records nothing. The first charge of a window sets its
/// expiry, so the window opens at the key's first request, as in the in-process count.
static CHARGE: LazyLock<redis::Script> = LazyLock::new(|| {
    redis::Script::new(
        r"
        local cost = tonumber(ARGV[1])
        local count = tonumber(redis.call('GET', KEYS[1]) or '0')
        if count + cost > tonumber(ARGV[2]) then
            local ttl = redis.call('PTTL', KEYS[1])
            if ttl < 0 then
                redis.call('PEXPIRE', KEYS[1], ARGV[3])
                ttl = tonumber(ARGV[3])
            end
            return ttl
        end
        if redis.call('INCRBY', KEYS[1], cost) == cost then
            redis.call('PEXPIRE', KEYS[1], ARGV[3])
        end
        return -1
        ",
    )
});

/// Where a [`RateLimiter`] keeps its counts.
#[derive(Clone)]
pub enum RateLimitStore {
    /// In Redis, shared by every replica; counted in-process while Redis cannot answer.
    Redis(SharedRedis),
    /// In this process only: for tests, and anything that runs without Redis.
    Local,
}

#[derive(Clone)]
pub struct RateLimiter {
    store: RateLimitStore,
    /// Names this limiter's keys in Redis, so two limiters keyed on the same caller keep
    /// separate counts.
    group: &'static str,
    /// The in-process counts: the whole count under [`RateLimitStore::Local`], the fallback
    /// while Redis cannot answer under [`RateLimitStore::Redis`].
    buckets: Arc<DashMap<String, (Instant, u32)>>,
    limit: u32,
    window: Duration,
}

impl RateLimiter {
    /// `group` must be unique among the server's limiters: it is what keeps their counts apart.
    pub fn new(store: &RateLimitStore, group: &'static str, limit: u32, window: Duration) -> Self {
        Self {
            store: store.clone(),
            group,
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
    pub async fn allow(&self, key: &str) -> bool {
        self.try_consume(key, 1).await.is_ok()
    }

    /// [`allow`](Self::allow), saying on refusal how long remains until
    /// `key`'s window resets, which is when a retry can first succeed.
    pub(crate) async fn check(&self, key: &str) -> Result<(), Duration> {
        self.check_with_limit(key, self.limit).await
    }

    /// [`check`](Self::check) against `limit` instead of the one the limiter was built with, for
    /// a caller whose budget changes while its window is open (it is compared with the count so
    /// far, which is kept).
    pub(crate) async fn check_with_limit(&self, key: &str, limit: u32) -> Result<(), Duration> {
        self.charge(key, 1, limit).await
    }

    /// Records `cost` units for `key` at once — for a route whose expense is
    /// the items in a request rather than the request itself. All or nothing:
    /// when the current window cannot take them, nothing is recorded and the
    /// error is how long until the window resets.
    ///
    /// A cost above the whole limit is charged as the limit, so a request the
    /// route accepts can always get through on an empty window.
    pub async fn try_consume(&self, key: &str, cost: u32) -> Result<(), Duration> {
        self.charge(key, cost.min(self.limit), self.limit).await
    }

    async fn charge(&self, key: &str, cost: u32, limit: u32) -> Result<(), Duration> {
        let RateLimitStore::Redis(redis) = &self.store else {
            return self.charge_locally(key, cost, limit);
        };
        match self.charge_in_redis(redis, key, cost, limit).await {
            Ok(outcome) => outcome,
            Err(reason) => {
                warn_counting_locally(self.group, &reason);
                self.charge_locally(key, cost, limit)
            }
        }
    }

    /// The outer `Err` is Redis failing to answer; the inner result is its verdict.
    async fn charge_in_redis(
        &self,
        redis: &SharedRedis,
        key: &str,
        cost: u32,
        limit: u32,
    ) -> Result<Result<(), Duration>, String> {
        let window_ms = u64::try_from(self.window.as_millis())
            .unwrap_or(u64::MAX)
            .max(1);
        let call = async {
            let mut connection = redis.connection().await?;
            CHARGE
                .key(format!("nasiko:rl:{}:{key}", self.group))
                .arg(cost)
                .arg(limit)
                .arg(window_ms)
                .invoke_async::<i64>(&mut connection)
                .await
        };
        let wait_ms = tokio::time::timeout(REDIS_BUDGET, call)
            .await
            .map_err(|_| format!("no answer within {REDIS_BUDGET:?}"))?
            .map_err(|e| e.to_string())?;
        Ok(match u64::try_from(wait_ms) {
            Ok(wait_ms) => Err(Duration::from_millis(wait_ms)),
            Err(_) => Ok(()),
        })
    }

    fn charge_locally(&self, key: &str, cost: u32, limit: u32) -> Result<(), Duration> {
        let now = Instant::now();
        let mut entry = self.buckets.entry(key.to_owned()).or_insert((now, 0));
        let elapsed = now.duration_since(entry.0);
        if elapsed > self.window {
            *entry = (now, cost);
            Ok(())
        } else if entry.1.saturating_add(cost) <= limit {
            entry.1 += cost;
            Ok(())
        } else {
            Err(self.window.saturating_sub(elapsed))
        }
    }
}

/// Logs that checks are being counted per replica, at most once per
/// [`OUTAGE_WARNING_INTERVAL_SECS`] for the whole process, since during an outage every limited
/// request lands here.
fn warn_counting_locally(group: &str, reason: &str) {
    static LAST_WARNED_SECS: AtomicU64 = AtomicU64::new(0);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    let last = LAST_WARNED_SECS.load(Ordering::Relaxed);
    if now.saturating_sub(last) >= OUTAGE_WARNING_INTERVAL_SECS
        && LAST_WARNED_SECS
            .compare_exchange(last, now, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
    {
        tracing::warn!(
            group,
            reason,
            "rate limit: Redis unavailable, counting in this replica only until it answers"
        );
    }
}

/// A 429 that says how long until the caller's window resets.
pub(crate) fn too_many_requests_after(retry_after: Duration) -> Response {
    let mut response = too_many_requests();
    let seconds = retry_after.as_secs().max(1).to_string();
    if let Ok(value) = axum::http::HeaderValue::from_str(&seconds) {
        response
            .headers_mut()
            .insert(axum::http::header::RETRY_AFTER, value);
    }
    response
}

fn too_many_requests() -> Response {
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
    match limiter.check(&claims.sub).await {
        Ok(()) => next.run(req).await,
        Err(retry_after) => too_many_requests_after(retry_after),
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
    match limiter.check("global").await {
        Ok(()) => next.run(req).await,
        Err(retry_after) => too_many_requests_after(retry_after),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn local(limit: u32, window: Duration) -> RateLimiter {
        RateLimiter::new(&RateLimitStore::Local, "test", limit, window)
    }

    #[tokio::test]
    async fn allows_up_to_the_limit_then_rejects() {
        let limiter = local(3, Duration::from_secs(60));
        assert!(limiter.allow("a").await);
        assert!(limiter.allow("a").await);
        assert!(limiter.allow("a").await);
        assert!(
            !limiter.allow("a").await,
            "4th request in the window must be rejected"
        );
    }

    #[tokio::test]
    async fn keys_are_independent() {
        let limiter = local(1, Duration::from_secs(60));
        assert!(limiter.allow("a").await);
        assert!(
            limiter.allow("b").await,
            "a different key must have its own budget"
        );
        assert!(!limiter.allow("a").await);
    }

    #[tokio::test]
    async fn weighted_cost_is_all_or_nothing_and_reports_the_reset() {
        let limiter = local(10, Duration::from_secs(60));
        assert!(limiter.try_consume("a", 6).await.is_ok());
        let retry_after = limiter.try_consume("a", 5).await.unwrap_err();
        assert!(retry_after <= Duration::from_secs(60) && retry_after > Duration::from_secs(55));
        assert!(
            limiter.try_consume("a", 4).await.is_ok(),
            "a rejected cost must not be recorded"
        );
        assert!(limiter.try_consume("a", 1).await.is_err());
    }

    #[tokio::test]
    async fn cost_above_the_limit_is_charged_as_the_limit() {
        let limiter = local(3, Duration::from_secs(60));
        assert!(limiter.try_consume("a", 100).await.is_ok());
        assert!(limiter.try_consume("a", 1).await.is_err());
    }

    #[tokio::test]
    async fn a_refusal_names_the_time_left_in_the_window() {
        let window = Duration::from_secs(60);
        let limiter = local(1, window);
        assert_eq!(limiter.check("a").await, Ok(()));
        tokio::time::sleep(Duration::from_millis(20)).await;
        let wait = limiter.check("a").await.expect_err("over the limit");
        assert!(wait <= window - Duration::from_millis(20), "{wait:?}");
        assert!(wait > window - Duration::from_secs(5), "{wait:?}");
    }

    #[tokio::test]
    async fn a_raised_limit_applies_to_the_window_already_open() {
        let limiter = local(1, Duration::from_secs(60));
        assert_eq!(limiter.check_with_limit("a", 1).await, Ok(()));
        assert!(limiter.check_with_limit("a", 1).await.is_err());
        assert_eq!(limiter.check_with_limit("a", 2).await, Ok(()));
        assert!(limiter.check_with_limit("a", 2).await.is_err());
    }

    /// Nothing listens on port 1, so every check falls back to this process's count — which
    /// still enforces the limit rather than admitting everything.
    #[tokio::test]
    async fn without_redis_the_limit_is_counted_in_process() {
        let unreachable =
            SharedRedis::new(redis::Client::open("redis://127.0.0.1:1/").expect("valid url"));
        let limiter = RateLimiter::new(
            &RateLimitStore::Redis(unreachable),
            "test",
            2,
            Duration::from_secs(60),
        );
        assert!(limiter.allow("a").await);
        assert!(limiter.allow("a").await);
        assert!(!limiter.allow("a").await);
    }

    // These need a running Redis at REDIS_URL (default redis://127.0.0.1:6379). Run them with
    // `cargo test -p nasiko-server --lib rate_limit -- --include-ignored`.

    fn redis_store() -> RateLimitStore {
        let url = std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6379/".into());
        RateLimitStore::Redis(SharedRedis::new(
            redis::Client::open(url).expect("valid url"),
        ))
    }

    /// A key no earlier run has counted against.
    fn fresh_key() -> String {
        uuid::Uuid::new_v4().to_string()
    }

    #[tokio::test]
    #[ignore = "requires Redis"]
    async fn replicas_share_one_budget_through_redis() {
        let store = redis_store();
        let window = Duration::from_secs(60);
        let replica_a = RateLimiter::new(&store, "test-shared", 3, window);
        let replica_b = RateLimiter::new(&store, "test-shared", 3, window);
        let key = fresh_key();
        assert!(replica_a.allow(&key).await);
        assert!(replica_b.allow(&key).await);
        assert!(replica_a.allow(&key).await);
        let wait = replica_b.check(&key).await.expect_err("budget spent");
        assert!(
            wait <= window && wait > window - Duration::from_secs(5),
            "{wait:?}"
        );
    }

    #[tokio::test]
    #[ignore = "requires Redis"]
    async fn groups_keep_separate_counts_in_redis() {
        let store = redis_store();
        let login = RateLimiter::new(&store, "test-group-a", 1, Duration::from_secs(60));
        let other = RateLimiter::new(&store, "test-group-b", 1, Duration::from_secs(60));
        let key = fresh_key();
        assert!(login.allow(&key).await);
        assert!(other.allow(&key).await, "another group has its own count");
        assert!(!login.allow(&key).await);
    }

    #[tokio::test]
    #[ignore = "requires Redis"]
    async fn a_weighted_cost_in_redis_is_all_or_nothing() {
        let limiter = RateLimiter::new(&redis_store(), "test-cost", 10, Duration::from_secs(60));
        let key = fresh_key();
        assert!(limiter.try_consume(&key, 6).await.is_ok());
        assert!(limiter.try_consume(&key, 5).await.is_err());
        assert!(limiter.try_consume(&key, 4).await.is_ok());
        assert!(limiter.try_consume(&key, 1).await.is_err());
        assert_eq!(
            limiter.check_with_limit(&key, 11).await,
            Ok(()),
            "a raised limit applies to the open window"
        );
    }

    #[tokio::test]
    #[ignore = "requires Redis"]
    async fn a_redis_window_resets_when_it_expires() {
        let limiter = RateLimiter::new(&redis_store(), "test-reset", 1, Duration::from_millis(50));
        let key = fresh_key();
        assert!(limiter.allow(&key).await);
        assert!(!limiter.allow(&key).await);
        tokio::time::sleep(Duration::from_millis(80)).await;
        assert!(limiter.allow(&key).await);
    }

    #[tokio::test]
    async fn a_global_refusal_carries_retry_after() {
        use axum::body::Body;
        use axum::routing::get;
        use tower::ServiceExt;

        let limiter = local(1, Duration::from_secs(60));
        let app = axum::Router::new()
            .route("/", get(|| async { "ok" }))
            .layer(axum::middleware::from_fn_with_state(
                limiter,
                limit_globally,
            ));
        let call = || {
            app.clone()
                .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
        };

        assert_eq!(call().await.unwrap().status(), StatusCode::OK);
        let refused = call().await.unwrap();
        assert_eq!(refused.status(), StatusCode::TOO_MANY_REQUESTS);
        let seconds: u64 = refused
            .headers()
            .get(axum::http::header::RETRY_AFTER)
            .expect("a 429 must say when to retry")
            .to_str()
            .unwrap()
            .parse()
            .unwrap();
        assert!((1..=60).contains(&seconds), "{seconds}");
    }

    #[tokio::test]
    async fn resets_after_the_window_elapses() {
        let limiter = local(1, Duration::from_millis(20));
        assert!(limiter.allow("a").await);
        assert!(!limiter.allow("a").await);
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert!(
            limiter.allow("a").await,
            "a new window must reset the budget"
        );
    }
}

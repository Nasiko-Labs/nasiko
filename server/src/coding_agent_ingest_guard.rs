//! Self-protection for `POST /api/telemetry/coding-agent/events/batch`.
//!
//! A history import sends every past turn a user has, and many users import at
//! once. Without bounds the route pushes Postgres as fast as the clients can
//! send. Three valves, each answering in a way the client can act on:
//!
//! - `413` — the request is bigger than one batch may be; split it.
//! - `429` + `Retry-After` — this user has sent their turns for the minute.
//! - `503` + `Retry-After` — the server is persisting as many batches as it
//!   will at once; try again in a moment.
//!
//! All three are plain text and reject the whole batch: nothing is persisted,
//! so a retry of the same batch is the complete recovery.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use axum::extract::{Request, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use nasiko_config::CodingAgentIngestConfig;
use opentelemetry::{KeyValue, global, metrics::Counter};
use rand::Rng;
use tokio::sync::Semaphore;

use crate::rate_limit::RateLimiter;
use crate::state::AppState;

/// The per-user budget window.
const RATE_WINDOW: Duration = Duration::from_secs(60);
/// Bounds of the `Retry-After` a `503` asks for. Jittered so a crowd of clients
/// turned away together does not return together.
const OVERLOAD_RETRY_SECS: std::ops::RangeInclusive<u64> = 2..=5;

/// The ingest route's rate limiter, concurrency cap and counters. One per
/// process, held on `AppState`.
#[derive(Clone)]
pub struct IngestGuard {
    events: RateLimiter,
    permits: Arc<Semaphore>,
    pub metrics: IngestMetrics,
}

impl IngestGuard {
    pub fn new(config: &CodingAgentIngestConfig) -> Self {
        Self {
            events: RateLimiter::new(config.events_per_min, RATE_WINDOW),
            permits: Arc::new(Semaphore::new(config.max_concurrent)),
            metrics: IngestMetrics::new(),
        }
    }

    /// Charge `count` events to `user`'s budget, or the `429` to answer with.
    pub(crate) fn admit_events(&self, user: &str, count: usize) -> Result<(), Response> {
        let cost = u32::try_from(count).unwrap_or(u32::MAX);
        self.events.try_consume(user, cost).map_err(|reset| {
            let retry_after = reset.as_secs().max(1);
            self.metrics.record_rejection(StatusCode::TOO_MANY_REQUESTS);
            tracing::info!(
                user_id = user,
                events = count,
                retry_after,
                "coding-agent ingest rate-limited"
            );
            plain(
                StatusCode::TOO_MANY_REQUESTS,
                format!(
                    "coding-agent ingest rate limit exceeded: at most {} turns per minute; \
                     retry after {retry_after}s",
                    self.events.limit()
                ),
                Some(retry_after),
            )
        })
    }

    /// The `413` for a batch the route will not take, with what to do about it.
    pub(crate) fn too_large(&self, reason: &str) -> Response {
        self.metrics.record_rejection(StatusCode::PAYLOAD_TOO_LARGE);
        tracing::info!(reason, "coding-agent ingest batch too large");
        plain(
            StatusCode::PAYLOAD_TOO_LARGE,
            format!("{reason}; split the batch and send the parts separately"),
            None,
        )
    }
}

/// Route middleware: hold one of the server-wide permits for the whole request
/// — body read, parse and every database write — or answer `503` at once.
/// Ahead of the body extractor on purpose, so an overloaded server does not
/// first buffer up to 8 MB it is about to turn away.
pub(crate) async fn limit_concurrency(
    State(state): State<AppState>,
    req: Request,
    next: Next,
) -> Response {
    let guard = &state.coding_agent_ingest;
    let Ok(_permit) = guard.permits.clone().try_acquire_owned() else {
        guard
            .metrics
            .record_rejection(StatusCode::SERVICE_UNAVAILABLE);
        let retry_after = rand::rng().random_range(OVERLOAD_RETRY_SECS);
        tracing::info!(retry_after, "coding-agent ingest at capacity");
        return plain(
            StatusCode::SERVICE_UNAVAILABLE,
            format!("coding-agent ingest is at capacity; retry after {retry_after}s"),
            Some(retry_after),
        );
    };
    next.run(req).await
}

fn plain(status: StatusCode, message: String, retry_after: Option<u64>) -> Response {
    let mut response = (status, message).into_response();
    if let Some(secs) = retry_after {
        response
            .headers_mut()
            .insert(header::RETRY_AFTER, HeaderValue::from(secs));
    }
    response
}

/// Ingest counters: exported as OTel metrics, and summarized in the backlog
/// log line (`coding_agent_backlog`) for deployments without a metrics
/// pipeline.
#[derive(Clone)]
pub struct IngestMetrics {
    window: Arc<WindowCounts>,
    events: Counter<u64>,
    rejections: Counter<u64>,
}

/// Counts since the last backlog report.
#[derive(Default)]
struct WindowCounts {
    accepted: AtomicU64,
    duplicate: AtomicU64,
    rejected: AtomicU64,
    too_large: AtomicU64,
    rate_limited: AtomicU64,
    overloaded: AtomicU64,
}

/// One reporting window's counts, taken (and reset) by [`IngestMetrics::take`].
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct IngestCounts {
    pub accepted: u64,
    pub duplicate: u64,
    pub rejected: u64,
    pub too_large: u64,
    pub rate_limited: u64,
    pub overloaded: u64,
}

impl IngestCounts {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

impl IngestMetrics {
    fn new() -> Self {
        let meter = global::meter("nasiko.cp");
        Self {
            window: Arc::default(),
            events: meter
                .u64_counter("nasiko.coding_agent.ingest.events")
                .with_description("Coding-agent turns ingested, by outcome")
                .build(),
            rejections: meter
                .u64_counter("nasiko.coding_agent.ingest.rejections")
                .with_description("Coding-agent ingest batches turned away, by HTTP status")
                .build(),
        }
    }

    /// Per-event outcomes of one processed batch.
    pub(crate) fn record_events(&self, accepted: u64, duplicate: u64, rejected: u64) {
        for (outcome, count, total) in [
            ("accepted", accepted, &self.window.accepted),
            ("duplicate", duplicate, &self.window.duplicate),
            ("rejected", rejected, &self.window.rejected),
        ] {
            if count > 0 {
                total.fetch_add(count, Ordering::Relaxed);
                self.events.add(count, &[KeyValue::new("outcome", outcome)]);
            }
        }
    }

    fn record_rejection(&self, status: StatusCode) {
        let total = match status {
            StatusCode::PAYLOAD_TOO_LARGE => &self.window.too_large,
            StatusCode::TOO_MANY_REQUESTS => &self.window.rate_limited,
            _ => &self.window.overloaded,
        };
        total.fetch_add(1, Ordering::Relaxed);
        self.rejections.add(
            1,
            &[KeyValue::new(
                "http.response.status_code",
                i64::from(status.as_u16()),
            )],
        );
    }

    /// The counts since the previous call.
    pub fn take(&self) -> IngestCounts {
        let take = |count: &AtomicU64| count.swap(0, Ordering::Relaxed);
        IngestCounts {
            accepted: take(&self.window.accepted),
            duplicate: take(&self.window.duplicate),
            rejected: take(&self.window.rejected),
            too_large: take(&self.window.too_large),
            rate_limited: take(&self.window.rate_limited),
            overloaded: take(&self.window.overloaded),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn guard(events_per_min: u32) -> IngestGuard {
        IngestGuard::new(&CodingAgentIngestConfig {
            events_per_min,
            ..Default::default()
        })
    }

    #[test]
    fn rate_limit_counts_events_and_answers_429_with_retry_after() {
        let guard = guard(150);
        assert!(guard.admit_events("user", 100).is_ok());
        let response = guard.admit_events("user", 100).unwrap_err();
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        let retry_after: u64 = response.headers()[header::RETRY_AFTER]
            .to_str()
            .unwrap()
            .parse()
            .unwrap();
        assert!((1..=60).contains(&retry_after));
        assert!(
            guard.admit_events("other", 100).is_ok(),
            "each user has their own budget"
        );
        assert_eq!(guard.metrics.take().rate_limited, 1);
    }

    #[test]
    fn too_large_is_a_plain_413_without_retry_after() {
        let guard = guard(10);
        let response = guard.too_large("at most 100 events per batch");
        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
        assert!(response.headers().get(header::RETRY_AFTER).is_none());
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "text/plain; charset=utf-8"
        );
    }

    #[test]
    fn take_returns_the_window_and_resets_it() {
        let metrics = guard(10).metrics;
        metrics.record_events(3, 1, 0);
        metrics.record_rejection(StatusCode::SERVICE_UNAVAILABLE);
        let counts = metrics.take();
        assert_eq!(
            counts,
            IngestCounts {
                accepted: 3,
                duplicate: 1,
                overloaded: 1,
                ..Default::default()
            }
        );
        assert!(metrics.take().is_empty());
    }
}

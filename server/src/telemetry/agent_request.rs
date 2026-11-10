//! Per-agent request load: how many requests are in flight to each agent, how long they take at
//! each level of concurrency, and how they end, keyed by agent UUID (names are reused and renamed,
//! ids are not). Recorded at every hop that sends a request to an agent:
//! - the direct proxy (`agent_proxy.rs`);
//! - the explicit-agent dispatch (`agent_stream` in `router/a2a_dispatch.rs`);
//! - the ReAct orchestrator's agent tool calls, through `CpCallGuard::before_call` (`acl.rs`);
//! - the MAF executor's step calls, fresh and resumed, through the meter `lib.rs` hands
//!   `start_worker`;
//! - each HITL resume delivery (`hitl/mod.rs`).
//!
//! The last three reach agents from code outside this crate or around a retry loop; the
//! orchestrator crates see only the `nasiko_types::agent_call` seam, implemented here by
//! [`AgentRequestMetrics`] and [`AgentRequestGuard`].
//!
//! The in-flight count is kept in-process as well as exported, because the duration histogram is
//! labelled with the band a request started in and an UpDownCounter cannot be read back. The band
//! is the load on one replica: the agent's in-flight count divided by its ready replicas, rounded
//! up (`agent_replicas.rs` keeps that count off the request path). The `nasiko.agent.inflight`
//! gauge stays agent-wide. Both counts are per server process; the exported series are
//! per-process too, so a deployment running several server processes must give each a distinct
//! resource identity.

use dashmap::{DashMap, mapref::entry::Entry};
use nasiko_types::agent_call::{AgentCallHandle, AgentCallMeter};
use opentelemetry::{
    KeyValue,
    metrics::{Counter, Histogram, Meter, UpDownCounter},
};
use std::{sync::Arc, time::Instant};

use super::agent_replicas::ReadyReplicas;
use super::genai::attr;

/// Attribute carrying the in-flight band a request started in.
pub const INFLIGHT_BAND: &str = "nasiko.inflight_band";
const OUTCOME: &str = "outcome";

/// Histogram buckets (seconds) for agent turns: a fast reply is about half a second, a long
/// multi-tool turn runs to the call timeout (five minutes by default). The SDK defaults are tuned
/// for milliseconds and would lump every real turn into a couple of buckets.
///
/// Every bucket is a Prometheus series per agent, in-flight band and outcome, so the list is kept
/// short: fine below 10 s, where typical turns and the capacity estimate's medians land, and an
/// edge at 60 s, the default p95 ceiling the autoscaler and its alert compare against, so that
/// comparison counts requests exactly rather than interpolating inside a bucket.
const REQUEST_DURATION_BUCKETS_SECS: [f64; 8] = [0.5, 1.0, 2.0, 5.0, 10.0, 30.0, 60.0, 300.0];

/// How a proxied request ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RequestOutcome {
    Ok,
    Error,
    /// The caller went away (or the guard was dropped) before the request reached a result.
    Cancelled,
}

impl RequestOutcome {
    /// The outcome of an upstream reply: a success status is `Ok`, anything else `Error`.
    pub fn from_success(success: bool) -> Self {
        if success { Self::Ok } else { Self::Error }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Error => "error",
            Self::Cancelled => "cancelled",
        }
    }
}

/// The band for a request that found `inflight` requests (itself included) in flight to its agent.
pub fn inflight_band(inflight: u64) -> &'static str {
    match inflight {
        0..=1 => "1",
        2..=4 => "2-4",
        5..=8 => "5-8",
        9..=16 => "9-16",
        _ => "17+",
    }
}

/// Requests in flight on one replica when `agent_inflight` are in flight across `ready_replicas`,
/// rounded up so a replica holding the remainder is not undercounted. 0 replicas counts as one.
fn per_replica_inflight(agent_inflight: u64, ready_replicas: u64) -> u64 {
    agent_inflight.div_ceil(ready_replicas.max(1))
}

/// Instruments plus the in-process counts behind the band.
#[derive(Clone)]
pub struct AgentRequestMetrics {
    inflight_gauge: UpDownCounter<i64>,
    duration: Histogram<f64>,
    requests: Counter<u64>,
    inflight: Arc<DashMap<String, u64>>,
    ready_replicas: ReadyReplicas,
}

impl AgentRequestMetrics {
    pub fn new(meter: &Meter) -> Self {
        Self {
            inflight_gauge: meter
                .i64_up_down_counter("nasiko.agent.inflight")
                .with_description("Requests currently in flight to an agent")
                .build(),
            duration: meter
                .f64_histogram("nasiko.agent.request.duration")
                .with_description(
                    "Seconds a request to an agent took, by the in-flight band it started in",
                )
                .with_unit("s")
                .with_boundaries(REQUEST_DURATION_BUCKETS_SECS.to_vec())
                .build(),
            requests: meter
                .u64_counter("nasiko.agent.requests")
                .with_description("Requests forwarded to an agent, by outcome")
                .build(),
            inflight: Arc::new(DashMap::new()),
            ready_replicas: ReadyReplicas::default(),
        }
    }

    /// The ready-replica cache the band divides by, for the refresher to keep current.
    pub fn ready_replicas(&self) -> ReadyReplicas {
        self.ready_replicas.clone()
    }

    /// Count a request as in flight until the returned guard is dropped. The guard starts as
    /// `Error`, so an early `?` return between here and the result is reported as a failure.
    pub fn begin(&self, agent_id: &str) -> AgentRequestGuard {
        let count = {
            let mut slot = self.inflight.entry(agent_id.to_string()).or_insert(0);
            *slot += 1;
            *slot
        };
        let started = Instant::now();
        let ready = self.ready_replicas.note_request(agent_id, started);
        self.inflight_gauge.add(
            1,
            &[KeyValue::new(attr::NASIKO_AGENT_ID, agent_id.to_string())],
        );
        AgentRequestGuard {
            metrics: self.clone(),
            agent_id: agent_id.to_string(),
            band: inflight_band(per_replica_inflight(count, ready)),
            started,
            outcome: RequestOutcome::Error,
        }
    }

    fn release(&self, agent_id: &str) {
        // Remove the entry at zero so the map is bounded by agents currently being called.
        if let Entry::Occupied(mut slot) = self.inflight.entry(agent_id.to_string()) {
            let count = slot.get().saturating_sub(1);
            if count == 0 {
                slot.remove();
            } else {
                *slot.get_mut() = count;
            }
        }
        self.inflight_gauge.add(
            -1,
            &[KeyValue::new(attr::NASIKO_AGENT_ID, agent_id.to_string())],
        );
    }

    /// Requests to `agent_id` in flight from this process right now.
    pub fn inflight_now(&self, agent_id: &str) -> u64 {
        self.inflight.get(agent_id).map_or(0, |count| *count)
    }
}

/// The seam crates outside the server (the ReAct orchestrator, the MAF executor) count their
/// own direct agent calls through.
impl AgentCallMeter for AgentRequestMetrics {
    fn begin(&self, agent_id: uuid::Uuid) -> Box<dyn AgentCallHandle> {
        let mut guard = AgentRequestMetrics::begin(self, &agent_id.to_string());
        // The seam's contract: a handle dropped before it is told a result is a caller that
        // stopped waiting.
        guard.set_outcome(RequestOutcome::Cancelled);
        Box::new(guard)
    }
}

/// Holds one in-flight count. Dropping it on any path (normal return, `?`, a streamed body that
/// ends, a client that disconnects mid-stream) decrements the count and records the duration and
/// outcome, so no path can leak a count.
pub struct AgentRequestGuard {
    metrics: AgentRequestMetrics,
    agent_id: String,
    band: &'static str,
    started: Instant,
    outcome: RequestOutcome,
}

impl AgentRequestGuard {
    pub fn set_outcome(&mut self, outcome: RequestOutcome) {
        self.outcome = outcome;
    }

    pub fn outcome(&self) -> RequestOutcome {
        self.outcome
    }

    /// Apply `outcome` only if nothing worse was already recorded. Used when a stream reaches
    /// its natural end: a mid-stream error stays an error, a plain end takes the upstream status.
    pub fn settle(&mut self, outcome: RequestOutcome) {
        if self.outcome == RequestOutcome::Cancelled {
            self.outcome = outcome;
        }
    }

    pub fn band(&self) -> &'static str {
        self.band
    }
}

impl AgentCallHandle for AgentRequestGuard {
    fn set_succeeded(&mut self, succeeded: bool) {
        self.set_outcome(RequestOutcome::from_success(succeeded));
    }
}

impl Drop for AgentRequestGuard {
    fn drop(&mut self) {
        self.metrics.release(&self.agent_id);
        let attrs = [
            KeyValue::new(attr::NASIKO_AGENT_ID, self.agent_id.clone()),
            KeyValue::new(INFLIGHT_BAND, self.band),
            KeyValue::new(OUTCOME, self.outcome.as_str()),
        ];
        self.metrics
            .duration
            .record(self.started.elapsed().as_secs_f64(), &attrs);
        self.metrics.requests.add(
            1,
            &[
                attrs[0].clone(),
                KeyValue::new(OUTCOME, self.outcome.as_str()),
            ],
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metrics() -> AgentRequestMetrics {
        // The global provider is a no-op in tests; the count under test is in-process.
        AgentRequestMetrics::new(&opentelemetry::global::meter("test"))
    }

    #[test]
    fn band_boundaries() {
        let cases = [
            (0, "1"),
            (1, "1"),
            (2, "2-4"),
            (4, "2-4"),
            (5, "5-8"),
            (8, "5-8"),
            (9, "9-16"),
            (16, "9-16"),
            (17, "17+"),
            (500, "17+"),
        ];
        for (inflight, band) in cases {
            assert_eq!(inflight_band(inflight), band, "inflight {inflight}");
        }
    }

    #[test]
    fn outcome_follows_upstream_success() {
        assert_eq!(RequestOutcome::from_success(true), RequestOutcome::Ok);
        assert_eq!(RequestOutcome::from_success(false), RequestOutcome::Error);
    }

    #[test]
    fn guard_counts_concurrent_requests_and_assigns_bands() {
        let m = metrics();
        let first = m.begin("a");
        let second = m.begin("a");
        let other_agent = m.begin("b");
        assert_eq!(
            (first.band(), second.band(), other_agent.band()),
            ("1", "2-4", "1")
        );
        assert_eq!(m.inflight_now("a"), 2);
        assert_eq!(m.inflight_now("b"), 1);
    }

    #[test]
    fn the_band_is_the_in_flight_count_per_ready_replica() {
        let m = metrics();
        let replicas = m.ready_replicas();
        replicas.note_request("a", Instant::now());
        replicas.set_ready("a", 2);
        let guards: Vec<_> = (0..10).map(|_| m.begin("a")).collect();
        // 4 in flight on 2 replicas is 2 each; 10 is 5 each, where the agent-wide count of 10
        // would have been "9-16".
        assert_eq!(guards[3].band(), "2-4");
        assert_eq!(guards[9].band(), "5-8");
        assert_eq!(m.inflight_now("a"), 10);
    }

    #[test]
    fn per_replica_inflight_rounds_up_and_treats_no_replicas_as_one() {
        assert_eq!(per_replica_inflight(10, 2), 5);
        assert_eq!(per_replica_inflight(5, 2), 3);
        assert_eq!(per_replica_inflight(3, 0), 3);
        assert_eq!(per_replica_inflight(1, 4), 1);
    }

    #[test]
    fn dropping_after_ok_releases_the_count() {
        let m = metrics();
        let mut guard = m.begin("a");
        guard.set_outcome(RequestOutcome::Ok);
        drop(guard);
        assert_eq!(m.inflight_now("a"), 0);
    }

    #[test]
    fn dropping_on_the_error_path_releases_the_count() {
        let m = metrics();
        let guard = m.begin("a");
        assert_eq!(guard.outcome(), RequestOutcome::Error);
        drop(guard);
        assert_eq!(m.inflight_now("a"), 0);
    }

    #[test]
    fn dropping_an_early_return_releases_the_count() {
        fn fails_before_a_result(m: &AgentRequestMetrics) -> Result<(), ()> {
            let _guard = m.begin("a");
            Err(())
        }
        let m = metrics();
        assert!(fails_before_a_result(&m).is_err());
        assert_eq!(m.inflight_now("a"), 0);
    }

    #[test]
    fn dropping_a_stream_guard_mid_flight_releases_the_count_as_cancelled() {
        let m = metrics();
        let mut guard = m.begin("a");
        guard.set_outcome(RequestOutcome::Cancelled);
        // The relay owns the guard; a disconnecting client drops the relay.
        let relay = Some(guard);
        assert_eq!(m.inflight_now("a"), 1);
        drop(relay);
        assert_eq!(m.inflight_now("a"), 0);
    }

    #[test]
    fn settle_keeps_a_mid_stream_error() {
        let m = metrics();
        let mut guard = m.begin("a");
        guard.set_outcome(RequestOutcome::Cancelled);
        guard.settle(RequestOutcome::Ok);
        assert_eq!(guard.outcome(), RequestOutcome::Ok);

        guard.set_outcome(RequestOutcome::Error);
        guard.settle(RequestOutcome::Ok);
        assert_eq!(guard.outcome(), RequestOutcome::Error);
    }
}

//! The shared Resume Dispatcher (M6) — turns a resolved `hitl_requests` row
//! into an actual push into the paused conversation, closing the gap M5 left
//! open ("resolving a row today has no effect an agent could ever observe").
//!
//! Shared across every `HitlOrigin`, not MCP-specific: this module only knows
//! how to claim a resolved row and hand it to a [`ResumeNotifier`] — it has no
//! opinion on *how* delivery happens, deliberately, so `direct_chat`/
//! `orchestrator`/`maf` origins can reuse the same claim/lease machinery with
//! a different notifier later. `crate::notifier::RuntimeResumeNotifier` is
//! the one concrete transport this milestone ships, proven against MCP's
//! `mcp_tool` origin.
//!
//! Structurally mirrors `oss/server/src/agents/build_worker.rs`: a poll loop
//! that drains the claim queue on each tick, a slower periodic sweep that
//! quarantines abandoned claims, and panic isolation around the per-row work
//! so one bad row can't take down the loop.

use std::sync::Arc;
use std::time::Duration;

use sqlx::PgPool;
use uuid::Uuid;

use crate::repo::{self, DEFAULT_RESUME_LEASE_MINUTES};
use crate::types::{HitlRequest, ResumeStatus};

/// Why a [`ResumeNotifier`] failed to deliver a resume push.
///
/// [`NotifyError::is_permanent`] tells the dispatcher's retry loop whether another attempt could
/// ever change the outcome — `MissingContextId` and a confirmed "no such agent" cannot, no matter
/// how many times they're retried, so retrying them up to `max_attempts` (found in review) only
/// delays recording the terminal `failed` status. A DB blip, a container that isn't up yet, and a
/// transport/peer error are all worth retrying, since the next attempt might land differently.
#[derive(Debug, thiserror::Error)]
pub enum NotifyError {
    #[error("hitl request {0} has no context_id to resume against")]
    MissingContextId(Uuid),
    #[error("could not resolve a live endpoint for agent {agent_id}: {reason}")]
    EndpointResolution {
        agent_id: Uuid,
        reason: String,
        /// Set only when `reason` is definitionally unrecoverable (e.g. the agent row itself
        /// doesn't exist) — a DB lookup failure or "no live/stored endpoint right now" is
        /// transient and worth retrying.
        permanent: bool,
    },
    /// The flow the agent's retry would have to authenticate against is older than the platform's
    /// flow timeout, so `mcp/handlers/gateway.rs`'s `flow_user` will reject it no matter what this
    /// nudge does. Permanent by construction: `flows.created_at` is fixed and only gets further
    /// away.
    #[error(
        "context {context_id} names a flow that is no longer live, so the agent's retried tool \
         call could not be authorized"
    )]
    FlowNotLive { context_id: String },
    #[error("transport error delivering resume notification: {0}")]
    Transport(#[from] reqwest::Error),
    #[error("peer rejected the resume notification: {0}")]
    PeerError(String),
}

impl NotifyError {
    pub fn is_permanent(&self) -> bool {
        match self {
            NotifyError::MissingContextId(_) | NotifyError::FlowNotLive { .. } => true,
            NotifyError::EndpointResolution { permanent, .. } => *permanent,
            NotifyError::Transport(_) | NotifyError::PeerError(_) => false,
        }
    }
}

/// Delivers a resolved [`HitlRequest`]'s decision into whatever is paused
/// waiting for it. Transport-agnostic by design — the dispatcher loop only
/// ever depends on this trait, never on a concrete HTTP/A2A client, so the
/// claim/lease machinery below is reusable for a non-MCP origin with an
/// entirely different delivery mechanism.
#[async_trait::async_trait]
pub trait ResumeNotifier: Send + Sync {
    async fn notify(&self, request: &HitlRequest) -> Result<(), NotifyError>;
}

/// Tunables for [`run`]. `Default` matches `build_worker`'s own constants
/// (5s poll, 10 min recovery sweep) where a direct analogue exists.
#[derive(Debug, Clone)]
pub struct DispatcherConfig {
    /// How often to check for newly resolved rows when the queue was empty
    /// on the last pass.
    pub poll_interval: Duration,
    /// How often to run `recover_stuck_resumes`.
    pub recovery_interval: Duration,
    /// Lease staleness threshold passed to `recover_stuck_resumes` — a claim
    /// older than this with no recorded outcome is presumed abandoned.
    pub lease_minutes: i64,
    /// Total outbound attempts made per claim before giving up and recording
    /// `ResumeStatus::Failed`.
    pub max_attempts: u32,
    /// Delay between in-process retry attempts.
    pub retry_delay: Duration,
}

impl Default for DispatcherConfig {
    fn default() -> Self {
        Self {
            poll_interval: Duration::from_secs(5),
            recovery_interval: Duration::from_secs(10 * 60),
            lease_minutes: DEFAULT_RESUME_LEASE_MINUTES,
            max_attempts: 3,
            retry_delay: Duration::from_secs(2),
        }
    }
}

impl DispatcherConfig {
    /// The claim lease actually used, which is never shorter than one whole delivery.
    ///
    /// `dispatch_one` holds its claim across every in-process retry, so the worst case is
    /// `max_attempts` requests at `notifier::RESUME_REQUEST_TIMEOUT_SECS` each plus the backoffs
    /// between them — roughly 15 minutes on the defaults. `lease_minutes` defaulted to 2, so the
    /// recovery tick could land mid-delivery, flip the claim to `delivery_outcome_unknown`, and
    /// leave the subsequent `finish_resume` (`WHERE resume_status = 'not_started'`) updating
    /// nothing: a resume that actually succeeded recorded permanently as unknown, needing a
    /// superuser requeue that re-delivers the nudge (found in review).
    ///
    /// Derived rather than documented because every input is independently env-tunable
    /// (`HITL_RESUME_MAX_ATTEMPTS`, `HITL_RESUME_RETRY_DELAY_SECS`, `HITL_RESUME_LEASE_MINUTES`),
    /// so a fixed default would go stale the moment one of them is raised. The sibling dispatcher
    /// (`oss/server/src/hitl/mod.rs`'s `lease_secs` helper) enforces the same
    /// never-shorter-than-`HITL_RESUME_LEASE_MINUTES` invariant off the same config field, floored
    /// at what its own delivery shape needs instead of this one's — it claims once per delivery
    /// attempt rather than holding one claim across a whole in-process retry loop, so its floor
    /// only has to clear a single request's timeout, not every retry's.
    fn effective_lease_minutes(&self) -> i64 {
        let attempts = i64::from(self.max_attempts).max(1);
        let secs = attempts * crate::notifier::RESUME_REQUEST_TIMEOUT_SECS as i64
            + (attempts - 1) * self.retry_delay.as_secs() as i64;
        // Round up, and never shorten a lease an operator deliberately set longer.
        self.lease_minutes.max((secs + 59) / 60)
    }
}

/// Concurrent in-flight deliveries, mirroring `oss/server/src/hitl/mod.rs::run`'s own
/// `MAX_CONCURRENT_DELIVERIES` on the same claim/spawn shape. Before this, `spawn(...).await`
/// gave panic isolation but zero concurrency: one unreachable agent (`notifier.rs`'s 300s
/// transport timeout x `max_attempts` retries x `retry_delay`) blocked the entire drain for
/// minutes, and — since the `select!` in `run` sits outside the drain loop — blocked the recovery
/// sweep along with it.
const MAX_CONCURRENT_DELIVERIES: usize = 8;

/// Main resume-dispatcher loop. Spawned once at server startup (mirrors
/// `build_worker::run`'s own call site) and runs until the process exits —
/// there is no shutdown channel because, unlike the build worker, there is no
/// sender whose drop should end the loop; the task is simply aborted with the
/// rest of the process.
pub async fn run(db: PgPool, notifier: Arc<dyn ResumeNotifier>, config: DispatcherConfig) {
    let lease_minutes = config.effective_lease_minutes();
    if lease_minutes != config.lease_minutes {
        tracing::warn!(
            configured = config.lease_minutes,
            effective = lease_minutes,
            "resume dispatcher: HITL_RESUME_LEASE_MINUTES is shorter than one whole delivery \
             (max_attempts x the notifier's per-request timeout); using the longer value so the \
             recovery sweep cannot quarantine a delivery that is still in flight"
        );
    }
    if let Err(e) = repo::recover_stuck_resumes(&db, lease_minutes).await {
        tracing::error!(%e, "resume dispatcher: startup recovery sweep failed");
    }

    let recovery_start = tokio::time::Instant::now() + config.recovery_interval;
    let mut recovery_tick = tokio::time::interval_at(recovery_start, config.recovery_interval);
    recovery_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    tracing::info!("resume dispatcher: started");
    // Tracks in-flight `dispatch_one` calls across poll cycles so one slow/unreachable agent never
    // blocks claiming or delivering anything else — see `MAX_CONCURRENT_DELIVERIES`'s doc comment.
    let mut deliveries: tokio::task::JoinSet<()> = tokio::task::JoinSet::new();
    loop {
        tokio::select! {
            _ = tokio::time::sleep(config.poll_interval) => {}
            _ = recovery_tick.tick() => {
                match repo::recover_stuck_resumes(&db, lease_minutes).await {
                    Ok(0) => {}
                    Ok(n) => tracing::warn!(
                        count = n,
                        "resume dispatcher: quarantined stuck claims as delivery_outcome_unknown"
                    ),
                    Err(e) => tracing::error!(%e, "resume dispatcher: recovery sweep failed"),
                }
                // Same tick, no new timer: `mcp_session_tool_grants` had no periodic sweep at all
                // before this (found in review) — see `sweep_expired_session_grants`'s own doc
                // comment for why that lets it grow unbounded.
                match repo::sweep_expired_session_grants(&db).await {
                    Ok(0) => {}
                    Ok(n) => tracing::info!(count = n, "resume dispatcher: swept expired session grants"),
                    Err(e) => tracing::error!(%e, "resume dispatcher: session-grant sweep failed"),
                }
            }
        }

        // Drain: keep claiming while a delivery slot is free and the queue has a claimable row,
        // same pattern as `build_worker::run`. Claim runs here in the loop itself (minimal, no
        // panic risk); delivery runs in a spawned task tracked by `deliveries`, concurrently with
        // every other in-flight one.
        while deliveries.len() < MAX_CONCURRENT_DELIVERIES {
            let request = match repo::claim_for_resume(&db).await {
                Ok(Some(r)) => r,
                Ok(None) => break,
                Err(e) => {
                    tracing::error!(%e, "resume dispatcher: claim error");
                    break;
                }
            };

            let db = db.clone();
            let notifier = notifier.clone();
            let config = config.clone();
            deliveries.spawn(async move {
                dispatch_one(&db, notifier.as_ref(), request, &config).await;
            });
        }

        // Reap whatever has finished without blocking this tick — a still-running delivery is
        // simply left in `deliveries` and picked up on a later iteration, mirroring
        // `oss/server/src/hitl/mod.rs::run`'s own non-blocking reap.
        while let Some(result) = deliveries.try_join_next() {
            if let Err(e) = result
                && e.is_panic()
            {
                tracing::error!(
                    "resume dispatcher: task panicked — claim left unresolved, \
                     the recovery sweep will quarantine it as delivery_outcome_unknown"
                );
            }
        }
    }
}

/// Deliver one already-claimed row, retrying in-process up to
/// `config.max_attempts` times, then record the definitive outcome via
/// `repo::finish_resume`. Runs as a `tokio::task::spawn` target so a panic
/// inside a notifier implementation is isolated from the poll loop (mirrors
/// `build_worker::execute_claimed_job`'s own panic-isolation rationale).
async fn dispatch_one(
    db: &PgPool,
    notifier: &dyn ResumeNotifier,
    request: HitlRequest,
    config: &DispatcherConfig,
) {
    let request_id = request.id;
    let mut attempts = 0u32;

    let last_error = loop {
        attempts += 1;
        match notifier.notify(&request).await {
            Ok(()) => {
                tracing::info!(id = %request_id, attempts, "resume dispatcher: delivered");
                if let Err(e) = repo::finish_resume(
                    db,
                    request_id,
                    ResumeStatus::Completed,
                    attempts as i32,
                    None,
                )
                .await
                {
                    tracing::error!(id = %request_id, %e, "resume dispatcher: failed to record completion");
                }
                return;
            }
            Err(e) => {
                let permanent = e.is_permanent();
                let error = e.to_string();
                tracing::warn!(
                    id = %request_id,
                    attempt = attempts,
                    max_attempts = config.max_attempts,
                    permanent,
                    %error,
                    "resume dispatcher: delivery attempt failed"
                );
                if permanent || attempts >= config.max_attempts {
                    break error;
                }
                tokio::time::sleep(config.retry_delay).await;
            }
        }
    };

    if let Err(e) = repo::finish_resume(
        db,
        request_id,
        ResumeStatus::Failed,
        attempts as i32,
        Some(&last_error),
    )
    .await
    {
        tracing::error!(id = %request_id, %e, "resume dispatcher: failed to record failure");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The lease must outlast a whole delivery, not one request — `dispatch_one` holds its claim
    /// across every in-process retry. A shorter lease lets the recovery sweep quarantine a
    /// delivery still in flight, and `finish_resume`'s `WHERE resume_status = 'not_started'` then
    /// silently updates nothing, recording a successful resume as unknown.
    #[test]
    fn the_lease_always_covers_a_whole_delivery() {
        let defaults = DispatcherConfig::default();
        let worst_case_secs = i64::from(defaults.max_attempts)
            * crate::notifier::RESUME_REQUEST_TIMEOUT_SECS as i64
            + (i64::from(defaults.max_attempts) - 1) * defaults.retry_delay.as_secs() as i64;
        assert!(
            defaults.effective_lease_minutes() * 60 >= worst_case_secs,
            "default lease {} min does not cover {worst_case_secs}s of delivery",
            defaults.effective_lease_minutes()
        );

        // A too-short configured lease is raised, not obeyed — every input is env-tunable, so the
        // floor has to be derived rather than trusted.
        let starved = DispatcherConfig {
            lease_minutes: 2,
            ..DispatcherConfig::default()
        };
        assert_eq!(starved.effective_lease_minutes(), 16);

        // Raising attempts moves the floor with it.
        let chattier = DispatcherConfig {
            lease_minutes: 2,
            max_attempts: 6,
            ..DispatcherConfig::default()
        };
        assert!(
            chattier.effective_lease_minutes() > starved.effective_lease_minutes(),
            "more attempts must demand a longer lease"
        );

        // An operator who deliberately set a longer lease keeps it.
        let generous = DispatcherConfig {
            lease_minutes: 120,
            ..DispatcherConfig::default()
        };
        assert_eq!(generous.effective_lease_minutes(), 120);
    }

    #[test]
    fn missing_context_id_is_always_permanent() {
        assert!(NotifyError::MissingContextId(Uuid::new_v4()).is_permanent());
    }

    #[test]
    fn no_such_agent_is_permanent_but_other_endpoint_failures_are_not() {
        assert!(
            NotifyError::EndpointResolution {
                agent_id: Uuid::new_v4(),
                reason: "no such agent".to_string(),
                permanent: true,
            }
            .is_permanent()
        );
        assert!(
            !NotifyError::EndpointResolution {
                agent_id: Uuid::new_v4(),
                reason: "no live or stored endpoint".to_string(),
                permanent: false,
            }
            .is_permanent()
        );
    }

    #[test]
    fn peer_and_transport_errors_are_never_permanent() {
        assert!(!NotifyError::PeerError("http 503: unavailable".to_string()).is_permanent());
    }
}

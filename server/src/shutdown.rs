//! Stopping the server without cutting requests off.
//!
//! The server runs as several replicas, and a rolling update or a scale-down stops one while the
//! others keep serving. On SIGTERM (or Ctrl-C) a replica:
//!
//! 1. starts answering `/ready` with 503, so the load balancer stops sending it requests;
//! 2. keeps serving for `SERVER_DRAIN_DELAY_SECS`, while that takes effect;
//! 3. stops accepting connections and signals its background workers to stop claiming work
//!    ([`ShutdownSignal::cancelled`]);
//! 4. gives requests already in flight `SERVER_SHUTDOWN_GRACE_SECS` to finish, then exits.
//!
//! The binary runs as PID 1 in its container, where the kernel ignores any signal the process has
//! no handler for: without this, SIGTERM did nothing and every stop was a SIGKILL at the end of the
//! pod's termination grace period.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tokio_util::sync::{CancellationToken, WaitForCancellationFuture};

/// Shared by the readiness route, the accept loop and the background workers.
#[derive(Clone, Default)]
pub struct ShutdownSignal {
    inner: Arc<Inner>,
}

#[derive(Default)]
struct Inner {
    draining: AtomicBool,
    stop: CancellationToken,
}

impl ShutdownSignal {
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether this replica has been asked to stop: `/ready` answers 503 from then on.
    pub fn is_draining(&self) -> bool {
        self.inner.draining.load(Ordering::Relaxed)
    }

    /// Resolves once the replica stops accepting work. Workers that claim jobs select on it and
    /// stop claiming; work already claimed finishes or is left for its lease to expire.
    pub fn cancelled(&self) -> WaitForCancellationFuture<'_> {
        self.inner.stop.cancelled()
    }

    pub fn is_cancelled(&self) -> bool {
        self.inner.stop.is_cancelled()
    }

    fn begin_drain(&self) {
        self.inner.draining.store(true, Ordering::Relaxed);
    }

    pub(crate) fn stop(&self) {
        self.inner.stop.cancel();
    }
}

/// Wait for SIGTERM or Ctrl-C, then drain: report not ready, wait `drain_delay`, then signal stop.
pub async fn drain_on_terminate(signal: ShutdownSignal, drain_delay: Duration) {
    terminate_requested().await;
    tracing::info!(
        drain_delay_secs = drain_delay.as_secs(),
        "shutdown requested; reporting not ready and draining"
    );
    signal.begin_drain();
    tokio::time::sleep(drain_delay).await;
    tracing::info!("drain delay over; no longer accepting connections");
    signal.stop();
}

async fn terminate_requested() {
    let ctrl_c = async {
        if let Err(e) = tokio::signal::ctrl_c().await {
            tracing::warn!(error = %e, "could not listen for Ctrl-C");
            std::future::pending::<()>().await;
        }
    };
    #[cfg(unix)]
    let term = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut sigterm) => {
                sigterm.recv().await;
            }
            Err(e) => {
                tracing::warn!(error = %e, "could not listen for SIGTERM");
                std::future::pending::<()>().await;
            }
        }
    };
    #[cfg(not(unix))]
    let term = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => {}
        _ = term => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn drain_then_stop() {
        let signal = ShutdownSignal::new();
        assert!(!signal.is_draining());
        assert!(!signal.is_cancelled());
        signal.begin_drain();
        assert!(signal.is_draining());
        assert!(!signal.is_cancelled());
        signal.stop();
        signal.cancelled().await;
        assert!(signal.clone().is_cancelled());
    }
}

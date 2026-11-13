//! Which provider attempts of one request failed, so each can be billed as the call it was.
//!
//! The fallback chain returns only the last error, and its deadline drops the attempt in
//! flight without a word; the caller owns an [`AttemptLog`] instead, which outlives both. A
//! failed attempt is one that ended its turn in the chain: a parameter-fix retry of the same
//! model is not recorded (the provider rejected the request as malformed, and the router
//! re-sent it at once), and a request refused before it was sent never reaches the log.

use tokio::time::Instant;

use super::ProviderError;
use crate::usage::{CallFailure, FailedAttempt};

/// The class a usage row records for a provider error, or `None` when the request was refused
/// at this edge and never sent ([`ProviderError::Rejected`]).
pub fn failure_of(error: &ProviderError) -> Option<CallFailure> {
    match error {
        ProviderError::Status { status, .. } => Some(CallFailure::new("status", Some(*status))),
        ProviderError::Transport(_) => Some(CallFailure::new("transport", None)),
        ProviderError::Parse(_) => Some(CallFailure::new("parse", None)),
        ProviderError::TimedOut(_) => Some(CallFailure::new("timeout", None)),
        ProviderError::Rejected(_) => None,
    }
}

/// The class for an error that ended a stream after it began: the provider took the request,
/// so the call counts whatever the error was.
pub fn mid_stream_failure(error: &ProviderError) -> CallFailure {
    let status = match error {
        ProviderError::Status { status, .. } => Some(*status),
        _ => None,
    };
    CallFailure::new("stream", status)
}

/// The attempt currently waiting on its provider.
struct InFlight {
    provider: String,
    model: String,
    platform_paid: bool,
    started: Instant,
}

/// The failed attempts of one request, filled in by the chain as it runs.
#[derive(Default)]
pub struct AttemptLog {
    failed: Vec<FailedAttempt>,
    in_flight: Option<InFlight>,
}

impl AttemptLog {
    /// An attempt is about to be sent to `provider`/`model`.
    pub fn start(&mut self, provider: &str, model: &str, platform_paid: bool) {
        self.in_flight = Some(InFlight {
            provider: provider.to_string(),
            model: model.to_string(),
            platform_paid,
            started: Instant::now(),
        });
    }

    /// The attempt in flight will be retried at once (a parameter fix) or succeeded.
    pub fn settle(&mut self) {
        self.in_flight = None;
    }

    /// The attempt in flight failed with `error`.
    pub fn fail(&mut self, error: &ProviderError) {
        match failure_of(error) {
            Some(failure) => self.record(failure),
            None => self.settle(),
        }
    }

    fn record(&mut self, failure: CallFailure) {
        if let Some(attempt) = self.in_flight.take() {
            self.failed.push(FailedAttempt {
                provider: attempt.provider,
                model: attempt.model,
                platform_paid: attempt.platform_paid,
                latency_ms: attempt.started.elapsed().as_millis() as i64,
                failure,
            });
        }
    }

    /// Every failed attempt, in order. An attempt still in flight was cancelled by the chain's
    /// deadline while the provider held the request, so it is a failed call too.
    pub fn take_failed(&mut self) -> Vec<FailedAttempt> {
        self.record(CallFailure::new("timeout", None));
        std::mem::take(&mut self.failed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(code: u16) -> ProviderError {
        ProviderError::Status {
            status: code,
            message: "upstream".into(),
            retryable: true,
        }
    }

    #[test]
    fn a_failed_attempt_is_recorded_with_its_class_and_status() {
        let mut log = AttemptLog::default();
        log.start("openai", "gpt-4o", true);
        log.fail(&status(503));
        let failed = log.take_failed();
        assert_eq!(failed.len(), 1);
        assert_eq!(
            (failed[0].provider.as_str(), failed[0].model.as_str()),
            ("openai", "gpt-4o")
        );
        assert_eq!(failed[0].failure, CallFailure::new("status", Some(503)));
        assert!(failed[0].platform_paid);
    }

    #[test]
    fn a_success_and_a_parameter_fix_retry_record_nothing() {
        let mut log = AttemptLog::default();
        log.start("openai", "o3", false);
        log.settle();
        log.start("openai", "o3", false);
        log.settle();
        assert!(log.take_failed().is_empty());
    }

    #[test]
    fn a_request_refused_before_sending_is_not_a_call() {
        let mut log = AttemptLog::default();
        log.start("anthropic", "claude", true);
        log.fail(&ProviderError::Rejected("audio part".into()));
        assert!(log.take_failed().is_empty());
    }

    #[test]
    fn an_attempt_cut_off_by_the_deadline_is_a_timed_out_call() {
        let mut log = AttemptLog::default();
        log.start("openai", "gpt-4o", true);
        log.fail(&status(429));
        log.start("anthropic", "claude", true);
        let failed = log.take_failed();
        assert_eq!(
            failed.iter().map(|f| f.failure).collect::<Vec<_>>(),
            vec![
                CallFailure::new("status", Some(429)),
                CallFailure::new("timeout", None)
            ]
        );
        assert_eq!(failed[1].model, "claude");
    }

    #[test]
    fn a_mid_stream_error_keeps_the_status_when_there_is_one() {
        assert_eq!(
            mid_stream_failure(&status(500)),
            CallFailure::new("stream", Some(500))
        );
        assert_eq!(
            mid_stream_failure(&ProviderError::Transport("reset".into())),
            CallFailure::new("stream", None)
        );
    }
}

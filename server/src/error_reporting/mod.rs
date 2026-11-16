//! Error and crash reporting to Sentry.
//!
//! **Off unless configured.** Nothing is sent anywhere unless `SENTRY_DSN` (this server's own
//! project) or `SENTRY_BROWSER_DSN` (the UI's project) is set, so a self-hosted install reports
//! nothing until its operator points it at a Sentry of their own. Configuration is read at
//! runtime, never baked into the UI bundle, because one bundle ships to every deployment.
//!
//! Four pieces, wired at the composition roots:
//! - [`init`]: this server's client, called first thing in `main`. Captures panics, `error!`
//!   events (with `warn!` lines as breadcrumbs), a sample of requests as transactions, and
//!   `SENTRY_LOGS_LEVEL`'s lines (default warnings and errors) as Sentry Logs. Every event names
//!   the workspace (`TENANT_ID`) and, inside a request, the signed-in user's id ([`identify`]):
//!   an id, never a name or an email.
//! - [`tracing_layer`]: joins `tracing` to that client; `telemetry::init_telemetry` installs it
//!   beside the OTLP layer, which keeps working unchanged.
//! - [`trace_requests`]: middleware recording a request as a transaction named by its route
//!   template, continuing the browser's trace (`sentry-trace`/`baggage` headers).
//! - [`router`]: `GET /api/telemetry/config`, what the UI needs to start its own SDK, and
//!   `POST /api/telemetry/sentry`, the tunnel that SDK posts to. Through the tunnel the page's
//!   CSP stays `connect-src 'self'`, ad blockers don't drop crash reports, and only envelopes
//!   for this deployment's own browser project are forwarded.
//!
//! What leaves the server is scrubbed first ([`scrub`]): request bodies and cookies are never
//! sent, headers are an allow-list, and URLs go through the same credential redaction as the
//! server's own request spans (`mcp::redact_credential_uri`).

mod config;
mod requests;
mod scrub;
mod tunnel;

use std::sync::OnceLock;

pub use config::ErrorReportingConfig;
pub use nasiko_utils::logs_level::LogsLevel;
pub use requests::trace_requests;
pub use tunnel::router;

static CONFIG: OnceLock<ErrorReportingConfig> = OnceLock::new();

/// Start this server's Sentry client, if `SENTRY_DSN` is set, and remember `config` for the
/// UI's endpoints. Hold the returned guard for the life of `main`: dropping it flushes what is
/// still queued and stops reporting.
///
/// `edition` (`oss`, `ee`) becomes a tag on every event, so one project can hold both.
pub fn init(
    config: ErrorReportingConfig,
    edition: &'static str,
) -> Option<sentry::ClientInitGuard> {
    let config = CONFIG.get_or_init(|| config);
    let dsn = config.server_dsn.clone()?;
    let mut options = sentry::ClientOptions::new()
        .release(config.server_release.clone())
        .environment(config.environment.clone())
        .traces_sample_rate(config.server_traces_sample_rate)
        .send_default_pii(false)
        .attach_stacktrace(true)
        .before_send(scrub::event);
    options.before_send_log = Some(std::sync::Arc::new(scrub::log));
    options.dsn = Some(dsn);
    let guard = sentry::init(options);
    let workspace = config.workspace.clone();
    sentry::configure_scope(|scope| {
        scope.set_tag("edition", edition);
        if let Some(workspace) = workspace {
            scope.set_tag("workspace", workspace);
        }
    });
    Some(guard)
}

/// Name the signed-in user on what the current request reports, by account id only. Called by
/// the auth middleware once a caller is known; [`trace_requests`] gives every routed request a
/// scope of its own, so the id never outlives the request or reaches another one. A no-op when
/// nothing is reported.
pub fn identify(user_id: &str) {
    let id = user_id.to_owned();
    sentry::configure_scope(|scope| {
        scope.set_user(Some(sentry::User {
            id: Some(id),
            ..Default::default()
        }))
    });
}

/// What [`init`] was given; nothing is reported before it runs (and in tests that never call it).
pub fn current_config() -> &'static ErrorReportingConfig {
    CONFIG.get_or_init(ErrorReportingConfig::disabled)
}

/// `error!` becomes an event, `warn!` a breadcrumb on the next event, and spans join the
/// request's transaction. Lines at or above `SENTRY_LOGS_LEVEL` are also sent as logs, through
/// [`scrub::log`]. `debug!` and `trace!` never leave. `None` (a no-op layer) unless this server
/// reports to Sentry.
pub fn tracing_layer<S>() -> Option<sentry::integrations::tracing::SentryLayer<S>>
where
    S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
{
    let config = current_config();
    config.server_dsn.as_ref()?;
    let logs = config.logs;
    Some(
        sentry::integrations::tracing::layer()
            .event_filter(move |metadata| event_filter(logs, metadata.level())),
    )
}

/// What one `tracing` line becomes in Sentry.
fn event_filter(
    logs: LogsLevel,
    level: &tracing::Level,
) -> sentry::integrations::tracing::EventFilter {
    use sentry::integrations::tracing::EventFilter;
    let base = match *level {
        tracing::Level::ERROR => EventFilter::Event,
        tracing::Level::WARN => EventFilter::Breadcrumb,
        _ => EventFilter::Ignore,
    };
    if logs.includes(level) {
        base | EventFilter::Log
    } else {
        base
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sentry::integrations::tracing::EventFilter;

    #[test]
    fn errors_are_events_warnings_breadcrumbs_and_logs_follow_the_level() {
        let bits = |logs, level| event_filter(logs, level).bits();
        let warn = LogsLevel::Warn;
        assert_eq!(
            bits(warn, &tracing::Level::ERROR),
            (EventFilter::Event | EventFilter::Log).bits()
        );
        assert_eq!(
            bits(warn, &tracing::Level::WARN),
            (EventFilter::Breadcrumb | EventFilter::Log).bits()
        );
        assert_eq!(
            bits(warn, &tracing::Level::INFO),
            EventFilter::Ignore.bits()
        );
        assert_eq!(
            bits(LogsLevel::Info, &tracing::Level::INFO),
            EventFilter::Log.bits()
        );
        assert_eq!(
            bits(LogsLevel::Off, &tracing::Level::ERROR),
            EventFilter::Event.bits()
        );
        assert_eq!(
            bits(LogsLevel::Info, &tracing::Level::DEBUG),
            EventFilter::Ignore.bits()
        );
    }
}

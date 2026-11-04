//! Error and crash reporting to Sentry.
//!
//! **Off unless configured.** Nothing is sent anywhere unless `SENTRY_DSN` (this server's own
//! project) or `SENTRY_BROWSER_DSN` (the UI's project) is set, so a self-hosted install reports
//! nothing until its operator points it at a Sentry of their own. Configuration is read at
//! runtime, never baked into the UI bundle, because one bundle ships to every deployment.
//!
//! Four pieces, wired at the composition roots:
//! - [`init`]: this server's client, called first thing in `main`. Captures panics, `error!`
//!   events (with `warn!` lines as breadcrumbs), and a sample of requests as transactions.
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
    options.dsn = Some(dsn);
    let guard = sentry::init(options);
    sentry::configure_scope(|scope| scope.set_tag("edition", edition));
    Some(guard)
}

/// What [`init`] was given; nothing is reported before it runs (and in tests that never call it).
pub fn current_config() -> &'static ErrorReportingConfig {
    CONFIG.get_or_init(ErrorReportingConfig::disabled)
}

/// `error!` becomes an event, `warn!` a breadcrumb on the next event, and spans join the
/// request's transaction. `info!` and below are left out: they are high-volume and carry
/// request detail that has not been reviewed for what it may contain. `None` (a no-op layer)
/// unless this server reports to Sentry.
pub fn tracing_layer<S>() -> Option<sentry::integrations::tracing::SentryLayer<S>>
where
    S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
{
    use sentry::integrations::tracing::EventFilter;
    current_config().server_dsn.as_ref()?;
    Some(
        sentry::integrations::tracing::layer().event_filter(|metadata| match *metadata.level() {
            tracing::Level::ERROR => EventFilter::Event,
            tracing::Level::WARN => EventFilter::Breadcrumb,
            _ => EventFilter::Ignore,
        }),
    )
}

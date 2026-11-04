use std::str::FromStr;

use sentry::types::Dsn;

/// Where errors go and how much is sampled, read once at startup.
///
/// Two Sentry projects, not one: the server's events and the UI's have different platforms,
/// stack-trace formats and owners, and Sentry groups and alerts per project. Either can be set
/// without the other.
#[derive(Clone, Debug, PartialEq)]
pub struct ErrorReportingConfig {
    /// `SENTRY_DSN`: this server's own panics and `error!` events.
    pub server_dsn: Option<Dsn>,
    /// `SENTRY_BROWSER_DSN`: the UI's errors, which reach Sentry through this server's tunnel.
    pub browser_dsn: Option<Dsn>,
    /// `SENTRY_ENVIRONMENT`, e.g. `production`, `staging`, `customer-acme`.
    pub environment: String,
    /// `SENTRY_RELEASE`; defaults to `nasiko-server@<crate version>`.
    pub server_release: String,
    /// `SENTRY_TRACES_SAMPLE_RATE`: share of server requests recorded as transactions.
    pub server_traces_sample_rate: f32,
    /// `SENTRY_BROWSER_TRACES_SAMPLE_RATE`: share of page loads and navigations traced.
    pub browser_traces_sample_rate: f32,
    /// `SENTRY_REPLAY_SESSION_SAMPLE_RATE`: share of all sessions recorded from the start.
    pub replay_session_sample_rate: f32,
    /// `SENTRY_REPLAY_ON_ERROR_SAMPLE_RATE`: share of sessions with an error whose last
    /// minute is kept.
    pub replay_on_error_sample_rate: f32,
}

const DEFAULT_ENVIRONMENT: &str = "production";
const DEFAULT_TRACES_SAMPLE_RATE: f32 = 0.1;
const DEFAULT_REPLAY_SESSION_SAMPLE_RATE: f32 = 0.0;
const DEFAULT_REPLAY_ON_ERROR_SAMPLE_RATE: f32 = 1.0;

impl ErrorReportingConfig {
    pub fn from_env() -> Self {
        Self::from_lookup(|name| std::env::var(name).ok())
    }

    /// Nothing is reported: the state of a server with no `SENTRY_*` set.
    pub fn disabled() -> Self {
        Self::from_lookup(|_| None)
    }

    /// Tracing is not installed yet when this runs, so a bad value is reported on stderr. A bad
    /// DSN turns that project off rather than stopping the boot: crash reporting is never a
    /// reason for the control plane not to start.
    pub(crate) fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Self {
        let set = |name: &str| get(name).filter(|value| !value.trim().is_empty());
        let rate = |name: &str, default: f32| match set(name) {
            None => default,
            Some(raw) => parse_rate(&raw).unwrap_or_else(|| {
                eprintln!("{name} must be a number from 0 to 1, got {raw:?}; using {default}");
                default
            }),
        };
        let dsn = |name: &str| {
            let raw = set(name)?;
            Dsn::from_str(raw.trim())
                .inspect_err(|e| {
                    eprintln!(
                        "{name} is not a valid Sentry DSN ({e}); reporting it to Sentry is off"
                    )
                })
                .ok()
        };
        Self {
            server_dsn: dsn("SENTRY_DSN"),
            browser_dsn: dsn("SENTRY_BROWSER_DSN"),
            environment: set("SENTRY_ENVIRONMENT")
                .map(|e| e.trim().to_owned())
                .unwrap_or_else(|| DEFAULT_ENVIRONMENT.to_owned()),
            server_release: set("SENTRY_RELEASE")
                .map(|r| r.trim().to_owned())
                .unwrap_or_else(|| concat!("nasiko-server@", env!("CARGO_PKG_VERSION")).to_owned()),
            server_traces_sample_rate: rate(
                "SENTRY_TRACES_SAMPLE_RATE",
                DEFAULT_TRACES_SAMPLE_RATE,
            ),
            browser_traces_sample_rate: rate(
                "SENTRY_BROWSER_TRACES_SAMPLE_RATE",
                DEFAULT_TRACES_SAMPLE_RATE,
            ),
            replay_session_sample_rate: rate(
                "SENTRY_REPLAY_SESSION_SAMPLE_RATE",
                DEFAULT_REPLAY_SESSION_SAMPLE_RATE,
            ),
            replay_on_error_sample_rate: rate(
                "SENTRY_REPLAY_ON_ERROR_SAMPLE_RATE",
                DEFAULT_REPLAY_ON_ERROR_SAMPLE_RATE,
            ),
        }
    }
}

fn parse_rate(raw: &str) -> Option<f32> {
    raw.trim()
        .parse::<f32>()
        .ok()
        .filter(|rate| (0.0..=1.0).contains(rate))
}

#[cfg(test)]
mod tests {
    use super::*;

    const DSN: &str = "https://abc123@o1.ingest.de.sentry.io/42";

    fn config(vars: &[(&str, &str)]) -> ErrorReportingConfig {
        ErrorReportingConfig::from_lookup(|name| {
            vars.iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| (*value).to_owned())
        })
    }

    #[test]
    fn nothing_set_reports_nothing() {
        let c = ErrorReportingConfig::disabled();
        assert_eq!(c.server_dsn, None);
        assert_eq!(c.browser_dsn, None);
        assert_eq!(c.environment, "production");
        assert!(c.server_release.starts_with("nasiko-server@"));
    }

    #[test]
    fn each_project_is_enabled_on_its_own() {
        let c = config(&[("SENTRY_BROWSER_DSN", DSN)]);
        assert_eq!(c.server_dsn, None);
        assert_eq!(c.browser_dsn.unwrap().project_id().value(), "42");
    }

    #[test]
    fn a_blank_or_invalid_dsn_is_off_not_fatal() {
        let c = config(&[("SENTRY_DSN", "  "), ("SENTRY_BROWSER_DSN", "not a dsn")]);
        assert_eq!(c.server_dsn, None);
        assert_eq!(c.browser_dsn, None);
    }

    #[test]
    fn rates_outside_zero_to_one_fall_back_to_the_default() {
        let c = config(&[
            ("SENTRY_TRACES_SAMPLE_RATE", "1.5"),
            ("SENTRY_BROWSER_TRACES_SAMPLE_RATE", "lots"),
            ("SENTRY_REPLAY_SESSION_SAMPLE_RATE", " 0.25 "),
        ]);
        assert_eq!(c.server_traces_sample_rate, DEFAULT_TRACES_SAMPLE_RATE);
        assert_eq!(c.browser_traces_sample_rate, DEFAULT_TRACES_SAMPLE_RATE);
        assert_eq!(c.replay_session_sample_rate, 0.25);
        assert_eq!(
            c.replay_on_error_sample_rate,
            DEFAULT_REPLAY_ON_ERROR_SAMPLE_RATE
        );
    }
}

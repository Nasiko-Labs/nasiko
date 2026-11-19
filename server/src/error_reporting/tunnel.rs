//! The UI's two endpoints: its SDK settings, and the tunnel its SDK posts envelopes through.
//!
//! Both are public: the sign-in page can crash too, before there is a session. The tunnel is
//! therefore not an open relay. It forwards only envelopes addressed to this deployment's own
//! browser project, only to that project's ingest URL, under a size cap and a global rate.

use std::sync::Arc;
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use sentry::types::Dsn;
use serde_json::json;

use super::ErrorReportingConfig;
use crate::rate_limit::{RateLimitStore, RateLimiter};

/// A replay segment is the largest envelope the browser SDK sends; it compresses a few
/// seconds of DOM changes and stays well under this.
const MAX_ENVELOPE_BYTES: usize = 2 * 1024 * 1024;
/// The envelope header (one JSON line naming the DSN) comes first and is small.
const MAX_HEADER_BYTES: usize = 8 * 1024;
/// One bucket for the whole deployment: there is no caller identity before sign-in, and the
/// process does not see real client addresses (see `rate_limit::limit_globally`).
const ENVELOPES_PER_MINUTE: u32 = 1200;
const UPSTREAM_TIMEOUT: Duration = Duration::from_secs(10);
/// What Sentry answers with that the SDK acts on: its backoff instructions.
const RELAYED_HEADERS: &[&str] = &["retry-after", "x-sentry-rate-limits"];

struct Tunnel {
    browser: Option<BrowserSettings>,
    http: reqwest::Client,
    limiter: RateLimiter,
}

struct BrowserSettings {
    dsn: Dsn,
    body: serde_json::Value,
}

/// `GET /api/telemetry/config` and `POST /api/telemetry/sentry`, both answering "off" unless
/// `SENTRY_BROWSER_DSN` is set.
pub fn router<S: Clone + Send + Sync + 'static>(
    config: &ErrorReportingConfig,
    http: reqwest::Client,
    rate_limits: &RateLimitStore,
) -> Router<S> {
    let browser = config.browser_dsn.clone().map(|dsn| BrowserSettings {
        body: json!({ "data": {
            "enabled": true,
            "dsn": public_dsn(&dsn),
            "environment": config.environment,
            "traces_sample_rate": config.browser_traces_sample_rate,
            "replays_session_sample_rate": config.replay_session_sample_rate,
            "replays_on_error_sample_rate": config.replay_on_error_sample_rate,
            "tunnel": "/api/telemetry/sentry",
            "workspace": config.workspace,
            "logs": config.logs != super::LogsLevel::Off,
        }}),
        dsn,
    });
    let tunnel = Arc::new(Tunnel {
        browser,
        http,
        limiter: RateLimiter::new(
            rate_limits,
            "sentry-tunnel",
            ENVELOPES_PER_MINUTE,
            Duration::from_secs(60),
        ),
    });
    Router::new()
        .route("/api/telemetry/config", get(browser_config))
        .route(
            "/api/telemetry/sentry",
            post(forward_envelope).layer(DefaultBodyLimit::max(MAX_ENVELOPE_BYTES)),
        )
        .with_state(tunnel)
}

/// The DSN as the operator wrote it. `Dsn`'s `Display` keeps an empty secret as `key:@host`;
/// browser SDKs parse that, but it reads like a credential was cut off.
fn public_dsn(dsn: &Dsn) -> String {
    dsn.to_string().replacen(":@", "@", 1)
}

async fn browser_config(State(tunnel): State<Arc<Tunnel>>) -> Response {
    let body = match &tunnel.browser {
        Some(browser) => browser.body.clone(),
        None => json!({ "data": { "enabled": false } }),
    };
    // Short-lived: an operator who sets or clears the DSN sees it take effect on the next load.
    ([(header::CACHE_CONTROL, "no-cache")], Json(body)).into_response()
}

async fn forward_envelope(State(tunnel): State<Arc<Tunnel>>, body: Bytes) -> Response {
    let Some(browser) = &tunnel.browser else {
        return error(
            StatusCode::NOT_FOUND,
            "error_reporting_off",
            "error reporting is off",
        );
    };
    if let Err(reason) = check_addressed_to(&body, &browser.dsn) {
        return error(StatusCode::BAD_REQUEST, "envelope_rejected", reason);
    }
    if let Err(wait) = tunnel.limiter.check("global").await {
        let mut response = error(
            StatusCode::TOO_MANY_REQUESTS,
            "rate_limited",
            "too many reports",
        );
        if let Ok(secs) = HeaderValue::from_str(&wait.as_secs().max(1).to_string()) {
            response.headers_mut().insert(header::RETRY_AFTER, secs);
        }
        return response;
    }
    let upstream = tunnel
        .http
        .post(browser.dsn.envelope_api_url().as_str())
        .header(header::CONTENT_TYPE, "application/x-sentry-envelope")
        .timeout(UPSTREAM_TIMEOUT)
        .body(body)
        .send()
        .await;
    match upstream {
        Ok(reply) => relay(reply),
        Err(e) => {
            // `debug`, not `warn`: a Sentry outage must not turn into reports about itself.
            tracing::debug!(error = %e, "error-report tunnel: Sentry unreachable");
            error(
                StatusCode::BAD_GATEWAY,
                "upstream_unreachable",
                "Sentry is unreachable",
            )
        }
    }
}

/// The envelope's first line is a JSON header; the browser SDK puts its DSN there when it posts
/// through a tunnel. Forward only when that DSN is this deployment's browser project.
fn check_addressed_to(envelope: &[u8], expected: &Dsn) -> Result<(), &'static str> {
    let header_end = envelope
        .iter()
        .take(MAX_HEADER_BYTES)
        .position(|b| *b == b'\n')
        .unwrap_or(envelope.len().min(MAX_HEADER_BYTES));
    let header: serde_json::Value =
        serde_json::from_slice(&envelope[..header_end]).map_err(|_| "not a Sentry envelope")?;
    let dsn: Dsn = header
        .get("dsn")
        .and_then(|d| d.as_str())
        .ok_or("the envelope names no DSN")?
        .parse()
        .map_err(|_| "the envelope's DSN is invalid")?;
    let same_project = dsn.host() == expected.host()
        && dsn.project_id() == expected.project_id()
        && dsn.public_key() == expected.public_key();
    if same_project {
        Ok(())
    } else {
        Err("the envelope is for another project")
    }
}

fn relay(reply: reqwest::Response) -> Response {
    let status = StatusCode::from_u16(reply.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let mut headers = HeaderMap::new();
    for name in RELAYED_HEADERS {
        if let Some(value) = reply
            .headers()
            .get(*name)
            .and_then(|v| HeaderValue::from_bytes(v.as_bytes()).ok())
        {
            headers.insert(*name, value);
        }
    }
    // The body (an event id) is of no use to the SDK; the status and backoff headers are.
    (status, headers).into_response()
}

fn error(status: StatusCode, code: &str, message: &str) -> Response {
    (status, Json(json!({ "error": message, "code": code }))).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use tower::ServiceExt;

    const DSN: &str = "https://public123@o9.ingest.de.sentry.io/4242";

    fn envelope(dsn: &str) -> String {
        format!(
            "{{\"event_id\":\"9ec79c33ec9942ab8353589fcb2e04dc\",\"dsn\":\"{dsn}\"}}\n{{\"type\":\"event\"}}\n{{}}\n"
        )
    }

    fn app(browser_dsn: Option<&str>) -> Router {
        let config = ErrorReportingConfig {
            browser_dsn: browser_dsn.map(|d| d.parse().unwrap()),
            ..ErrorReportingConfig::disabled()
        };
        router(&config, reqwest::Client::new(), &RateLimitStore::Local)
    }

    async fn call(
        app: Router,
        method: &str,
        uri: &str,
        body: String,
    ) -> (StatusCode, serde_json::Value) {
        let response = app
            .oneshot(
                axum::http::Request::builder()
                    .method(method)
                    .uri(uri)
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
        )
    }

    #[tokio::test]
    async fn config_says_off_without_a_browser_dsn() {
        let (status, body) = call(app(None), "GET", "/api/telemetry/config", String::new()).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, json!({ "data": { "enabled": false } }));
    }

    #[tokio::test]
    async fn config_hands_the_ui_its_dsn_and_the_tunnel() {
        let (_, body) = call(
            app(Some(DSN)),
            "GET",
            "/api/telemetry/config",
            String::new(),
        )
        .await;
        assert_eq!(body["data"]["enabled"], true);
        assert_eq!(body["data"]["dsn"], DSN);
        assert_eq!(body["data"]["tunnel"], "/api/telemetry/sentry");
        assert_eq!(body["data"]["logs"], true);
        assert!(body["data"]["workspace"].is_null());
    }

    #[tokio::test]
    async fn config_names_the_workspace_and_whether_logs_are_on() {
        let config = ErrorReportingConfig {
            browser_dsn: Some(DSN.parse().unwrap()),
            workspace: Some("t-123".into()),
            logs: super::super::LogsLevel::Off,
            ..ErrorReportingConfig::disabled()
        };
        let app = router(&config, reqwest::Client::new(), &RateLimitStore::Local);
        let (_, body) = call(app, "GET", "/api/telemetry/config", String::new()).await;
        assert_eq!(body["data"]["workspace"], "t-123");
        assert_eq!(body["data"]["logs"], false);
    }

    #[tokio::test]
    async fn the_tunnel_is_closed_when_reporting_is_off() {
        let (status, body) = call(app(None), "POST", "/api/telemetry/sentry", envelope(DSN)).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body["code"], "error_reporting_off");
    }

    #[tokio::test]
    async fn the_tunnel_refuses_envelopes_for_another_project() {
        for other in [
            "https://public123@o9.ingest.de.sentry.io/1",
            "https://someoneelse@o9.ingest.de.sentry.io/4242",
            "https://public123@attacker.example/4242",
        ] {
            let (status, body) = call(
                app(Some(DSN)),
                "POST",
                "/api/telemetry/sentry",
                envelope(other),
            )
            .await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{other}");
            assert_eq!(body["code"], "envelope_rejected");
        }
    }

    #[test]
    fn an_envelope_must_name_its_dsn_in_a_json_header() {
        let expected: Dsn = DSN.parse().unwrap();
        assert!(check_addressed_to(envelope(DSN).as_bytes(), &expected).is_ok());
        assert!(check_addressed_to(b"{\"event_id\":\"x\"}\n{}", &expected).is_err());
        assert!(check_addressed_to(b"not json\n{}", &expected).is_err());
        assert!(check_addressed_to(b"", &expected).is_err());
    }

    #[tokio::test]
    async fn an_oversized_envelope_is_refused_before_it_is_read() {
        let mut body = envelope(DSN);
        body.push_str(&"x".repeat(MAX_ENVELOPE_BYTES));
        let (status, _) = call(app(Some(DSN)), "POST", "/api/telemetry/sentry", body).await;
        assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    }

    #[tokio::test]
    async fn an_envelope_for_this_project_reaches_sentry_and_its_backoff_comes_back() {
        let mut sentry = mockito::Server::new_async().await;
        let dsn = format!("http://public123@{}/4242", sentry.host_with_port());
        let ingest = sentry
            .mock("POST", "/api/4242/envelope/")
            .match_header("content-type", "application/x-sentry-envelope")
            .match_body(mockito::Matcher::Exact(envelope(&dsn)))
            .with_status(429)
            .with_header("retry-after", "60")
            .with_header("x-sentry-rate-limits", "60:error:organization")
            .create_async()
            .await;
        let response = app(Some(&dsn))
            .oneshot(
                axum::http::Request::post("/api/telemetry/sentry")
                    .body(Body::from(envelope(&dsn)))
                    .unwrap(),
            )
            .await
            .unwrap();
        ingest.assert_async().await;
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(response.headers()["retry-after"], "60");
        assert_eq!(
            response.headers()["x-sentry-rate-limits"],
            "60:error:organization"
        );
    }
}

//! What a request may tell Sentry. An allow-list, not a deny-list: a header or field added
//! later stays out until someone decides it is safe to send.

use sentry::protocol::{Event, Map, Request};

use crate::mcp::redact_credential_uri;

/// Headers that help debug a failure and carry no credential or personal data.
const SAFE_HEADERS: &[&str] = &[
    "accept",
    "content-length",
    "content-type",
    "user-agent",
    "x-request-id",
];

/// `before_send` for every event this server reports.
pub(super) fn event(mut event: Event<'static>) -> Option<Event<'static>> {
    event.request = event.request.take().map(request);
    Some(event)
}

/// The request as Sentry sees it, from the incoming HTTP request.
pub(super) fn from_http<B>(req: &axum::http::Request<B>) -> Request {
    let headers = req
        .headers()
        .iter()
        .filter_map(|(name, value)| {
            Some((name.as_str().to_owned(), value.to_str().ok()?.to_owned()))
        })
        .collect();
    let host = req
        .headers()
        .get(axum::http::header::HOST)
        .and_then(|h| h.to_str().ok());
    request(Request {
        url: host.and_then(|h| format!("http://{h}{}", req.uri()).parse().ok()),
        method: Some(req.method().to_string()),
        headers,
        ..Default::default()
    })
}

/// Drop bodies, cookies and environment; keep the allow-listed headers; redact credentials in
/// the URL path and query exactly as the server's own request spans do.
pub(super) fn request(request: Request) -> Request {
    let url = request.url.and_then(|mut url| {
        let path_and_query = match url.query() {
            Some(query) => format!("{}?{query}", url.path()),
            None => url.path().to_owned(),
        };
        let uri: axum::http::Uri = path_and_query.parse().ok()?;
        let redacted = redact_credential_uri(&uri);
        let (path, query) = match redacted.split_once('?') {
            Some((path, query)) => (path.to_owned(), Some(query.to_owned())),
            None => (redacted, None),
        };
        url.set_path(&path);
        url.set_query(query.as_deref());
        url.set_fragment(None);
        Some(url)
    });
    let headers: Map<String, String> = request
        .headers
        .into_iter()
        .filter(|(name, _)| SAFE_HEADERS.contains(&name.to_ascii_lowercase().as_str()))
        .collect();
    Request {
        query_string: url.as_ref().and_then(|u| u.query().map(str::to_owned)),
        url,
        method: request.method,
        headers,
        data: None,
        cookies: None,
        env: Map::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn http(uri: &str) -> axum::http::Request<()> {
        axum::http::Request::builder()
            .uri(uri)
            .header("host", "cp.example.com")
            .header("authorization", "Bearer secret-jwt")
            .header("cookie", "nasiko_session=secret")
            .header("user-agent", "Mozilla/5.0")
            .body(())
            .unwrap()
    }

    #[test]
    fn credentials_in_the_path_never_leave() {
        let r = from_http(&http("/api/mcp/s/agent-credential-123"));
        let url = r.url.unwrap().to_string();
        assert!(!url.contains("agent-credential-123"), "{url}");
        assert!(url.ends_with("/api/mcp/s/%7Btoken%7D"), "{url}");
    }

    #[test]
    fn sensitive_query_values_are_redacted_and_others_kept() {
        let r = from_http(&http("/api/auth/sso/session?token=jwt.value&page=2"));
        assert_eq!(r.query_string.as_deref(), Some("token=[REDACTED]&page=2"));
        assert!(!r.url.unwrap().as_str().contains("jwt.value"));
    }

    #[test]
    fn only_allow_listed_headers_are_kept() {
        let r = from_http(&http("/api/agents"));
        assert_eq!(
            r.headers.get("user-agent").map(String::as_str),
            Some("Mozilla/5.0")
        );
        assert!(!r.headers.contains_key("authorization"));
        assert!(!r.headers.contains_key("cookie"));
        assert!(!r.headers.contains_key("host"));
    }

    #[test]
    fn an_event_loses_bodies_cookies_and_environment() {
        let mut env = Map::new();
        env.insert("REMOTE_ADDR".into(), "10.0.0.1".into());
        let event = Event {
            request: Some(Request {
                url: "https://cp.example.com/api/x?code=oauth-code".parse().ok(),
                data: Some(r#"{"password":"hunter2"}"#.into()),
                cookies: Some("nasiko_session=secret".into()),
                env,
                ..Default::default()
            }),
            ..Default::default()
        };
        let request = super::event(event).unwrap().request.unwrap();
        assert_eq!(request.data, None);
        assert_eq!(request.cookies, None);
        assert!(request.env.is_empty());
        assert_eq!(request.query_string.as_deref(), Some("code=[REDACTED]"));
    }
}

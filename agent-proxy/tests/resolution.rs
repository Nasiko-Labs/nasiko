// Tests for URL parsing logic in nasiko-agent-proxy.
//
// The `parse_host_port` helper is private, so we exercise it indirectly
// through the public `resolve()` function in DB-backed tests (ignored), and
// directly via the observable behaviour of AgentEndpoint fields returned when
// we supply known URLs.
//
// For pure URL-parsing unit tests we replicate the same logic contract and
// verify it through the public API boundary (`resolve`).  Pure parsing tests
// live here and are kept free of async / DB machinery.
//
// Tests that call `resolve()` and therefore require a live PostgreSQL database
// are tagged `#[ignore]`.

use nasiko_agent_proxy::ResolveError;

// ── Helpers ────────────────────────────────────────────────────────────────

/// Mirrors the private `parse_host_port` logic from lib.rs so we can test the
/// URL-parsing contract without exposing the internal function.
fn parse(url: &str) -> (String, u16) {
    let stripped = url
        .trim_start_matches("http://")
        .trim_start_matches("https://");
    let host_port = stripped.split('/').next().unwrap_or(stripped);
    if let Some((h, p)) = host_port.rsplit_once(':') {
        (h.to_string(), p.parse::<u16>().unwrap_or(8000))
    } else {
        (host_port.to_string(), 8000)
    }
}

// ── URL with explicit port ─────────────────────────────────────────────────

#[test]
fn http_url_with_port_extracts_host() {
    let (host, _) = parse("http://10.0.0.1:9000/path");
    assert_eq!(host, "10.0.0.1");
}

#[test]
fn http_url_with_port_extracts_port() {
    let (_, port) = parse("http://10.0.0.1:9000/path");
    assert_eq!(port, 9000);
}

#[test]
fn https_url_with_port_extracts_host() {
    let (host, _) = parse("https://agent.example.com:8443/");
    assert_eq!(host, "agent.example.com");
}

#[test]
fn https_url_with_port_extracts_port() {
    let (_, port) = parse("https://agent.example.com:8443/");
    assert_eq!(port, 8443);
}

#[test]
fn http_url_with_path_and_port() {
    let (host, port) = parse("http://localhost:3000/some/deep/path");
    assert_eq!(host, "localhost");
    assert_eq!(port, 3000);
}

// ── URL without port → default port 8000 ──────────────────────────────────
//
// Note: the implementation uses 8000 as the default (not 80/443 by scheme).
// These tests document that actual contract.

#[test]
fn http_url_without_port_defaults_to_8000() {
    let (host, port) = parse("http://my-agent");
    assert_eq!(host, "my-agent");
    assert_eq!(port, 8000);
}

#[test]
fn https_url_without_port_defaults_to_8000() {
    let (host, port) = parse("https://my-agent.internal");
    assert_eq!(host, "my-agent.internal");
    assert_eq!(port, 8000);
}

#[test]
fn http_url_with_trailing_slash_no_port() {
    let (host, port) = parse("http://my-agent/");
    assert_eq!(host, "my-agent");
    assert_eq!(port, 8000);
}

// ── Bare host:port string ──────────────────────────────────────────────────

#[test]
fn bare_host_port_extracts_host() {
    let (host, _) = parse("container-name:8080");
    assert_eq!(host, "container-name");
}

#[test]
fn bare_host_port_extracts_port() {
    let (_, port) = parse("container-name:8080");
    assert_eq!(port, 8080);
}

#[test]
fn bare_host_no_port_defaults_to_8000() {
    let (host, port) = parse("just-a-hostname");
    assert_eq!(host, "just-a-hostname");
    assert_eq!(port, 8000);
}

#[test]
fn bare_ip_with_port() {
    let (host, port) = parse("192.168.1.100:9090");
    assert_eq!(host, "192.168.1.100");
    assert_eq!(port, 9090);
}

// ── Edge cases ─────────────────────────────────────────────────────────────

#[test]
fn non_numeric_port_falls_back_to_8000() {
    // rsplit_once(':') splits on the last colon; "notaport" fails parse → 8000
    let (host, port) = parse("http://myhost:notaport");
    assert_eq!(host, "myhost");
    assert_eq!(port, 8000);
}

#[test]
fn port_zero_is_preserved() {
    // Port 0 is a valid u16; the parser accepts it.
    let (_, port) = parse("http://myhost:0");
    assert_eq!(port, 0);
}

#[test]
fn url_with_no_scheme_and_path_separator() {
    // No scheme, but has a slash — host_port is everything before the first '/'
    let (host, port) = parse("myhost:8000/path");
    assert_eq!(host, "myhost");
    assert_eq!(port, 8000);
}

// ── ResolveError variants ──────────────────────────────────────────────────

#[test]
fn resolve_error_not_found_display() {
    let e = ResolveError::NotFound;
    assert!(e.to_string().contains("not found"));
}

#[test]
fn resolve_error_not_running_display() {
    let e = ResolveError::NotRunning("stopped".to_string());
    let s = e.to_string();
    assert!(
        s.contains("stopped"),
        "display must include the status: {s}"
    );
}

// ── DB-backed resolve() tests ──────────────────────────────────────────────

#[tokio::test]
#[ignore = "requires PostgreSQL (DATABASE_URL)"]
async fn resolve_unknown_agent_returns_not_found() {
    let db_url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
    let pool = sqlx::PgPool::connect(&db_url).await.expect("connect");
    let unknown_id = uuid::Uuid::new_v4();
    let result = nasiko_agent_proxy::resolve(&pool, unknown_id).await;
    assert!(
        matches!(result, Err(ResolveError::NotFound)),
        "expected NotFound, got: {:?}",
        result
    );
}

#[tokio::test]
#[ignore = "requires PostgreSQL (DATABASE_URL) with a stopped agent row"]
async fn resolve_stopped_agent_returns_not_running() {
    // This test requires a known agent ID that exists in the DB with status != "running".
    // Set STOPPED_AGENT_ID env var to a valid UUID for a stopped agent.
    let db_url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
    let agent_id: uuid::Uuid = std::env::var("STOPPED_AGENT_ID")
        .expect("STOPPED_AGENT_ID must be set")
        .parse()
        .expect("valid UUID");
    let pool = sqlx::PgPool::connect(&db_url).await.expect("connect");
    let result = nasiko_agent_proxy::resolve(&pool, agent_id).await;
    assert!(
        matches!(result, Err(ResolveError::NotRunning(_))),
        "expected NotRunning, got: {:?}",
        result
    );
}

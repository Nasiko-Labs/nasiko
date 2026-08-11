use super::*;

// ─── lookup_existing: 404 → None, error → Err, success → Some ───────────────

#[test]
fn lookup_existing_returns_none_on_real_404() {
    let mut srv = mockito::Server::new();
    srv.mock("GET", "/api/agents/ghost")
        .with_status(404)
        .with_body("not found")
        .create();
    let client = Client::for_test(&srv.url(), None);

    let found = lookup_existing(&client, "ghost").unwrap();
    assert!(found.is_none());
}

#[test]
fn lookup_existing_propagates_error_instead_of_treating_as_new_agent() {
    // A network/auth failure must not be conflated with "doesn't exist yet" —
    // otherwise push would register a fresh agent row on top of one that
    // already exists.
    let mut srv = mockito::Server::new();
    srv.mock("GET", "/api/agents/flaky")
        .with_status(500)
        .with_body("boom")
        .create();
    let client = Client::for_test(&srv.url(), None);

    assert!(lookup_existing(&client, "flaky").is_err());
}

#[test]
fn lookup_existing_returns_id_and_version_on_success() {
    let mut srv = mockito::Server::new();
    srv.mock("GET", "/api/agents/live")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"{"id":"agent-1","version":"1.2.3"}"#)
        .create();
    let client = Client::for_test(&srv.url(), None);

    let (id, version) = lookup_existing(&client, "live").unwrap().unwrap();
    assert_eq!(id, "agent-1");
    assert_eq!(version, Some("1.2.3".to_string()));
}

// ─── used_version_context ────────────────────────────────────────────────────

#[test]
fn used_version_context_is_empty_for_a_brand_new_agent() {
    let srv = mockito::Server::new();
    let client = Client::for_test(&srv.url(), None);

    let (current, used) = used_version_context(&client, &None).unwrap();
    assert_eq!(current, None);
    assert!(used.is_empty());
}

#[test]
fn used_version_context_reports_current_version_and_history() {
    let mut srv = mockito::Server::new();
    srv.mock("GET", "/api/agents/agent-1/versions")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            r#"{"data": [
                {"version": "1.0.0", "status": "active", "is_active": true,
                 "can_rollback": false, "created_at": "2024-01-01T00:00:00Z"}
            ]}"#,
        )
        .create();
    let client = Client::for_test(&srv.url(), None);
    let existing = Some(("agent-1".to_string(), Some("1.0.0".to_string())));

    let (current, used) = used_version_context(&client, &existing).unwrap();
    assert_eq!(current, Some("1.0.0".to_string()));
    assert_eq!(used, vec!["1.0.0".to_string()]);
}

#[test]
fn used_version_context_propagates_history_fetch_failure() {
    // Failing open here would let `resolve_deploy_version` treat every
    // version as unused, defeating the duplicate-version guard entirely.
    let mut srv = mockito::Server::new();
    srv.mock("GET", "/api/agents/agent-1/versions")
        .with_status(500)
        .with_body("boom")
        .create();
    let client = Client::for_test(&srv.url(), None);
    let existing = Some(("agent-1".to_string(), Some("1.0.0".to_string())));

    assert!(used_version_context(&client, &existing).is_err());
}

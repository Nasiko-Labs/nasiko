use super::*;

// ─── used_version_context ────────────────────────────────────────────────────

#[test]
fn used_version_context_is_empty_for_a_brand_new_agent() {
    let srv = mockito::Server::new();
    let client = Client::for_test(&srv.url(), None);

    let (current, used) = used_version_context(&client, None).unwrap();
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
    let existing = (
        "agent-1".to_string(),
        serde_json::json!({"id": "agent-1", "version": "1.0.0"}),
    );

    let (current, used) = used_version_context(&client, Some(&existing)).unwrap();
    assert_eq!(current, Some("1.0.0"));
    assert_eq!(used, vec!["1.0.0".to_string()]);
}

#[test]
fn used_version_context_excludes_pushed_status_from_history() {
    // A version that was `push`ed but never deployed must not block deploying
    // it — only rows with a real deployed status count as "used".
    let mut srv = mockito::Server::new();
    srv.mock("GET", "/api/agents/agent-1/versions")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            r#"{"data": [
                {"version": "1.0.0", "status": "active", "is_active": true,
                 "can_rollback": false, "created_at": "2024-01-01T00:00:00Z"},
                {"version": "2.0.0", "status": "pushed", "is_active": false,
                 "can_rollback": false, "created_at": "2024-01-02T00:00:00Z"}
            ]}"#,
        )
        .create();
    let client = Client::for_test(&srv.url(), None);
    let existing = (
        "agent-1".to_string(),
        serde_json::json!({"id": "agent-1", "version": "1.0.0"}),
    );

    let (_, used) = used_version_context(&client, Some(&existing)).unwrap();
    assert_eq!(used, vec!["1.0.0".to_string()]);
}

#[test]
fn used_version_context_propagates_history_fetch_failure() {
    let mut srv = mockito::Server::new();
    srv.mock("GET", "/api/agents/agent-1/versions")
        .with_status(500)
        .with_body("boom")
        .create();
    let client = Client::for_test(&srv.url(), None);
    let existing = (
        "agent-1".to_string(),
        serde_json::json!({"id": "agent-1", "version": "1.0.0"}),
    );

    assert!(used_version_context(&client, Some(&existing)).is_err());
}

use super::*;

// ─── resolve_image_deploy_version ────────────────────────────────────────────

#[test]
fn redeploying_the_currently_deployed_tag_is_a_clean_conflict_not_a_silent_rebump() {
    let err = resolve_image_deploy_version(
        "legal-agent:1.0.1",
        "1.0.1",
        VersionFlags::default(),
        Some("1.0.1"),
        &["1.0.0".to_string(), "1.0.1".to_string()],
        "deploy",
    )
    .unwrap_err()
    .to_string();
    assert!(
        err.contains("legal-agent:1.0.1 already exists"),
        "error should name the exact artifact: {err}"
    );
    assert!(err.contains("immutable"));
    assert!(err.contains("Suggested next version: 1.0.2"));
    assert!(err.contains("nasiko build --version 1.0.2"));
    assert!(err.contains("nasiko deploy legal-agent:1.0.2"));
}

#[test]
fn explicit_image_tag_is_preserved_regardless_of_currently_deployed_version() {
    let decision = resolve_image_deploy_version(
        "legal-agent:1.0.1",
        "1.0.1",
        VersionFlags::default(),
        Some("1.0.0"),
        &["1.0.0".to_string()],
        "deploy",
    )
    .unwrap();
    assert_eq!(decision.version, "1.0.1");
}

#[test]
fn explicit_image_tag_reusing_history_is_rejected_with_no_overwrite_option() {
    let err = resolve_image_deploy_version(
        "legal-agent:1.0.0",
        "1.0.0",
        VersionFlags::default(),
        Some("1.0.0"),
        &["1.0.0".to_string()],
        "deploy",
    )
    .unwrap_err();
    assert!(err.to_string().contains("already exists"));
    assert!(err.to_string().contains("immutable"));
    assert!(!err.to_string().contains("overwrite"));
}

#[test]
fn push_conflict_message_uses_the_push_verb() {
    let err = resolve_image_deploy_version(
        "legal-agent:1.0.0",
        "1.0.0",
        VersionFlags::default(),
        Some("1.0.0"),
        &["1.0.0".to_string()],
        "push",
    )
    .unwrap_err()
    .to_string();
    assert!(err.contains("nasiko push legal-agent:1.0.1"));
}

#[test]
fn image_without_an_explicit_tag_still_falls_back_to_version_prompt_logic() {
    // No `:tag` -> falls back to the normal suggest/prompt path.
    let err = resolve_image_deploy_version(
        "legal-agent",
        "latest",
        VersionFlags::default(),
        Some("1.0.0"),
        &["1.0.0".to_string()],
        "deploy",
    )
    .unwrap_err();
    assert!(err.to_string().contains("no usable"));
}

#[test]
fn mismatched_version_flag_and_image_tag_is_rejected() {
    // A --version that disagrees with the image ref's own tag must be a hard
    // error, not silently win — otherwise the bytes tagged 1.0.1 would ship
    // under the label 9.9.9, decoupling the artifact from its version again.
    let err = resolve_image_deploy_version(
        "legal-agent:1.0.1",
        "1.0.1",
        VersionFlags {
            version: Some("9.9.9"),
            ..Default::default()
        },
        Some("1.0.0"),
        &["1.0.0".to_string()],
        "deploy",
    )
    .unwrap_err();
    assert!(err.to_string().contains("must match"));
}

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

// ─── already_pushed ───────────────────────────────────────────────────────────

#[test]
fn already_pushed_is_false_for_a_brand_new_agent() {
    let srv = mockito::Server::new();
    let client = Client::for_test(&srv.url(), None);
    assert!(!already_pushed(&client, None, "1.0.0").unwrap());
}

#[test]
fn already_pushed_is_true_for_a_pushed_row() {
    let mut srv = mockito::Server::new();
    srv.mock("GET", "/api/agents/agent-1/versions")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            r#"{"data": [
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
    assert!(already_pushed(&client, Some(&existing), "2.0.0").unwrap());
}

#[test]
fn already_pushed_is_false_for_an_active_version() {
    // Deploy must not treat an already-active version as "promote without
    // rebuilding" — that path is only for a pushed-but-undeployed draft.
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
    assert!(!already_pushed(&client, Some(&existing), "1.0.0").unwrap());
}

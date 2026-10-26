use super::*;
use crate::util::dir_references_mcp_gateway;

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

// ─── dir_references_mcp_gateway ──────────────────────────────────────────────

#[test]
fn dir_references_mcp_gateway_finds_a_reference_in_a_nested_source_file() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::write(
        dir.path().join("src/main.py"),
        "gateway_url = os.environ['MCP_GATEWAY_URL']\n",
    )
    .unwrap();
    assert!(dir_references_mcp_gateway(dir.path()));
}

#[test]
fn dir_references_mcp_gateway_is_false_with_no_reference() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("main.py"), "print('hello')\n").unwrap();
    assert!(!dir_references_mcp_gateway(dir.path()));
}

#[test]
fn dir_references_mcp_gateway_ignores_skipped_directories() {
    // A reference sitting only inside a vendored/build dir shouldn't count — a
    // real MCP_GATEWAY_TOKEN in the repo's own .env, say, isn't the agent's own
    // code calling the gateway.
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("node_modules/pkg")).unwrap();
    std::fs::write(
        dir.path().join("node_modules/pkg/index.js"),
        "const x = 'MCP_GATEWAY_TOKEN';\n",
    )
    .unwrap();
    assert!(!dir_references_mcp_gateway(dir.path()));
}

// ─── deploy --dry-run plan ───────────────────────────────────────────────────

fn plan_for<'a>(
    current_version: Option<&'a str>,
    already_pushed: bool,
    env: &'a HashMap<String, String>,
) -> DeployPlan<'a> {
    DeployPlan {
        agent_name: "weather",
        agent_exists: current_version.is_some(),
        current_version,
        version: "1.3.0",
        image_ref: "nasiko/weather:1.3.0",
        already_pushed,
        builds_image: true,
        port: 8000,
        workload_kind: None,
        hitl: false,
        writable: false,
        writable_path: None,
        env,
    }
}

#[test]
fn dry_run_plan_never_shows_env_values() {
    let env = HashMap::from([
        ("API_KEY".to_string(), "sk-live-secret".to_string()),
        ("DB_URL".to_string(), "postgres://user:pw@db".to_string()),
    ]);
    let text = plan_lines(&plan_for(Some("1.2.0"), false, &env)).join("\n");
    assert!(text.contains("API_KEY=****"));
    assert!(text.contains("DB_URL=****"));
    assert!(!text.contains("sk-live-secret"));
    assert!(!text.contains("postgres://"));
}

#[test]
fn dry_run_plan_describes_update_vs_new_agent_and_reuse_vs_build() {
    let env = HashMap::new();
    let update = plan_lines(&plan_for(Some("1.2.0"), false, &env)).join("\n");
    assert!(update.contains("update 1.2.0 → 1.3.0"));
    assert!(update.contains("(build + push)"));
    assert!(update.contains("env       (none)"));

    let new_agent = plan_lines(&plan_for(None, true, &env)).join("\n");
    assert!(new_agent.contains("register new agent at 1.3.0"));
    assert!(new_agent.contains("(reuse the already-pushed image)"));
}

#[test]
fn dry_run_plan_shows_the_writable_mount() {
    let env = HashMap::new();
    let mut plan = plan_for(None, false, &env);
    plan.writable = true;
    assert!(
        plan_lines(&plan)
            .join("\n")
            .contains("writable  yes, at /workspace")
    );
    plan.writable_path = Some("/data");
    assert!(
        plan_lines(&plan)
            .join("\n")
            .contains("writable  yes, at /data")
    );
}

#[test]
fn dry_run_plan_shows_a_coding_agent_deploys_port_less() {
    let env = HashMap::new();
    let plain = plan_lines(&plan_for(None, false, &env)).join("\n");
    assert!(plain.contains("port      8000"));
    assert!(!plain.contains("kind"));

    let mut plan = plan_for(None, false, &env);
    plan.workload_kind = Some("acp_coding_agent");
    plan.hitl = true;
    let coding = plan_lines(&plan).join("\n");
    assert!(coding.contains("kind      coding agent, HITL on"));
    assert!(coding.contains("port      none (driven over stdio)"));
}

#[test]
fn dry_run_plan_says_what_happens_to_hitl_when_it_is_not_turned_on() {
    // The deploy only ever sends HITL as on: a new agent is created with it off, and an
    // existing agent keeps whatever it has.
    let env = HashMap::new();
    let mut plan = plan_for(None, false, &env);
    plan.workload_kind = Some("acp_coding_agent");
    assert!(
        plan_lines(&plan)
            .join("\n")
            .contains("coding agent, HITL off")
    );
    plan.agent_exists = true;
    assert!(
        plan_lines(&plan)
            .join("\n")
            .contains("coding agent, HITL unchanged")
    );

    // HITL means nothing for other kinds, so their line doesn't mention it.
    plan.workload_kind = Some("agent");
    let other = plan_lines(&plan).join("\n");
    assert!(
        other.contains("kind      agent") && !other.contains("HITL"),
        "{other}"
    );
}

#[test]
fn dry_run_plan_names_an_existing_agent_without_a_live_version_and_a_raw_image_push() {
    let env = HashMap::new();
    let mut plan = plan_for(None, false, &env);
    plan.agent_exists = true;
    plan.builds_image = false;
    let text = plan_lines(&plan).join("\n");
    assert!(
        text.contains("deploy 1.3.0 to the existing agent"),
        "{text}"
    );
    assert!(text.contains("(push the local image)"), "{text}");
}

// ─── resolve_workload_kind / deploy_ports ────────────────────────────────────

#[test]
fn coding_agent_flag_wins_over_the_card() {
    // The explicit flag marks a coding agent even when the card says otherwise.
    let card = serde_json::json!({ "workloadKind": "agent" });
    assert_eq!(
        resolve_workload_kind(true, &card).as_deref(),
        Some("acp_coding_agent")
    );
}

#[test]
fn card_workload_kind_is_the_default_without_the_flag() {
    let card = serde_json::json!({ "workloadKind": "acp_coding_agent" });
    assert_eq!(
        resolve_workload_kind(false, &card).as_deref(),
        Some("acp_coding_agent")
    );
}

#[test]
fn no_flag_and_no_card_field_means_ordinary_agent() {
    // No flag, no (or empty) card field → None, so the server defaults to `agent`.
    assert_eq!(resolve_workload_kind(false, &serde_json::json!({})), None);
    assert_eq!(
        resolve_workload_kind(false, &serde_json::json!({ "workloadKind": "" })),
        None
    );
}

#[test]
fn a_coding_agent_deploys_port_less() {
    // Coding agents are driven over stdio, so they carry no container port;
    // everything else keeps its service port.
    assert!(deploy_ports(Some("acp_coding_agent"), 8000).is_empty());
    assert_eq!(deploy_ports(None, 8000), vec![8000]);
    assert_eq!(deploy_ports(Some("agent"), 8000), vec![8000]);
}

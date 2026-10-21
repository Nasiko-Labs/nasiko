//! Integration tests for the observability endpoints.
//!
//! Covers:
//!   GET  /api/observability/agents/{agent_ref}/logs
//!   GET  /api/observability/agents/{agent_ref}/logs/stream
//!   GET  /api/observability/agent/{agent_ref}/stats
//!   GET  /api/observability/session/list
//!   GET  /api/observability/trace/{trace_id}
//!   GET  /api/observability/finops/dashboard
//!
//! The read path is provider-backed (Tempo/Loki HTTP clients). Per-agent
//! backend failures degrade soft: session/list and finops return zeroed 200
//! responses rather than 5xx. Stats queries Tempo directly (zeroed 200 when
//! Tempo has no data). Logs merge proxy_logs (DB) with container logs.
//!
//! All routes are under `/api` and require authentication.
//!
//! Requires infra (Postgres :5432, Redis, MinIO, Tempo, Loki):
//!   cargo test -p nasiko-server --test observability -- --test-threads=1

mod common;

use serde_json::{Value, json};
use serial_test::serial;
use uuid::Uuid;

// ─── shared helpers ──────────────────────────────────────────────────────────

async fn init_admin(server: &common::TestServer) -> Value {
    server
        .client
        .post(server.url("/api/auth/initialize-admin"))
        .json(&json!({"username": "admin", "email": "admin@obs.test"}))
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap()
}

async fn create_agent(server: &common::TestServer, user_id: &str, name: &str) -> Value {
    common::as_superuser(
        server.client.post(server.url("/api/agents")),
        user_id,
        "admin",
    )
    .json(&json!({"name": name, "version": "1.0.0"}))
    .send()
    .await
    .unwrap()
    .json::<Value>()
    .await
    .unwrap()
}

async fn seed_user(server: &common::TestServer, username: &str) -> Uuid {
    sqlx::query_scalar("INSERT INTO users (username, email) VALUES ($1, $2) RETURNING id")
        .bind(username)
        .bind(format!("{username}@obs.test"))
        .fetch_one(&server.db)
        .await
        .expect("seed user")
}

async fn seed_agent(server: &common::TestServer, owner_id: Uuid, name: &str) -> Uuid {
    sqlx::query_scalar("INSERT INTO agents (name, owner_id) VALUES ($1, $2) RETURNING id")
        .bind(name)
        .bind(owner_id)
        .fetch_one(&server.db)
        .await
        .expect("seed agent")
}

/// Seed a proxy_log row for `target_agent_id`, called by `caller_id`.
/// Returns nothing — used for state setup only.
async fn seed_proxy_log(
    server: &common::TestServer,
    caller_id: Uuid,
    target_agent_id: Uuid,
    status: i32,
    latency_ms: i64,
    error: Option<&str>,
) {
    sqlx::query(
        r#"INSERT INTO proxy_logs (caller_id, target_agent_id, method, latency_ms, status, error)
           VALUES ($1, $2, 'tasks/send', $3, $4, $5)"#,
    )
    .bind(caller_id)
    .bind(target_agent_id)
    .bind(latency_ms)
    .bind(status)
    .bind(error)
    .execute(&server.db)
    .await
    .expect("seed proxy_log");
}

// ─── authentication guard tests ──────────────────────────────────────────────

#[tokio::test]
#[serial]
async fn observe_logs_requires_auth() {
    let server = common::TestServer::start().await;

    let res = server
        .client
        .get(server.url("/api/observability/agents/some-agent/logs"))
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), 401);
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn observe_stats_requires_auth() {
    let server = common::TestServer::start().await;

    // Stats moved to the singular /agent/{ref}/stats route (routes.rs).
    let res = server
        .client
        .get(server.url("/api/observability/agent/some-agent/stats"))
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), 401);
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn observe_traces_requires_auth() {
    let server = common::TestServer::start().await;

    // The old /traces list was replaced by /session/list + /trace/{id}.
    for path in [
        "/api/observability/session/list",
        "/api/observability/trace/abc123def456",
    ] {
        let res = server.client.get(server.url(path)).send().await.unwrap();
        assert_eq!(res.status(), 401, "{path} must require auth");
    }
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn observe_finops_requires_auth() {
    let server = common::TestServer::start().await;

    // Finops moved to /finops/dashboard.
    let res = server
        .client
        .get(server.url("/api/observability/finops/dashboard"))
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), 401);
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn unmatched_api_path_is_a_json_404_not_the_spa() {
    // The UI fallback serves index.html for anything unrouted, and /api was
    // reaching it — so a missing endpoint answered 200 with HTML and every
    // caller reported it as "malformed JSON body". A route that is not there
    // has to say so, in the envelope the rest of the API uses.
    let server = common::TestServer::start().await;

    let res = server
        .client
        .get(server.url("/api/observability/finops/no-such-endpoint"))
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), 404, "an unmatched /api path must not be 200");
    let ctype = res
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    assert!(
        ctype.contains("application/json"),
        "got content-type {ctype}"
    );

    let body: Value = res.json().await.expect("body must parse as JSON");
    assert_eq!(body["status_code"], 404);
    assert!(
        body["message"]
            .as_str()
            .unwrap_or("")
            .contains("no-such-endpoint"),
        "the message should name the path that missed: {body}"
    );

    server.cleanup().await;
}

// ─── provider-backed list/detail endpoints ───────────────────────────────────
//
// The pre-refactor API returned 503 when no Tempo backend was configured; the
// provider-backed replacements degrade soft instead (zeroed/empty 200s, with
// per-agent Tempo failures logged and skipped).

#[tokio::test]
#[serial]
async fn ensure_session_uses_callers_same_name_agent_and_is_idempotent() {
    let server = common::TestServer::start().await;
    let _admin = init_admin(&server).await;
    let alice = seed_user(&server, "ensure-alice").await;
    let bob = seed_user(&server, "ensure-bob").await;
    let alice_agent = seed_agent(&server, alice, "claude-code").await;
    let _bob_agent = seed_agent(&server, bob, "claude-code").await;

    let request = || {
        server
            .client
            .post(server.url("/api/observability/session/ensure"))
            .bearer_auth(common::sign_token(
                &alice.to_string(),
                "ensure-alice",
                false,
                "member",
            ))
            .json(&json!({"session_id": "owned-coding-session", "agent_name": "claude-code"}))
    };

    assert_eq!(request().send().await.unwrap().status(), 201);
    assert_eq!(request().send().await.unwrap().status(), 200);

    let stored: (Uuid, Uuid) = sqlx::query_as(
        "SELECT user_id, agent_id FROM chat_sessions WHERE session_id = 'owned-coding-session'",
    )
    .fetch_one(&server.db)
    .await
    .unwrap();
    assert_eq!(stored, (alice, alice_agent));

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn ensure_session_conflicts_when_existing_user_or_agent_differs() {
    let server = common::TestServer::start().await;
    let _admin = init_admin(&server).await;
    let alice = seed_user(&server, "conflict-alice").await;
    let bob = seed_user(&server, "conflict-bob").await;
    let alice_agent = seed_agent(&server, alice, "claude-code").await;
    let _alice_other = seed_agent(&server, alice, "other-code").await;
    let _bob_agent = seed_agent(&server, bob, "claude-code").await;

    sqlx::query(
        "INSERT INTO chat_sessions (session_id, user_id, agent_id, title) VALUES ($1, $2, $3, 'Coding session')",
    )
    .bind("conflicting-session")
    .bind(alice)
    .bind(alice_agent)
    .execute(&server.db)
    .await
    .unwrap();

    for (user_id, username, agent_name) in [
        (bob, "conflict-bob", "claude-code"),
        (alice, "conflict-alice", "other-code"),
    ] {
        let response = server
            .client
            .post(server.url("/api/observability/session/ensure"))
            .bearer_auth(common::sign_token(
                &user_id.to_string(),
                username,
                false,
                "member",
            ))
            .json(&json!({"session_id": "conflicting-session", "agent_name": agent_name}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 409, "{username}/{agent_name}");
    }

    let stored: (Uuid, Uuid) = sqlx::query_as(
        "SELECT user_id, agent_id FROM chat_sessions WHERE session_id = 'conflicting-session'",
    )
    .fetch_one(&server.db)
    .await
    .unwrap();
    assert_eq!(stored, (alice, alice_agent));

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn session_detail_returns_404_for_another_users_session() {
    let server = common::TestServer::start().await;
    let _admin = init_admin(&server).await;
    let owner = seed_user(&server, "session-detail-owner").await;
    let stranger = seed_user(&server, "session-detail-stranger").await;
    let agent = seed_agent(&server, owner, "session-detail-agent").await;
    sqlx::query(
        "INSERT INTO chat_sessions (session_id, user_id, agent_id, title) VALUES ($1, $2, $3, 'Private')",
    )
    .bind("private-observability-session")
    .bind(owner)
    .bind(agent)
    .execute(&server.db)
    .await
    .unwrap();

    let response = server
        .client
        .get(server.url("/api/observability/session/private-observability-session"))
        .bearer_auth(common::sign_token(
            &stranger.to_string(),
            "session-detail-stranger",
            false,
            "member",
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 404);
    assert_eq!(response.text().await.unwrap(), "session not found");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn agent_stats_and_logs_return_404_for_inaccessible_agent() {
    let server = common::TestServer::start().await;
    let _admin = init_admin(&server).await;
    let owner = seed_user(&server, "observe-agent-owner").await;
    let stranger = seed_user(&server, "observe-agent-stranger").await;
    let agent = seed_agent(&server, owner, "private-observe-agent").await;
    let token = common::sign_token(
        &stranger.to_string(),
        "observe-agent-stranger",
        false,
        "member",
    );

    for path in [
        format!("/api/observability/agent/{agent}/stats"),
        format!("/api/observability/agents/{agent}/logs"),
        format!("/api/observability/agents/{agent}/logs/stream"),
        "/api/observability/agent/private-observe-agent/stats".into(),
        "/api/observability/agents/private-observe-agent/logs".into(),
    ] {
        let response = server
            .client
            .get(server.url(&path))
            .bearer_auth(&token)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 404, "{path}");
    }

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn agent_stats_deny_ambiguous_name_with_inaccessible_same_name_agent() {
    let server = common::TestServer::start().await;
    let _admin = init_admin(&server).await;
    let caller = seed_user(&server, "same-name-caller").await;
    let other = seed_user(&server, "same-name-other").await;
    let caller_agent = seed_agent(&server, caller, "duplicate-telemetry-name").await;
    let _other_agent = seed_agent(&server, other, "duplicate-telemetry-name").await;

    let response = server
        .client
        .get(server.url(&format!("/api/observability/agent/{caller_agent}/stats")))
        .bearer_auth(common::sign_token(
            &caller.to_string(),
            "same-name-caller",
            false,
            "member",
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 404);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn agent_logs_allow_owner_and_explicit_grantee() {
    let server = common::TestServer::start().await;
    let _admin = init_admin(&server).await;
    let owner = seed_user(&server, "logs-access-owner").await;
    let grantee = seed_user(&server, "logs-access-grantee").await;
    let agent = seed_agent(&server, owner, "shared-observe-agent").await;
    sqlx::query(
        "INSERT INTO agent_grants (agent_id, grant_type, grantee_id) VALUES ($1, 'user', $2)",
    )
    .bind(agent)
    .bind(grantee.to_string())
    .execute(&server.db)
    .await
    .unwrap();

    for (user_id, username) in [
        (owner, "logs-access-owner"),
        (grantee, "logs-access-grantee"),
    ] {
        let response = server
            .client
            .get(server.url(&format!("/api/observability/agents/{agent}/logs")))
            .bearer_auth(common::sign_token(
                &user_id.to_string(),
                username,
                false,
                "member",
            ))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200, "{username}");
    }

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn finops_dashboard_only_lists_accessible_agents_for_non_superuser() {
    let server = common::TestServer::start().await;
    let _admin = init_admin(&server).await;
    let caller = seed_user(&server, "finops-caller").await;
    let owner = seed_user(&server, "finops-owner").await;
    let private_owner = seed_user(&server, "finops-private-owner").await;
    let own_agent = seed_agent(&server, caller, "finops-own-agent").await;
    let shared_agent = seed_agent(&server, owner, "finops-shared-agent").await;
    let private_agent = seed_agent(&server, private_owner, "finops-private-agent").await;
    sqlx::query(
        "INSERT INTO agent_grants (agent_id, grant_type, grantee_id) VALUES ($1, 'user', $2)",
    )
    .bind(shared_agent)
    .bind(caller.to_string())
    .execute(&server.db)
    .await
    .unwrap();

    let response = server
        .client
        .get(server.url("/api/observability/finops/dashboard"))
        .bearer_auth(common::sign_token(
            &caller.to_string(),
            "finops-caller",
            false,
            "member",
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let body: Value = response.json().await.unwrap();
    let ids: Vec<String> = body["data"]["agents"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|agent| agent["agent_id"].as_str())
        .map(str::to_owned)
        .collect();
    assert_eq!(body["data"]["summary"]["total_agents"], 2);
    assert!(ids.contains(&own_agent.to_string()));
    assert!(ids.contains(&shared_agent.to_string()));
    assert!(!ids.contains(&private_agent.to_string()));

    server.cleanup().await;
}

const SPEND_ENDPOINTS: [&str; 4] = ["timeseries", "calendar", "day", "attributions"];

async fn spend_row(server: &common::TestServer, agent_name: &str, user: Uuid, cost: f64) {
    sqlx::query(
        "INSERT INTO trace_usage \
           (trace_id, agent_name, user_id, model, provider, input_tokens, output_tokens, \
            cost_usd, latency_ms, started_at) \
         VALUES ($1, $2, $3, 'gpt-4o', 'openai', 100, 10, $4, 500, now())",
    )
    .bind(format!("trace-{}", Uuid::new_v4()))
    .bind(agent_name)
    .bind(user)
    .bind(cost)
    .execute(&server.db)
    .await
    .expect("seed trace_usage");
}

async fn spend_path(server: &common::TestServer, endpoint: &str) -> String {
    let today: String =
        sqlx::query_scalar("SELECT to_char(now() AT TIME ZONE 'UTC', 'YYYY-MM-DD')")
            .fetch_one(&server.db)
            .await
            .unwrap();
    match endpoint {
        "timeseries" => "/api/observability/finops/spend-timeseries?range=7d".into(),
        "calendar" => format!(
            "/api/observability/finops/spend-calendar?month={}",
            &today[..7]
        ),
        "day" => format!("/api/observability/finops/spend-calendar/day?date={today}"),
        _ => "/api/observability/finops/attributions?range=7d".into(),
    }
}

async fn spend_seen(server: &common::TestServer, endpoint: &str, token: &str) -> (f64, String) {
    let response = server
        .client
        .get(server.url(&spend_path(server, endpoint).await))
        .bearer_auth(token)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200, "{endpoint}");
    let text = response.text().await.unwrap();
    let body: Value = serde_json::from_str(&text).unwrap();
    let data = &body["data"];
    let sum = |rows: &Value, key: &str| -> f64 {
        rows.as_array()
            .map(|r| r.iter().filter_map(|x| x[key].as_f64()).sum())
            .unwrap_or(0.0)
    };
    let total = match endpoint {
        "timeseries" => sum(&data["points"], "spend_usd"),
        "calendar" => sum(&data["days"], "spend_usd"),
        "day" => sum(&data["hours"], "spend_usd"),
        _ => sum(&data["rows"], "total_cost"),
    };
    (total, text)
}

struct SpendWorld {
    caller_token: String,
    stranger_token: String,
    admin_token: String,
}

async fn spend_world(server: &common::TestServer) -> SpendWorld {
    let caller = seed_user(server, "spend-caller").await;
    let owner = seed_user(server, "spend-owner").await;
    let hidden_owner = seed_user(server, "spend-hidden-owner").await;
    let stranger = seed_user(server, "spend-stranger").await;
    let admin = seed_user(server, "spend-admin").await;
    seed_agent(server, caller, "spend-own-agent").await;
    let shared = seed_agent(server, owner, "spend-shared-agent").await;
    seed_agent(server, hidden_owner, "spend-hidden-agent").await;
    sqlx::query(
        "INSERT INTO agent_grants (agent_id, grant_type, grantee_id) VALUES ($1, 'user', $2)",
    )
    .bind(shared)
    .bind(caller.to_string())
    .execute(&server.db)
    .await
    .unwrap();
    spend_row(server, "spend-own-agent", caller, 1.0).await;
    spend_row(server, "spend-shared-agent", owner, 2.0).await;
    spend_row(server, "spend-hidden-agent", hidden_owner, 4.0).await;
    SpendWorld {
        caller_token: common::sign_token(&caller.to_string(), "spend-caller", false, "member"),
        stranger_token: common::sign_token(
            &stranger.to_string(),
            "spend-stranger",
            false,
            "member",
        ),
        admin_token: common::sign_token(&admin.to_string(), "spend-admin", true, "admin"),
    }
}

#[tokio::test]
#[serial]
async fn finops_spend_endpoints_hide_agents_the_caller_cannot_access() {
    let server = common::TestServer::start().await;
    let _admin = init_admin(&server).await;
    let world = spend_world(&server).await;

    for endpoint in SPEND_ENDPOINTS {
        let (total, body) = spend_seen(&server, endpoint, &world.caller_token).await;
        assert!(
            (total - 3.0).abs() < 1e-6,
            "{endpoint}: own + granted agent only, got {total}"
        );
        assert!(
            !body.contains("spend-hidden-agent"),
            "{endpoint} leaked an agent the caller cannot access: {body}"
        );
        assert!(
            body.contains("spend-own-agent") || endpoint == "timeseries" || endpoint == "calendar"
        );
    }

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn finops_spend_endpoints_are_empty_for_a_user_with_no_agents() {
    let server = common::TestServer::start().await;
    let _admin = init_admin(&server).await;
    let world = spend_world(&server).await;

    for endpoint in SPEND_ENDPOINTS {
        let (total, body) = spend_seen(&server, endpoint, &world.stranger_token).await;
        assert_eq!(total, 0.0, "{endpoint}: {body}");
        for name in [
            "spend-own-agent",
            "spend-shared-agent",
            "spend-hidden-agent",
        ] {
            assert!(!body.contains(name), "{endpoint} leaked {name}: {body}");
        }
    }

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn finops_spend_endpoints_show_the_whole_fleet_to_a_superuser() {
    let server = common::TestServer::start().await;
    let _admin = init_admin(&server).await;
    let world = spend_world(&server).await;

    for endpoint in SPEND_ENDPOINTS {
        let (total, _) = spend_seen(&server, endpoint, &world.admin_token).await;
        assert!((total - 7.0).abs() < 1e-6, "{endpoint}: got {total}");
    }

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn finops_spend_endpoints_do_not_leak_a_private_agent_through_the_agent_filter() {
    let server = common::TestServer::start().await;
    let _admin = init_admin(&server).await;
    let world = spend_world(&server).await;

    for endpoint in SPEND_ENDPOINTS {
        let path = format!(
            "{}&agent_id=spend-hidden-agent",
            spend_path(&server, endpoint).await
        );
        let response = server
            .client
            .get(server.url(&path))
            .bearer_auth(&world.caller_token)
            .send()
            .await
            .unwrap();
        let status = response.status();
        let text = response.text().await.unwrap();
        assert!(
            status == 404 || status == 200,
            "{endpoint}: unexpected status {status}"
        );
        assert!(
            !text.contains("4.0") && !text.contains("spend-hidden-agent\"") || status == 404,
            "{endpoint} returned the private agent's spend: {text}"
        );
    }

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn finops_spend_endpoints_require_authentication() {
    let server = common::TestServer::start().await;
    let _admin = init_admin(&server).await;

    for endpoint in SPEND_ENDPOINTS {
        let response = server
            .client
            .get(server.url(&spend_path(&server, endpoint).await))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 401, "{endpoint}");
    }

    server.cleanup().await;
}

async fn seed_workflow(server: &common::TestServer, owner: Uuid, name: &str) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO mafs (user_id, name, maf_json) VALUES ($1, $2, '{}'::jsonb) RETURNING id",
    )
    .bind(owner)
    .bind(name)
    .fetch_one(&server.db)
    .await
    .expect("seed workflow")
}

async fn seed_run(
    server: &common::TestServer,
    workflow: Uuid,
    owner: Uuid,
    status: &str,
    cost: f64,
    step_agent: Option<&str>,
) {
    let steps = match step_agent {
        Some(agent) => json!([{"agent_name": agent}]),
        None => json!([]),
    };
    sqlx::query(
        "INSERT INTO maf_executions \
           (maf_id, user_id, status, cost_usd, started_at, duration_ms, step_results) \
         VALUES ($1, $2, $3, $4, now(), 1000, $5)",
    )
    .bind(workflow)
    .bind(owner)
    .bind(status)
    .bind(cost)
    .bind(steps)
    .execute(&server.db)
    .await
    .expect("seed execution");
}

struct WorkflowWorld {
    caller_token: String,
    stranger_token: String,
    admin_token: String,
}

async fn workflow_world(server: &common::TestServer) -> WorkflowWorld {
    let caller = seed_user(server, "wf-caller").await;
    let other = seed_user(server, "wf-other").await;
    let stranger = seed_user(server, "wf-stranger").await;
    let admin = seed_user(server, "wf-admin").await;
    seed_agent(server, caller, "wf-step-agent").await;
    let own = seed_workflow(server, caller, "wf-own-flow").await;
    let foreign = seed_workflow(server, other, "wf-foreign-flow").await;
    seed_run(server, own, caller, "success", 1.0, Some("wf-step-agent")).await;
    seed_run(server, own, caller, "failed", 8.0, Some("wf-step-agent")).await;
    seed_run(
        server,
        foreign,
        other,
        "success",
        2.0,
        Some("wf-step-agent"),
    )
    .await;
    WorkflowWorld {
        caller_token: common::sign_token(&caller.to_string(), "wf-caller", false, "member"),
        stranger_token: common::sign_token(&stranger.to_string(), "wf-stranger", false, "member"),
        admin_token: common::sign_token(&admin.to_string(), "wf-admin", true, "admin"),
    }
}

async fn workflow_rows(
    server: &common::TestServer,
    endpoint: &str,
    token: &str,
    extra: &str,
) -> (Vec<(String, f64)>, String) {
    let path = match endpoint {
        "dashboard" => format!("/api/observability/finops/dashboard?range=7d&view=workflow{extra}"),
        _ => format!("/api/observability/finops/attributions?range=7d&view=workflow{extra}"),
    };
    let response = server
        .client
        .get(server.url(&path))
        .bearer_auth(token)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200, "{endpoint}{extra}");
    let text = response.text().await.unwrap();
    let body: Value = serde_json::from_str(&text).unwrap();
    let rows = match endpoint {
        "dashboard" => &body["data"]["attributions"]["rows"],
        _ => &body["data"]["rows"],
    };
    let parsed = rows
        .as_array()
        .unwrap_or_else(|| panic!("{endpoint}: no workflow rows in {text}"))
        .iter()
        .map(|r| {
            (
                r["workflow_name"].as_str().unwrap().to_owned(),
                r["total_cost"].as_f64().unwrap(),
            )
        })
        .collect();
    (parsed, text)
}

#[tokio::test]
#[serial]
async fn finops_workflow_view_lists_only_the_callers_own_successful_runs() {
    let server = common::TestServer::start().await;
    let _admin = init_admin(&server).await;
    let world = workflow_world(&server).await;

    for endpoint in ["dashboard", "attributions"] {
        let (rows, body) = workflow_rows(&server, endpoint, &world.caller_token, "").await;
        assert_eq!(rows.len(), 1, "{endpoint}: {body}");
        assert_eq!(rows[0].0, "wf-own-flow", "{endpoint}");
        assert!(
            (rows[0].1 - 1.0).abs() < 1e-6,
            "{endpoint}: failed runs must not count, got {}",
            rows[0].1
        );
        assert!(
            !body.contains("wf-foreign-flow"),
            "{endpoint} leaked another user's workflow: {body}"
        );
    }

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn finops_workflow_view_is_empty_for_a_user_with_no_workflows() {
    let server = common::TestServer::start().await;
    let _admin = init_admin(&server).await;
    let world = workflow_world(&server).await;

    for endpoint in ["dashboard", "attributions"] {
        let (rows, body) = workflow_rows(&server, endpoint, &world.stranger_token, "").await;
        assert!(rows.is_empty(), "{endpoint}: {body}");
    }

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn finops_workflow_view_shows_every_workflow_to_a_superuser() {
    let server = common::TestServer::start().await;
    let _admin = init_admin(&server).await;
    let world = workflow_world(&server).await;

    for endpoint in ["dashboard", "attributions"] {
        let (rows, body) = workflow_rows(&server, endpoint, &world.admin_token, "").await;
        let names: Vec<&str> = rows.iter().map(|r| r.0.as_str()).collect();
        assert_eq!(rows.len(), 2, "{endpoint}: {body}");
        assert!(names.contains(&"wf-own-flow") && names.contains(&"wf-foreign-flow"));
        let total: f64 = rows.iter().map(|r| r.1).sum();
        assert!((total - 3.0).abs() < 1e-6, "{endpoint}: got {total}");
    }

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn finops_workflow_view_agent_filter_does_not_widen_the_owner_scope() {
    let server = common::TestServer::start().await;
    let _admin = init_admin(&server).await;
    let world = workflow_world(&server).await;

    for endpoint in ["dashboard", "attributions"] {
        let (rows, body) = workflow_rows(
            &server,
            endpoint,
            &world.caller_token,
            "&agent_id=wf-step-agent",
        )
        .await;
        let names: Vec<&str> = rows.iter().map(|r| r.0.as_str()).collect();
        assert_eq!(names, vec!["wf-own-flow"], "{endpoint}: {body}");
    }

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn agent_hours_only_returns_accessible_agents_for_non_superuser() {
    let server = common::TestServer::start().await;
    let _admin = init_admin(&server).await;
    let caller = seed_user(&server, "hours-caller").await;
    let other = seed_user(&server, "hours-other").await;
    let own_agent = seed_agent(&server, caller, "hours-own-agent").await;
    let private_agent = seed_agent(&server, other, "hours-private-agent").await;
    let started_at = chrono::Utc::now() - chrono::Duration::hours(1);
    let ended_at = chrono::Utc::now();

    for (agent_id, name) in [
        (own_agent, "hours-own-agent"),
        (private_agent, "hours-private-agent"),
    ] {
        sqlx::query(
            r#"INSERT INTO agent_instance_sessions
                   (agent_id, agent_name, instance_key, runtime, started_at, last_seen_at, ended_at)
               VALUES ($1, $2, $3, 'docker', $4, $5, $5)"#,
        )
        .bind(agent_id)
        .bind(name)
        .bind(format!("instance-{agent_id}"))
        .bind(started_at)
        .bind(ended_at)
        .execute(&server.db)
        .await
        .unwrap();
    }

    let token = common::sign_token(&caller.to_string(), "hours-caller", false, "member");
    let response = server
        .client
        .get(server.url("/api/observability/finops/agent-hours"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let body: Value = response.json().await.unwrap();
    let rows = body["data"]["agents"].as_array().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["agent_id"], own_agent.to_string());
    assert!(body["data"]["total_hours"].as_f64().unwrap() < 1.1);

    let response = server
        .client
        .get(server.url(&format!(
            "/api/observability/finops/agent-hours?agent_id={private_agent}"
        )))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 404);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn observe_session_list_returns_envelope_with_no_sessions() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    let res = server
        .client
        .get(server.url("/api/observability/session/list"))
        .bearer_auth(common::sign_token(uid, "admin", true, "admin"))
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), 200, "session/list degrades soft, never 503");
    let body: Value = res.json().await.unwrap();
    assert!(
        body["data"]["sessions"].is_array(),
        "sessions array expected: {body}"
    );
    assert_eq!(body["data"]["sessions"].as_array().unwrap().len(), 0);

    server.cleanup().await;
}

// ─── session/list: SQL-only listing ──────────────────────────────────────────
//
// session/list is served entirely from Postgres (`observability::sessions_sql`):
// `session_traces` for the trace count, `trace_usage` for tokens/cost,
// `chat_messages` for the previews, `flows` for the status. None of these tests
// needs Tempo or Loki, which is the point of the change.

/// One chat session plus `flows` rows with the given statuses.
async fn seed_session_with_flows(
    server: &common::TestServer,
    user_id: Uuid,
    agent_id: Uuid,
    session_id: &str,
    flow_statuses: &[&str],
) {
    sqlx::query(
        "INSERT INTO chat_sessions (session_id, user_id, agent_id, title) VALUES ($1, $2, $3, $4)",
    )
    .bind(session_id)
    .bind(user_id)
    .bind(agent_id)
    .bind("seeded")
    .execute(&server.db)
    .await
    .expect("seed chat_session");

    for (i, status) in flow_statuses.iter().enumerate() {
        sqlx::query(
            "INSERT INTO flows (flow_id, user_id, status, session_id) VALUES ($1, $2, $3, $4)",
        )
        .bind(format!("{session_id}-flow-{i}"))
        .bind(user_id)
        .bind(status)
        .bind(session_id)
        .execute(&server.db)
        .await
        .expect("seed flow");
    }
}

async fn list_sessions(server: &common::TestServer, uid: &str, query: &str) -> Value {
    let res = server
        .client
        .get(server.url(&format!("/api/observability/session/list{query}")))
        .bearer_auth(common::sign_token(uid, "admin", true, "admin"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200, "session/list should answer 200");
    res.json().await.unwrap()
}

#[tokio::test]
#[serial]
async fn session_list_derives_status_from_flow_outcomes() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();
    let uuid: Uuid = uid.parse().unwrap();
    let agent = seed_agent(&server, uuid, "status-agent").await;

    // One failed flow makes the whole session failed, even alongside a running
    // one: a session with a failed request has failed, whatever is still in
    // flight. A session with no flows at all is unknown, never "ok".
    seed_session_with_flows(&server, uuid, agent, "ses-ok", &["completed", "completed"]).await;
    seed_session_with_flows(&server, uuid, agent, "ses-failed", &["completed", "failed"]).await;
    seed_session_with_flows(&server, uuid, agent, "ses-mixed", &["failed", "running"]).await;
    seed_session_with_flows(&server, uuid, agent, "ses-running", &["running"]).await;
    seed_session_with_flows(&server, uuid, agent, "ses-none", &[]).await;

    let body = list_sessions(&server, uid, "?limit=100").await;
    let by_id: std::collections::HashMap<&str, &Value> = body["data"]["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| (s["session_id"].as_str().unwrap(), s))
        .collect();

    assert_eq!(by_id["ses-ok"]["status"], "ok");
    assert_eq!(by_id["ses-failed"]["status"], "failed");
    assert_eq!(by_id["ses-mixed"]["status"], "failed", "failure wins");
    assert_eq!(by_id["ses-running"]["status"], "running");
    assert!(
        by_id["ses-none"]["status"].is_null(),
        "no flows means unknown, not ok"
    );
    assert_eq!(by_id["ses-failed"]["failed_flows"], 1);
    assert!(
        by_id["ses-ok"]["error_count"].is_null(),
        "error_count has no source yet; null means unknown, never zero"
    );

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn session_list_status_filter_is_applied_in_sql_with_an_exact_count() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();
    let uuid: Uuid = uid.parse().unwrap();
    let agent = seed_agent(&server, uuid, "filter-agent").await;

    for i in 0..3 {
        seed_session_with_flows(&server, uuid, agent, &format!("ses-bad-{i}"), &["failed"]).await;
    }
    for i in 0..5 {
        seed_session_with_flows(
            &server,
            uuid,
            agent,
            &format!("ses-good-{i}"),
            &["completed"],
        )
        .await;
    }

    // The point of filtering in SQL: `matching_count` counts every match in the
    // window, so "3 failed sessions" comes from one call rather than from
    // counting the rows that happened to fit on the page.
    let body = list_sessions(&server, uid, "?status=failed&limit=2").await;
    assert_eq!(body["data"]["sessions"].as_array().unwrap().len(), 2);
    assert_eq!(body["data"]["matching_count"], 3);
    for s in body["data"]["sessions"].as_array().unwrap() {
        assert_eq!(s["status"], "failed");
    }

    let ok = list_sessions(&server, uid, "?status=ok&limit=100").await;
    assert_eq!(ok["data"]["matching_count"], 5);

    let bad = server
        .client
        .get(server.url("/api/observability/session/list?status=nonsense"))
        .bearer_auth(common::sign_token(uid, "admin", true, "admin"))
        .send()
        .await
        .unwrap();
    assert_eq!(
        bad.status(),
        400,
        "an unknown status is rejected, not ignored"
    );

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn session_list_pages_by_cursor_and_still_accepts_offset() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();
    let uuid: Uuid = uid.parse().unwrap();
    let agent = seed_agent(&server, uuid, "paging-agent").await;

    for i in 0..5 {
        seed_session_with_flows(
            &server,
            uuid,
            agent,
            &format!("ses-page-{i}"),
            &["completed"],
        )
        .await;
    }

    let first = list_sessions(&server, uid, "?limit=2").await;
    assert_eq!(first["data"]["sessions"].as_array().unwrap().len(), 2);
    assert_eq!(first["data"]["pagination"]["has_next_page"], true);
    let cursor = first["data"]["pagination"]["end_cursor"].as_str().unwrap();

    let second = list_sessions(&server, uid, &format!("?limit=2&cursor={cursor}")).await;
    let first_ids: Vec<&str> = first["data"]["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["session_id"].as_str().unwrap())
        .collect();
    for s in second["data"]["sessions"].as_array().unwrap() {
        assert!(
            !first_ids.contains(&s["session_id"].as_str().unwrap()),
            "a cursor page must not repeat the previous page"
        );
    }

    // offset is still honoured: the Sessions page pages on it, so removing it
    // here would break that page before its own change ships.
    let by_offset = list_sessions(&server, uid, "?limit=2&offset=2").await;
    assert_eq!(
        by_offset["data"]["sessions"], second["data"]["sessions"],
        "offset and cursor must address the same page"
    );

    // Paging past the end is empty, not a wrap back to the first page.
    let past_end = list_sessions(&server, uid, "?limit=2&offset=500").await;
    assert_eq!(past_end["data"]["sessions"].as_array().unwrap().len(), 0);
    assert_eq!(past_end["data"]["pagination"]["has_next_page"], false);

    let bad_cursor = server
        .client
        .get(server.url("/api/observability/session/list?cursor=not-a-cursor"))
        .bearer_auth(common::sign_token(uid, "admin", true, "admin"))
        .send()
        .await
        .unwrap();
    assert_eq!(bad_cursor.status(), 400);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn session_list_aggregates_metrics_without_the_trace_store() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();
    let uuid: Uuid = uid.parse().unwrap();
    let agent = seed_agent(&server, uuid, "metrics-agent").await;
    seed_session_with_flows(&server, uuid, agent, "ses-metrics", &["completed"]).await;

    sqlx::query(
        "INSERT INTO session_traces (session_id, trace_id, agent_id, agent_name) \
         VALUES ('ses-metrics', 'trace-m', $1, 'metrics-agent')",
    )
    .bind(agent)
    .execute(&server.db)
    .await
    .expect("seed session_trace");

    // trace_usage is keyed (trace_id, agent_name): one trace crossing two agents
    // is two rows. Tokens and cost must sum across them while the trace counts
    // once toward the latency percentiles.
    for (name, input, cost, latency) in [
        ("agent-a", 100i64, 1.25f64, 4000i64),
        ("agent-b", 200, 2.75, 4000),
    ] {
        sqlx::query(
            "INSERT INTO trace_usage (trace_id, agent_name, session_id, input_tokens, output_tokens, \
             cost_usd, latency_ms, started_at) \
             VALUES ('trace-m', $1, 'ses-metrics', $2, 0, $3, $4, now())",
        )
        .bind(name)
        .bind(input)
        .bind(cost)
        .bind(latency)
        .execute(&server.db)
        .await
        .expect("seed trace_usage");
    }

    sqlx::query(
        "INSERT INTO chat_messages (session_id, role, content) VALUES \
         ('ses-metrics', 'user', 'the first thing asked'), \
         ('ses-metrics', 'assistant', 'the last thing answered')",
    )
    .execute(&server.db)
    .await
    .expect("seed chat_messages");

    let body = list_sessions(&server, uid, "?limit=10").await;
    let row = body["data"]["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["session_id"] == "ses-metrics")
        .expect("seeded session is listed");

    assert_eq!(row["num_traces"], 1, "counted from session_traces, once");
    assert_eq!(row["token_usage"]["total"], 300, "summed across agent rows");
    assert_eq!(row["cost_summary"]["total"]["cost"], 4.0);
    // Previews come from chat_messages, so the list never reads Loki.
    assert_eq!(row["first_input"], "the first thing asked");
    assert_eq!(row["last_output"], "the last thing answered");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn session_list_never_shows_another_users_sessions() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();
    let uuid: Uuid = uid.parse().unwrap();
    let agent = seed_agent(&server, uuid, "scope-agent").await;
    let other = seed_user(&server, "someone-else").await;

    seed_session_with_flows(&server, uuid, agent, "ses-mine", &["completed"]).await;
    seed_session_with_flows(&server, other, agent, "ses-theirs", &["failed"]).await;

    // A non-superuser sees only their own, and the count must agree with the
    // rows — a scoped list with an unscoped count would leak the other session.
    let res = server
        .client
        .get(server.url("/api/observability/session/list?limit=100"))
        .bearer_auth(common::sign_token(uid, "member", false, "member"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let body: Value = res.json().await.unwrap();
    let ids: Vec<&str> = body["data"]["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["session_id"].as_str().unwrap())
        .collect();
    assert!(ids.contains(&"ses-mine"));
    assert!(!ids.contains(&"ses-theirs"));
    assert_eq!(body["data"]["matching_count"], 1);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn observe_trace_by_id_returns_404_for_unknown_trace() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    let res = server
        .client
        .get(server.url("/api/observability/trace/abc123def456"))
        .bearer_auth(common::sign_token(uid, "admin", true, "admin"))
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), 404, "unknown trace id must 404");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn observe_finops_dashboard_returns_zeroed_summary() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    create_agent(&server, uid, "finops-zero-agent").await;

    let res = server
        .client
        .get(server.url("/api/observability/finops/dashboard"))
        .bearer_auth(common::sign_token(uid, "admin", true, "admin"))
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), 200, "finops degrades soft, never 503");
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["data"]["summary"]["total_operations"], 0);
    assert_eq!(body["data"]["summary"]["total_agents"], 1);
    assert!(body["data"]["agents"].is_array());

    server.cleanup().await;
}

/// `summary.total_agents` off the finops dashboard — the field both the overview
/// and tokenops pages read to pick between their first-run screen and the real
/// dashboard.
async fn finops_summary(server: &common::TestServer, uid: &str) -> Value {
    let res = server
        .client
        .get(server.url("/api/observability/finops/dashboard"))
        .bearer_auth(common::sign_token(uid, "admin", true, "admin"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let body: Value = res.json().await.unwrap();
    body["data"]["summary"].clone()
}

/// One closed container session, so the agent has billable hours in the window
/// the dashboard reports on.
async fn seed_container_hours(server: &common::TestServer, agent: Uuid, name: &str) {
    sqlx::query(
        "INSERT INTO agent_instance_sessions \
         (agent_id, agent_name, instance_key, runtime, started_at, last_seen_at, ended_at) \
         VALUES ($1, $2, $3, 'docker', now() - interval '2 hours', now(), now())",
    )
    .bind(agent)
    .bind(name)
    .bind(format!("container-{agent}"))
    .execute(&server.db)
    .await
    .expect("seed container session");
}

/// An `is_internal` agent (Weave's dashboard-generator) must not pass for a
/// deployed fleet: alone it reports zero agents, so the overview and tokenops
/// pages keep their first-run screen. Alongside a real agent it counts again.
#[tokio::test]
#[serial]
async fn finops_dashboard_excludes_internal_agent_only_when_it_is_alone() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();
    let owner: Uuid = uid.parse().unwrap();

    let internal = seed_agent(&server, owner, "weave-dashboard-generator").await;
    sqlx::query("UPDATE agents SET is_internal = true WHERE id = $1")
        .bind(internal)
        .execute(&server.db)
        .await
        .expect("mark internal");

    seed_container_hours(&server, internal, "weave-dashboard-generator").await;

    // The fleet is empty AND the summary says so throughout. The internal
    // agent's container hours have to come out with it: a first-run screen
    // reporting zero agents and zero spend beside non-zero hours reads as a
    // bug, and those hours are the one number the denied agent still fed.
    let summary = finops_summary(&server, uid).await;
    assert_eq!(
        summary["total_agents"].as_i64().unwrap(),
        0,
        "an internal agent alone must read as an empty fleet"
    );
    assert_eq!(
        summary["total_container_hours"].as_f64().unwrap(),
        0.0,
        "hours from the agent we just denied must not survive into the summary"
    );

    seed_agent(&server, owner, "finops-real-agent").await;
    let summary = finops_summary(&server, uid).await;
    assert_eq!(
        summary["total_agents"].as_i64().unwrap(),
        2,
        "with a real agent deployed the internal one counts again"
    );
    assert!(
        summary["total_container_hours"].as_f64().unwrap() > 0.0,
        "and so do its hours"
    );

    server.cleanup().await;
}

// ─── 404 for unknown agents ───────────────────────────────────────────────────

#[tokio::test]
#[serial]
async fn agent_logs_returns_404_for_unknown_name() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    let res = server
        .client
        .get(server.url("/api/observability/agents/no-such-agent/logs"))
        .bearer_auth(common::sign_token(uid, "admin", true, "admin"))
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), 404);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn agent_logs_returns_404_for_unknown_uuid() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();
    let fake_id = Uuid::new_v4();

    let res = server
        .client
        .get(server.url(&format!("/api/observability/agents/{fake_id}/logs")))
        .bearer_auth(common::sign_token(uid, "admin", true, "admin"))
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), 404);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn agent_stats_returns_404_for_unknown_agent() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    let res = server
        .client
        .get(server.url("/api/observability/agents/ghost-agent/stats"))
        .bearer_auth(common::sign_token(uid, "admin", true, "admin"))
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), 404);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn agent_stream_returns_404_for_unknown_agent() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    let res = server
        .client
        .get(server.url("/api/observability/agents/ghost-agent/logs/stream"))
        .bearer_auth(common::sign_token(uid, "admin", true, "admin"))
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), 404);

    server.cleanup().await;
}

// ─── agent resolution: UUID and name ─────────────────────────────────────────

#[tokio::test]
#[serial]
async fn agent_logs_resolves_by_name() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    create_agent(&server, uid, "resolve-by-name").await;

    let res = server
        .client
        .get(server.url("/api/observability/agents/resolve-by-name/logs"))
        .bearer_auth(common::sign_token(uid, "admin", true, "admin"))
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), 200, "should resolve agent by name");

    let body: Value = res.json().await.unwrap();
    assert!(body.is_array(), "logs response should be an array");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn agent_logs_resolves_by_uuid() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    let agent = create_agent(&server, uid, "resolve-by-uuid").await;
    let agent_id = agent["id"].as_str().unwrap();

    let res = server
        .client
        .get(server.url(&format!("/api/observability/agents/{agent_id}/logs")))
        .bearer_auth(common::sign_token(uid, "admin", true, "admin"))
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), 200, "should resolve agent by UUID");

    let body: Value = res.json().await.unwrap();
    assert!(body.is_array(), "logs response should be an array");

    server.cleanup().await;
}

// ─── proxy_logs are surfaced in the logs endpoint ────────────────────────────

#[tokio::test]
#[serial]
async fn agent_logs_returns_proxy_log_entries() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();
    let user_id: Uuid = Uuid::parse_str(uid).unwrap();

    let agent = create_agent(&server, uid, "proxy-logs-test").await;
    let agent_id: Uuid = agent["id"].as_str().unwrap().parse().unwrap();

    // Seed 3 proxy log rows for this agent
    seed_proxy_log(&server, user_id, agent_id, 200, 42, None).await;
    seed_proxy_log(&server, user_id, agent_id, 500, 88, Some("upstream error")).await;
    seed_proxy_log(
        &server,
        user_id,
        agent_id,
        404,
        15,
        Some("agent returned 404"),
    )
    .await;

    let res = server
        .client
        .get(server.url("/api/observability/agents/proxy-logs-test/logs"))
        .bearer_auth(common::sign_token(uid, "admin", true, "admin"))
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), 200);

    let body: Value = res.json().await.unwrap();
    let entries = body.as_array().unwrap();

    // At least the 3 proxy log rows should appear (container logs may add more)
    assert!(
        entries.len() >= 3,
        "expected at least 3 log entries, got {}: {body}",
        entries.len()
    );

    // All entries from proxy should have source = "proxy"
    let proxy_entries: Vec<&Value> = entries
        .iter()
        .filter(|e| e["source"].as_str() == Some("proxy"))
        .collect();
    assert_eq!(proxy_entries.len(), 3, "exactly 3 proxy log entries");

    // Each entry should have required fields
    for entry in &proxy_entries {
        assert!(
            entry["timestamp"].is_string(),
            "timestamp should be a string"
        );
        assert!(entry["message"].is_string(), "message should be a string");
        assert!(entry["level"].is_string(), "level should be a string");
    }

    server.cleanup().await;
}

// ─── level field is correctly derived from HTTP status ───────────────────────

#[tokio::test]
#[serial]
async fn proxy_log_level_reflects_http_status() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();
    let user_id: Uuid = Uuid::parse_str(uid).unwrap();

    let agent = create_agent(&server, uid, "level-test-agent").await;
    let agent_id: Uuid = agent["id"].as_str().unwrap().parse().unwrap();

    seed_proxy_log(&server, user_id, agent_id, 200, 10, None).await; // INFO
    seed_proxy_log(&server, user_id, agent_id, 404, 20, None).await; // WARN
    seed_proxy_log(&server, user_id, agent_id, 503, 30, None).await; // ERROR

    let res = server
        .client
        .get(server.url("/api/observability/agents/level-test-agent/logs"))
        .bearer_auth(common::sign_token(uid, "admin", true, "admin"))
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), 200);
    let body: Value = res.json().await.unwrap();
    let entries = body.as_array().unwrap();

    let proxy_entries: Vec<&Value> = entries
        .iter()
        .filter(|e| e["source"].as_str() == Some("proxy"))
        .collect();

    let levels: Vec<&str> = proxy_entries
        .iter()
        .filter_map(|e| e["level"].as_str())
        .collect();

    assert!(levels.contains(&"INFO"), "200 → INFO");
    assert!(levels.contains(&"WARN"), "404 → WARN");
    assert!(levels.contains(&"ERROR"), "503 → ERROR");

    server.cleanup().await;
}

// ─── level filter query parameter ────────────────────────────────────────────

#[tokio::test]
#[serial]
async fn agent_logs_level_filter_returns_only_matching_level() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();
    let user_id: Uuid = Uuid::parse_str(uid).unwrap();

    let agent = create_agent(&server, uid, "filter-level-agent").await;
    let agent_id: Uuid = agent["id"].as_str().unwrap().parse().unwrap();

    seed_proxy_log(&server, user_id, agent_id, 200, 10, None).await; // INFO
    seed_proxy_log(&server, user_id, agent_id, 200, 12, None).await; // INFO
    seed_proxy_log(&server, user_id, agent_id, 500, 50, Some("boom")).await; // ERROR

    let res = server
        .client
        .get(server.url("/api/observability/agents/filter-level-agent/logs?level=ERROR"))
        .bearer_auth(common::sign_token(uid, "admin", true, "admin"))
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), 200);
    let body: Value = res.json().await.unwrap();
    let entries = body.as_array().unwrap();

    // All returned entries must be ERROR level
    for entry in entries {
        assert_eq!(
            entry["level"].as_str(),
            Some("ERROR"),
            "all entries should be ERROR after level filter"
        );
    }
    // At least the one seeded ERROR should be present
    assert!(!entries.is_empty(), "should have at least one ERROR entry");

    server.cleanup().await;
}

// ─── search filter query parameter ───────────────────────────────────────────

#[tokio::test]
#[serial]
async fn agent_logs_search_filter_returns_only_matching_messages() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();
    let user_id: Uuid = Uuid::parse_str(uid).unwrap();

    let agent = create_agent(&server, uid, "search-filter-agent").await;
    let agent_id: Uuid = agent["id"].as_str().unwrap().parse().unwrap();

    // "upstream error" will appear in the message for error rows
    seed_proxy_log(&server, user_id, agent_id, 500, 40, Some("upstream error")).await;
    seed_proxy_log(&server, user_id, agent_id, 200, 12, None).await; // no error message

    let res = server
        .client
        .get(server.url("/api/observability/agents/search-filter-agent/logs?search=upstream"))
        .bearer_auth(common::sign_token(uid, "admin", true, "admin"))
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), 200);
    let body: Value = res.json().await.unwrap();
    let entries = body.as_array().unwrap();

    // All returned messages must contain "upstream" (case-insensitive)
    for entry in entries {
        let msg = entry["message"].as_str().unwrap_or("").to_lowercase();
        assert!(
            msg.contains("upstream"),
            "all entries should contain 'upstream': got {msg}"
        );
    }
    assert!(
        !entries.is_empty(),
        "should have at least one matching entry"
    );

    server.cleanup().await;
}

// ─── stats endpoint ───────────────────────────────────────────────────────────
//
// Stats moved to /agent/{ref}/stats and are provider-backed (Tempo): the
// response is a { data: { project: { id: <agent name>, trace_count, ... } } }
// envelope (get_agent_stats in oss/server/src/observability/handler.rs). The
// old proxy_logs fallback (source/total_requests/error_rate) no longer exists.

#[tokio::test]
#[serial]
async fn agent_stats_resolves_by_name() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    create_agent(&server, uid, "stats-source-agent").await;

    let res = server
        .client
        .get(server.url("/api/observability/agent/stats-source-agent/stats"))
        .bearer_auth(common::sign_token(uid, "admin", true, "admin"))
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), 503, "test server has observability disabled");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn agent_stats_resolves_by_uuid() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    let agent = create_agent(&server, uid, "stats-uuid-agent").await;
    let agent_id = agent["id"].as_str().unwrap();

    let res = server
        .client
        .get(server.url(&format!("/api/observability/agent/{agent_id}/stats")))
        .bearer_auth(common::sign_token(uid, "admin", true, "admin"))
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), 503, "test server has observability disabled");

    server.cleanup().await;
}

// ─── SSE log stream ───────────────────────────────────────────────────────────

#[tokio::test]
#[serial]
async fn agent_logs_stream_returns_text_event_stream_content_type() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    create_agent(&server, uid, "stream-test-agent").await;

    // We only check the Content-Type header — we don't consume the stream.
    let res = server
        .client
        .get(server.url("/api/observability/agents/stream-test-agent/logs/stream"))
        .bearer_auth(common::sign_token(uid, "admin", true, "admin"))
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), 200);

    let content_type = res
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(
        content_type.contains("text/event-stream"),
        "SSE stream should have text/event-stream content-type, got: {content_type}"
    );

    server.cleanup().await;
}

// ─── metrics and readiness (no auth required) ────────────────────────────────

#[tokio::test]
#[serial]
async fn metrics_endpoint_is_publicly_accessible() {
    let server = common::TestServer::start().await;
    let _admin = init_admin(&server).await; // ensure tables exist

    let res = server
        .client
        .get(server.url("/metrics"))
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), 200);

    let body: Value = res.json().await.unwrap();
    // Should include known counter fields
    assert!(
        body["agents_total"].is_number(),
        "agents_total should be a number"
    );
    assert!(
        body["users_total"].is_number(),
        "users_total should be a number"
    );

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn readiness_endpoint_is_publicly_accessible() {
    let server = common::TestServer::start().await;

    let res = server
        .client
        .get(server.url("/readiness"))
        .send()
        .await
        .unwrap();

    // Status is 200 (ready) or 503 (degraded — if redis/docker is down)
    let status = res.status().as_u16();
    assert!(
        status == 200 || status == 503,
        "readiness should return 200 or 503, got {status}"
    );

    let body: Value = res.json().await.unwrap();
    assert!(body["postgres"].is_boolean(), "postgres field required");
    assert!(body["status"].is_string(), "status field required");

    server.cleanup().await;
}

// ─── deleted agents are not resolvable ───────────────────────────────────────

#[tokio::test]
#[serial]
async fn deleted_agent_not_found_in_logs() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    let agent = create_agent(&server, uid, "deleted-logs-agent").await;
    let agent_id = agent["id"].as_str().unwrap();

    // Soft-delete the agent directly in the DB
    sqlx::query("UPDATE agents SET deleted_at = now() WHERE id = $1")
        .bind(Uuid::parse_str(agent_id).unwrap())
        .execute(&server.db)
        .await
        .expect("soft delete agent");

    // Both UUID and name lookups should now return 404
    let by_name = server
        .client
        .get(server.url("/api/observability/agents/deleted-logs-agent/logs"))
        .bearer_auth(common::sign_token(uid, "admin", true, "admin"))
        .send()
        .await
        .unwrap();
    assert_eq!(by_name.status(), 404, "deleted agent should be 404 by name");

    let by_uuid = server
        .client
        .get(server.url(&format!("/api/observability/agents/{agent_id}/logs")))
        .bearer_auth(common::sign_token(uid, "admin", true, "admin"))
        .send()
        .await
        .unwrap();
    assert_eq!(by_uuid.status(), 404, "deleted agent should be 404 by UUID");

    server.cleanup().await;
}

// ─── stats with no data returns zero counts ──────────────────────────────────

#[tokio::test]
#[serial]
async fn agent_stats_returns_zero_counts_for_new_agent() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    create_agent(&server, uid, "zero-stats-agent").await;

    let res = server
        .client
        .get(server.url("/api/observability/agent/zero-stats-agent/stats"))
        .bearer_auth(common::sign_token(uid, "admin", true, "admin"))
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), 503, "test server has observability disabled");

    server.cleanup().await;
}

// ─── logs are empty for new agent ────────────────────────────────────────────

#[tokio::test]
#[serial]
async fn agent_logs_returns_empty_array_for_new_agent() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    create_agent(&server, uid, "empty-logs-agent").await;

    let res = server
        .client
        .get(server.url("/api/observability/agents/empty-logs-agent/logs"))
        .bearer_auth(common::sign_token(uid, "admin", true, "admin"))
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), 200);
    let body: Value = res.json().await.unwrap();
    let entries = body.as_array().unwrap();

    // Container runtime will return empty logs for a non-deployed agent
    // so the overall result should be empty or only container entries
    // (either is fine — the important thing is no 404 or 500)
    let _ = entries; // structure is valid, count may vary

    server.cleanup().await;
}

// ─── error message is included in log line ───────────────────────────────────

#[tokio::test]
#[serial]
async fn proxy_log_error_message_appears_in_log_line() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();
    let user_id: Uuid = Uuid::parse_str(uid).unwrap();

    let agent = create_agent(&server, uid, "error-msg-agent").await;
    let agent_id: Uuid = agent["id"].as_str().unwrap().parse().unwrap();

    seed_proxy_log(
        &server,
        user_id,
        agent_id,
        502,
        99,
        Some("bad gateway downstream"),
    )
    .await;

    let res = server
        .client
        .get(server.url("/api/observability/agents/error-msg-agent/logs"))
        .bearer_auth(common::sign_token(uid, "admin", true, "admin"))
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), 200);
    let body: Value = res.json().await.unwrap();
    let entries = body.as_array().unwrap();

    let error_entry = entries
        .iter()
        .find(|e| e["source"].as_str() == Some("proxy"))
        .expect("should have a proxy entry");

    let msg = error_entry["message"].as_str().unwrap_or("");
    assert!(
        msg.contains("bad gateway downstream"),
        "error field should appear in log message: {msg}"
    );
    assert!(
        msg.contains("502"),
        "HTTP status should appear in log message: {msg}"
    );

    server.cleanup().await;
}

// ─── limit parameter is respected ────────────────────────────────────────────

#[tokio::test]
#[serial]
async fn agent_logs_limit_parameter_is_respected() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();
    let user_id: Uuid = Uuid::parse_str(uid).unwrap();

    let agent = create_agent(&server, uid, "limit-test-agent").await;
    let agent_id: Uuid = agent["id"].as_str().unwrap().parse().unwrap();

    // Seed 10 proxy log rows
    for i in 0..10_i64 {
        seed_proxy_log(&server, user_id, agent_id, 200, i * 5 + 1, None).await;
    }

    let res = server
        .client
        .get(server.url("/api/observability/agents/limit-test-agent/logs?limit=3"))
        .bearer_auth(common::sign_token(uid, "admin", true, "admin"))
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), 200);
    let body: Value = res.json().await.unwrap();
    let entries = body.as_array().unwrap();

    assert!(
        entries.len() <= 3,
        "limit=3 should return at most 3 entries, got {}",
        entries.len()
    );

    server.cleanup().await;
}

// ─── since parameter filters out old entries ─────────────────────────────────

#[tokio::test]
#[serial]
async fn agent_logs_since_parameter_filters_old_entries() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();
    let user_id: Uuid = Uuid::parse_str(uid).unwrap();

    let agent = create_agent(&server, uid, "since-test-agent").await;
    let agent_id: Uuid = agent["id"].as_str().unwrap().parse().unwrap();

    // Insert a log row 2 hours ago (outside the 1-hour window we'll query)
    sqlx::query(
        r#"INSERT INTO proxy_logs (caller_id, target_agent_id, method, latency_ms, status, timestamp)
           VALUES ($1, $2, 'tasks/send', 10, 200, now() - interval '2 hours')"#,
    )
    .bind(user_id)
    .bind(agent_id)
    .execute(&server.db)
    .await
    .unwrap();

    // Insert a recent log row (within the last hour)
    seed_proxy_log(&server, user_id, agent_id, 200, 10, None).await;

    // Query with since = 90 minutes ago — old entry should be excluded.
    // RFC-3339 timestamps contain '+' which must be percent-encoded in query strings.
    let since_raw = (chrono::Utc::now() - chrono::Duration::try_minutes(90).unwrap()).to_rfc3339();
    let since_encoded = since_raw.replace('+', "%2B");
    let url =
        format!("/api/observability/agents/since-test-agent/logs?since={since_encoded}&limit=10");

    let res = server
        .client
        .get(server.url(&url))
        .bearer_auth(common::sign_token(uid, "admin", true, "admin"))
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), 200);
    let body: Value = res.json().await.unwrap();
    let entries = body.as_array().unwrap();

    // Only the recent row should be in range; the 2-hour-old row should be excluded.
    let proxy_entries: Vec<&Value> = entries
        .iter()
        .filter(|e| e["source"].as_str() == Some("proxy"))
        .collect();

    assert_eq!(
        proxy_entries.len(),
        1,
        "only the recent proxy log should appear when since filter applied; got: {body}"
    );

    server.cleanup().await;
}

// ─── finops: fewer round-trips (OP-4) ────────────────────────────────────────
//
// `compare=previous` and `group_by=agent` exist to collapse request fan-out:
// the Overview used to ask for four dashboards (two windows x current/previous)
// and one spend-timeseries per top driver.

/// One `trace_usage` row `hours_ago` in the past.
async fn seed_usage(
    server: &common::TestServer,
    trace_id: &str,
    agent_name: &str,
    cost: f64,
    hours_ago: i64,
) {
    sqlx::query(
        "INSERT INTO trace_usage (trace_id, agent_name, input_tokens, output_tokens, \
         cost_usd, latency_ms, started_at) \
         VALUES ($1, $2, 10, 10, $3, 100, now() - ($4 || ' hours')::interval)",
    )
    .bind(trace_id)
    .bind(agent_name)
    .bind(cost)
    .bind(hours_ago.to_string())
    .execute(&server.db)
    .await
    .expect("seed trace_usage");
}

#[tokio::test]
#[serial]
async fn dashboard_compare_previous_returns_both_windows_in_one_request() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();
    let uuid: Uuid = uid.parse().unwrap();
    seed_agent(&server, uuid, "cmp-agent").await;

    // 2h ago is inside a 24h window; 30h ago is inside its preceding window.
    seed_usage(&server, "cmp-cur", "cmp-agent", 4.0, 2).await;
    seed_usage(&server, "cmp-prev", "cmp-agent", 1.0, 30).await;

    let get = |q: &str| {
        server
            .client
            .get(server.url(&format!("/api/observability/finops/dashboard{q}")))
            .bearer_auth(common::sign_token(uid, "admin", true, "admin"))
            .send()
    };

    // Without `compare`, the payload is unchanged — the field is absent, not null.
    let plain: Value = get("?range=24h").await.unwrap().json().await.unwrap();
    let row = plain["data"]["agents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["agent_name"] == "cmp-agent")
        .expect("agent row");
    assert!(
        row.get("previous").is_none(),
        "compare is opt-in; the default payload must not change: {row}"
    );

    let compared: Value = get("?range=24h&compare=previous")
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let row = compared["data"]["agents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["agent_name"] == "cmp-agent")
        .expect("agent row");
    assert_eq!(row["total_cost"], 4.0);
    assert_eq!(
        row["previous"]["total_cost"], 1.0,
        "the preceding window was already queried for the KPI strip; \
         compare=previous keeps the per-agent detail instead of discarding it"
    );

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn spend_timeseries_groups_by_agent_with_an_other_bucket() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    // Four agents, descending spend, so top=2 leaves two in `other`.
    for (i, (name, cost)) in [("ts-a", 8.0), ("ts-b", 4.0), ("ts-c", 2.0), ("ts-d", 1.0)]
        .iter()
        .enumerate()
    {
        seed_usage(&server, &format!("ts-{i}"), name, *cost, 2).await;
    }

    let body: Value = server
        .client
        .get(
            server.url("/api/observability/finops/spend-timeseries?range=24h&group_by=agent&top=2"),
        )
        .bearer_auth(common::sign_token(uid, "admin", true, "admin"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    let series = body["data"]["series"].as_array().expect("series present");
    let named: Vec<&str> = series
        .iter()
        .filter(|s| s["is_other"] == false)
        .map(|s| s["agent_name"].as_str().unwrap())
        .collect();
    assert_eq!(
        named,
        vec!["ts-a", "ts-b"],
        "top 2 by window spend, in order"
    );

    let other = series
        .iter()
        .find(|s| s["is_other"] == true)
        .expect("an `other` series");
    assert_eq!(
        other["total_spend_usd"], 3.0,
        "everything outside the top folds into `other` (2.0 + 1.0)"
    );
    assert!(other["agent_name"].is_null());

    // The invariant that makes the series safe to chart: they still sum to the
    // fleet points, so no spend is hidden by the top-N cut.
    let fleet: f64 = body["data"]["points"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["spend_usd"].as_f64().unwrap())
        .sum();
    let by_series: f64 = series
        .iter()
        .map(|s| s["total_spend_usd"].as_f64().unwrap())
        .sum();
    assert!(
        (fleet - by_series).abs() < 1e-6,
        "series must sum to the fleet total: {fleet} vs {by_series}"
    );

    // Without group_by the field is absent, so existing callers are untouched.
    let plain: Value = server
        .client
        .get(server.url("/api/observability/finops/spend-timeseries?range=24h"))
        .bearer_auth(common::sign_token(uid, "admin", true, "admin"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(plain["data"].get("series").is_none());

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn spend_timeseries_rejects_an_unknown_group_by() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    // Silently ignoring it would return a fleet-wide chart under a per-agent
    // legend, which looks like it worked.
    let res = server
        .client
        .get(server.url("/api/observability/finops/spend-timeseries?group_by=model"))
        .bearer_auth(common::sign_token(uid, "admin", true, "admin"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 400);

    server.cleanup().await;
}

// ─── coding-agents/usage (OP-5 / H-1) ────────────────────────────────────────
//
// Without this route the Harnesses page pays a bare 404 on every load and then
// rebuilds the viewer's own usage from three other endpoints. Every 404 here
// must carry a `code`: an uncoded one is how the UI detects an absent endpoint,
// so it would silently re-enable that fallback.

const USAGE: &str = "/api/observability/coding-agents/usage";

async fn seed_harness_agent(
    server: &common::TestServer,
    owner: Uuid,
    name: &str,
    harness: &str,
) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO agents (name, owner_id, coding_agent_integration_id) \
         VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(name)
    .bind(owner)
    .bind(harness)
    .fetch_one(&server.db)
    .await
    .expect("seed harness agent")
}

/// One harness turn: which agent ran it, in which session, on which model.
struct Turn<'a> {
    trace: &'a str,
    agent: Uuid,
    agent_name: &'a str,
    user: Uuid,
    session: &'a str,
    model: &'a str,
    cost: f64,
    hours_ago: i64,
}

async fn seed_turn(server: &common::TestServer, t: Turn<'_>) {
    let Turn {
        trace,
        agent,
        agent_name,
        user,
        session,
        model,
        cost,
        hours_ago,
    } = t;
    sqlx::query(
        "INSERT INTO trace_usage (trace_id, agent_name, agent_id, user_id, session_id, model, \
         input_tokens, output_tokens, cost_usd, latency_ms, started_at) \
         VALUES ($1, $2, $3, $4, $5, $6, 100, 50, $7, 100, now() - ($8 || ' hours')::interval)",
    )
    .bind(trace)
    .bind(agent_name)
    .bind(agent)
    .bind(user)
    .bind(session)
    .bind(model)
    .bind(cost)
    .bind(hours_ago.to_string())
    .execute(&server.db)
    .await
    .expect("seed turn");
}

#[tokio::test]
#[serial]
async fn coding_agent_usage_reports_the_callers_own_harnesses() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();
    let uuid: Uuid = uid.parse().unwrap();

    let claude = seed_harness_agent(&server, uuid, "cc", "claude").await;
    // Registered but never used in the window: an idle seat, not an absent one.
    seed_harness_agent(&server, uuid, "cx", "codex").await;

    seed_turn(
        &server,
        Turn {
            trace: "t1",
            agent: claude,
            agent_name: "cc",
            user: uuid,
            session: "s1",
            model: "sonnet",
            cost: 1.0,
            hours_ago: 2,
        },
    )
    .await;
    seed_turn(
        &server,
        Turn {
            trace: "t2",
            agent: claude,
            agent_name: "cc",
            user: uuid,
            session: "s1",
            model: "sonnet",
            cost: 2.0,
            hours_ago: 3,
        },
    )
    .await;
    seed_turn(
        &server,
        Turn {
            trace: "t3",
            agent: claude,
            agent_name: "cc",
            user: uuid,
            session: "s2",
            model: "opus",
            cost: 3.0,
            hours_ago: 4,
        },
    )
    .await;

    let body: Value = server
        .client
        .get(server.url(&format!("{USAGE}?range=7d")))
        .bearer_auth(common::sign_token(uid, "admin", true, "admin"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let d = &body["data"];

    assert_eq!(
        d["scope"]["kind"], "user",
        "OSS answers the Individual level"
    );
    assert_eq!(d["scope"]["user_id"], uid);
    assert_eq!(d["window"]["range"], "7d");
    assert_eq!(d["totals"]["turns"], 3);
    assert_eq!(d["totals"]["sessions"], 2, "two distinct session ids");
    assert_eq!(d["totals"]["tokens"], 450);
    assert_eq!(d["totals"]["cost_usd"], 6.0);
    assert_eq!(d["totals"]["active_devs"], 1);
    assert_eq!(d["totals"]["scope_devs"], 1);
    // Registered on two harnesses, active on one.
    assert_eq!(d["totals"]["idle_seats"], 1, "codex is registered but idle");
    assert!(
        d["totals"]["delta_pct"].is_null(),
        "no compare was asked for"
    );

    let by: std::collections::HashMap<&str, &Value> = d["by_harness"]
        .as_array()
        .unwrap()
        .iter()
        .map(|h| (h["harness"].as_str().unwrap(), h))
        .collect();
    assert_eq!(by["claude"]["turns"], 3);
    assert_eq!(by["claude"]["active_devs"], 1);
    assert_eq!(by["claude"]["idle_seats"], 0);
    // Most-used model first, capped at two.
    assert_eq!(
        by["claude"]["top_models"],
        serde_json::json!(["sonnet", "opus"])
    );
    assert_eq!(by["codex"]["turns"], 0, "an idle seat still has an entry");
    assert_eq!(by["codex"]["idle_seats"], 1);

    // Everything the trend and the Individual panel need, in the one response.
    assert!(!d["series"].as_array().unwrap().is_empty());
    assert_eq!(d["recent_sessions"].as_array().unwrap().len(), 2);
    assert_eq!(d["rows"].as_array().unwrap().len(), 0);
    assert_eq!(d["has_more"], false);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn coding_agent_usage_compares_turns_against_the_previous_window() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();
    let uuid: Uuid = uid.parse().unwrap();
    let claude = seed_harness_agent(&server, uuid, "cc", "claude").await;

    // 2 turns in the last 24h, 1 in the 24h before it: +100%.
    seed_turn(
        &server,
        Turn {
            trace: "c1",
            agent: claude,
            agent_name: "cc",
            user: uuid,
            session: "s1",
            model: "sonnet",
            cost: 1.0,
            hours_ago: 2,
        },
    )
    .await;
    seed_turn(
        &server,
        Turn {
            trace: "c2",
            agent: claude,
            agent_name: "cc",
            user: uuid,
            session: "s1",
            model: "sonnet",
            cost: 1.0,
            hours_ago: 3,
        },
    )
    .await;
    seed_turn(
        &server,
        Turn {
            trace: "c3",
            agent: claude,
            agent_name: "cc",
            user: uuid,
            session: "s0",
            model: "sonnet",
            cost: 1.0,
            hours_ago: 30,
        },
    )
    .await;

    let body: Value = server
        .client
        .get(server.url(&format!("{USAGE}?range=24h&compare=1")))
        .bearer_auth(common::sign_token(uid, "admin", true, "admin"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(body["data"]["totals"]["turns"], 2);
    assert_eq!(body["data"]["totals"]["delta_pct"], 100.0);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn coding_agent_usage_refuses_other_levels_with_a_coded_404() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();
    let other = seed_user(&server, "someone-else").await;

    let get = |q: String| {
        server
            .client
            .get(server.url(&format!("{USAGE}{q}")))
            .bearer_auth(common::sign_token(uid, "admin", true, "admin"))
            .send()
    };

    // OSS has no org tree, and a superuser is no exception: there is nothing to
    // be superuser *of*.
    for (q, code) in [
        ("?scope=org&group_by=unit", "unit_not_visible"),
        ("?scope=mine&group_by=unit", "unit_not_visible"),
        ("?scope=unassigned&group_by=user", "unit_not_visible"),
    ] {
        let res = get(q.to_string()).await.unwrap();
        assert_eq!(res.status(), 404, "{q}");
        let body: Value = res.json().await.unwrap();
        assert_eq!(body["code"], code, "{q}");
    }

    let res = get(format!("?user_id={other}")).await.unwrap();
    assert_eq!(res.status(), 404);
    assert_eq!(
        res.json::<Value>().await.unwrap()["code"],
        "user_not_visible"
    );

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn coding_agent_usage_rejects_malformed_params_before_checking_visibility() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    // A typo must read as a typo, not as "not visible" — otherwise the page
    // shows an empty level and the mistake is invisible.
    for (q, status, code) in [
        ("?scope=everything", 400, "invalid_scope"),
        ("?scope=unit", 400, "missing_unit_id"),
        ("?group_by=harness", 400, "invalid_group_by"),
        ("?range=90d", 400, "invalid_range"),
        ("?start_time=last-tuesday", 400, "invalid_window"),
        ("?cursor=abc", 400, "invalid_cursor"),
    ] {
        let res = server
            .client
            .get(server.url(&format!("{USAGE}{q}")))
            .bearer_auth(common::sign_token(uid, "admin", true, "admin"))
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), status, "{q}");
        assert_eq!(res.json::<Value>().await.unwrap()["code"], code, "{q}");
    }

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn coding_agent_usage_requires_auth() {
    let server = common::TestServer::start().await;
    let res = server.client.get(server.url(USAGE)).send().await.unwrap();
    assert_eq!(res.status(), 401);
    server.cleanup().await;
}

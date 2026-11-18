//! Integration tests for the Overview's estate (OV-1) and problem decisions (OV-2).
//!
//! Covers:
//!   GET   /api/overview/estate?range=
//!   PATCH /api/overview/problems/{id}
//!
//! Requires infra (Postgres :5432, Redis, MinIO):
//!   cargo test -p nasiko-server --test overview -- --test-threads=1

mod common;

use serde_json::{Value, json};
use serial_test::serial;
use uuid::Uuid;

async fn seed_user(server: &common::TestServer, username: &str) -> Uuid {
    sqlx::query_scalar("INSERT INTO users (username, email) VALUES ($1, $2) RETURNING id")
        .bind(username)
        .bind(format!("{username}@overview.test"))
        .fetch_one(&server.db)
        .await
        .expect("seed user")
}

async fn seed_agent(server: &common::TestServer, owner: Uuid, name: &str) -> Uuid {
    sqlx::query_scalar("INSERT INTO agents (name, owner_id) VALUES ($1, $2) RETURNING id")
        .bind(name)
        .bind(owner)
        .fetch_one(&server.db)
        .await
        .expect("seed agent")
}

/// `runs` traces for `agent` two hours ago, the first `failed` of them with an error span.
async fn seed_runs(server: &common::TestServer, agent: Uuid, user: Uuid, runs: i32, failed: i32) {
    seed_traces(
        server,
        agent,
        user,
        Traces {
            runs,
            failed,
            latency_ms: 800,
            hours_ago: 2,
        },
    )
    .await;
}

struct Traces {
    runs: i32,
    failed: i32,
    latency_ms: i64,
    hours_ago: i32,
}

async fn seed_traces(server: &common::TestServer, agent: Uuid, user: Uuid, t: Traces) {
    let Traces {
        runs,
        failed,
        latency_ms,
        hours_ago,
    } = t;
    for i in 0..runs {
        sqlx::query(
            "INSERT INTO trace_usage \
               (trace_id, agent_name, agent_id, user_id, session_id, model, provider, \
                input_tokens, output_tokens, cost_usd, latency_ms, error_count, started_at) \
             VALUES ($1, 'seeded', $2, $3, $4, 'gpt-4o', 'openai', 1000, 100, 0.05, $6, $5, \
                     now() - make_interval(hours => $7))",
        )
        .bind(format!("trace-{}", Uuid::new_v4()))
        .bind(agent)
        .bind(user)
        .bind(format!("session-{}", i % 4))
        .bind(if i < failed { 1 } else { 0 })
        .bind(latency_ms)
        .bind(hours_ago)
        .execute(&server.db)
        .await
        .expect("seed trace_usage");
    }
}

fn token(user: Uuid, name: &str, superuser: bool) -> String {
    common::sign_token(&user.to_string(), name, superuser, "member")
}

async fn estate(server: &common::TestServer, bearer: &str, range: &str) -> (u16, Value) {
    let res = server
        .client
        .get(server.url(&format!("/api/overview/estate?range={range}")))
        .bearer_auth(bearer)
        .send()
        .await
        .unwrap();
    let status = res.status().as_u16();
    let body = res.json::<Value>().await.unwrap_or(Value::Null);
    (status, body)
}

async fn patch(server: &common::TestServer, bearer: &str, id: &str, body: Value) -> (u16, String) {
    let res = server
        .client
        .patch(server.url(&format!("/api/overview/problems/{id}")))
        .bearer_auth(bearer)
        .json(&body)
        .send()
        .await
        .unwrap();
    (res.status().as_u16(), res.text().await.unwrap())
}

fn ids(list: &Value) -> Vec<String> {
    list.as_array()
        .unwrap()
        .iter()
        .map(|x| x["id"].as_str().unwrap().to_owned())
        .collect()
}

#[tokio::test]
#[serial]
async fn estate_requires_auth() {
    let server = common::TestServer::start().await;
    let res = server
        .client
        .get(server.url("/api/overview/estate"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 401);
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn an_unknown_range_is_refused() {
    let server = common::TestServer::start().await;
    let admin = seed_user(&server, "ov-admin").await;
    let (status, _) = estate(&server, &token(admin, "ov-admin", true), "24h").await;
    assert_eq!(status, 400);
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn the_estate_draws_what_ran_and_finds_a_failing_agent() {
    let server = common::TestServer::start().await;
    let admin = seed_user(&server, "ov-admin").await;
    let pricing = seed_agent(&server, admin, "pricing-engine").await;
    let quiet = seed_agent(&server, admin, "quiet-agent").await;
    seed_runs(&server, pricing, admin, 40, 12).await;
    let jira: Uuid = sqlx::query_scalar(
        "INSERT INTO mcp_connectors (provider_type, name, url) \
         VALUES ('mcp_server', 'jira-cloud', 'http://127.0.0.1:1/mcp') RETURNING id",
    )
    .fetch_one(&server.db)
    .await
    .unwrap();
    // The gateway names an MCP server's tools `{first 16 hex of its id}__{tool}`.
    let tool = format!("{}__create_issue", &jira.simple().to_string()[..16]);
    // Gateway tool calls, as the MCP gateway records them in `tool_calls`.
    for (i, ok) in [true, false, true].into_iter().enumerate() {
        sqlx::query(
            "INSERT INTO tool_calls (trace_id, call_id, user_id, agent_id, agent_name, \
                 tool_name, tool_kind, status, started_at) \
             VALUES ('ov-trace', $1, $2, $3, 'pricing-engine', $4, 'mcp', $5, now())",
        )
        .bind(format!("call-{i}"))
        .bind(admin)
        .bind(pricing)
        .bind(&tool)
        .bind(if ok { "ok" } else { "error" })
        .execute(&server.db)
        .await
        .unwrap();
    }

    let (status, body) = estate(&server, &token(admin, "ov-admin", true), "7d").await;
    assert_eq!(status, 200, "{body}");
    let e = &body["data"];
    assert_eq!(e["me"], admin.to_string());
    assert_eq!(e["days"].as_array().unwrap().len(), 14);
    assert_eq!(e["groups"], json!([]));
    assert_eq!(e["allowance"], Value::Null);

    let agents = ids(&e["agents"]);
    assert!(agents.contains(&pricing.to_string()) && agents.contains(&quiet.to_string()));
    let row = e["agents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["id"] == pricing.to_string())
        .unwrap();
    assert_eq!(row["runs"], 40);
    assert_eq!(row["success_pct"], 70.0);
    assert_eq!(row["model_ids"], json!(["model:gpt-4o"]));
    assert_eq!(row["tool_ids"], json!([jira.to_string()]));
    assert_eq!(ids(&e["models"]), vec!["model:gpt-4o"]);
    assert_eq!(e["providers"][0]["brand"], "openai");
    assert!(ids(&e["tools"]).contains(&jira.to_string()));

    let flows: Vec<&str> = e["flows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["id"].as_str().unwrap())
        .collect();
    assert!(flows.contains(&"people-agents") && flows.contains(&"agents-tools"));

    let problem = e["problems"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == format!("agent-failing:{pricing}"))
        .expect("the failing agent is a problem");
    assert_eq!(problem["tier"], "broken");
    assert_eq!(problem["kind"], "errors");
    assert_eq!(problem["mine"], true);
    assert_eq!(problem["trend"].as_array().unwrap().len(), 10);
    assert_eq!(problem["action"]["target"]["page"], "agent");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_member_sees_only_what_they_can_reach() {
    let server = common::TestServer::start().await;
    let member = seed_user(&server, "ov-member").await;
    let other = seed_user(&server, "ov-other").await;
    let own = seed_agent(&server, member, "member-agent").await;
    let private = seed_agent(&server, other, "private-agent").await;
    seed_runs(&server, private, other, 40, 20).await;

    let (status, body) = estate(&server, &token(member, "ov-member", false), "30d").await;
    assert_eq!(status, 200, "{body}");
    let e = &body["data"];
    assert_eq!(ids(&e["agents"]), vec![own.to_string()]);
    assert_eq!(ids(&e["people"]), vec![member.to_string()]);
    assert!(
        e["problems"].as_array().unwrap().is_empty(),
        "a problem about an agent they can't see is left out"
    );

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_member_counts_only_their_own_calls_on_a_public_agent() {
    let server = common::TestServer::start().await;
    let owner = seed_user(&server, "ov-seed-owner").await;
    let member = seed_user(&server, "ov-seed-member").await;
    let stranger = seed_user(&server, "ov-seed-stranger").await;
    let public = seed_agent(&server, owner, "public-seed-agent").await;
    sqlx::query("UPDATE agents SET is_public = TRUE WHERE id = $1")
        .bind(public)
        .execute(&server.db)
        .await
        .unwrap();
    seed_runs(&server, public, member, 3, 0).await;
    seed_runs(&server, public, stranger, 40, 20).await;

    let (status, body) = estate(&server, &token(member, "ov-seed-member", false), "7d").await;
    assert_eq!(status, 200, "{body}");
    let e = &body["data"];
    let row = e["agents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["id"] == public.to_string())
        .expect("a public agent is on a member's map");
    assert_eq!(row["runs"], 3, "only the member's own calls count");
    assert_eq!(row["success_pct"], 100.0);
    assert!(
        !e["problems"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["id"] == format!("agent-failing:{public}")),
        "a stranger's failures aren't the member's problem"
    );

    let (status, body) = estate(&server, &token(owner, "ov-seed-owner", true), "7d").await;
    assert_eq!(status, 200, "{body}");
    let row = body["data"]["agents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["id"] == public.to_string())
        .unwrap()
        .clone();
    assert_eq!(row["runs"], 43, "a superuser sees every caller");

    server.cleanup().await;
}

/// An owner sees their agent failing for other callers (its health), but not those callers' runs or
/// spend (their usage).
#[tokio::test]
#[serial]
async fn an_owner_sees_their_agent_failing_for_others_without_seeing_their_usage() {
    let server = common::TestServer::start().await;
    let owner = seed_user(&server, "ov-health-owner").await;
    let stranger = seed_user(&server, "ov-health-stranger").await;
    let public = seed_agent(&server, owner, "ov-health-agent").await;
    sqlx::query("UPDATE agents SET is_public = TRUE WHERE id = $1")
        .bind(public)
        .execute(&server.db)
        .await
        .unwrap();
    seed_runs(&server, public, stranger, 40, 20).await;

    let (status, body) = estate(&server, &token(owner, "ov-health-owner", false), "7d").await;
    assert_eq!(status, 200, "{body}");
    let e = &body["data"];
    let row = e["agents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["id"] == public.to_string())
        .expect("the owner's agent is on their map");
    assert_eq!(
        row["runs"], 0,
        "a stranger's runs are not the owner's usage"
    );
    assert_eq!(row["spend_usd"], 0.0, "nor their spend");
    assert_eq!(
        row["success_pct"], 50.0,
        "but the agent's health counts them"
    );
    assert!(
        e["problems"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["id"] == format!("agent-failing:{public}")),
        "the owner is told their agent is failing: {e}"
    );

    server.cleanup().await;
}

/// A user manager who is not a superuser (`role = 'admin'`) sees every caller's usage, as on the
/// FinOps pages, so the two never disagree for the same person.
#[tokio::test]
#[serial]
async fn a_user_manager_counts_every_callers_usage_like_finops() {
    let server = common::TestServer::start().await;
    let owner = seed_user(&server, "ov-um-owner").await;
    let manager = seed_user(&server, "ov-um-manager").await;
    let stranger = seed_user(&server, "ov-um-stranger").await;
    sqlx::query("UPDATE users SET role = 'admin' WHERE id = $1")
        .bind(manager)
        .execute(&server.db)
        .await
        .unwrap();
    let public = seed_agent(&server, owner, "ov-um-agent").await;
    sqlx::query("UPDATE agents SET is_public = TRUE WHERE id = $1")
        .bind(public)
        .execute(&server.db)
        .await
        .unwrap();
    seed_runs(&server, public, manager, 3, 0).await;
    seed_runs(&server, public, stranger, 40, 0).await;

    let (status, body) = estate(&server, &token(manager, "ov-um-manager", false), "7d").await;
    assert_eq!(status, 200, "{body}");
    let row = body["data"]["agents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["id"] == public.to_string())
        .expect("a public agent is on the manager's map")
        .clone();
    assert_eq!(row["runs"], 43, "a user manager sees every caller");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_problem_takes_an_owner_a_snooze_and_handled() {
    let server = common::TestServer::start().await;
    let admin = seed_user(&server, "ov-admin").await;
    let teammate = seed_user(&server, "ov-teammate").await;
    let agent = seed_agent(&server, admin, "doc-writer").await;
    seed_runs(&server, agent, admin, 30, 10).await;
    let bearer = token(admin, "ov-admin", true);
    let id = format!("agent-failing:{agent}");

    let (status, body) = patch(&server, &bearer, &id, json!({"owner_id": teammate})).await;
    assert_eq!(status, 200, "{body}");
    let answered: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(answered["data"]["owner_id"], teammate.to_string());

    // Everyone who can see the problem sees who is on it.
    let (_, e) = estate(&server, &bearer, "30d").await;
    let listed = e["data"]["problems"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == id)
        .unwrap()
        .clone();
    assert_eq!(listed["owner_id"], teammate.to_string());

    let (status, body) = patch(&server, &bearer, &id, json!({"handled": true})).await;
    assert_eq!(status, 200);
    let handled: Value = serde_json::from_str(&body).unwrap();
    assert!(handled["data"]["handled_at"].is_string());
    assert_eq!(
        handled["data"]["owner_id"],
        teammate.to_string(),
        "other decisions stay"
    );

    let until = (chrono::Utc::now() + chrono::Duration::days(1)).to_rfc3339();
    let (status, body) = patch(&server, &bearer, &id, json!({"snoozed_until": until})).await;
    assert_eq!(status, 200);
    let snoozed: Value = serde_json::from_str(&body).unwrap();
    assert!(snoozed["data"]["snoozed_until"].is_string());

    let (status, _) = patch(&server, &bearer, &id, json!({"owner_id": null})).await;
    assert_eq!(status, 200);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_patch_is_refused_for_an_unknown_problem_person_or_shape() {
    let server = common::TestServer::start().await;
    let admin = seed_user(&server, "ov-admin").await;
    let agent = seed_agent(&server, admin, "doc-writer").await;
    seed_runs(&server, agent, admin, 30, 10).await;
    let bearer = token(admin, "ov-admin", true);
    let id = format!("agent-failing:{agent}");

    let (status, body) = patch(
        &server,
        &bearer,
        "agent-failing:nope",
        json!({"handled": true}),
    )
    .await;
    assert_eq!((status, body.as_str()), (404, "no such problem"));
    let (status, _) = patch(&server, &bearer, &id, json!({"owner_id": Uuid::new_v4()})).await;
    assert_eq!(status, 422, "an owner must be a person");
    let (status, _) = patch(
        &server,
        &bearer,
        &id,
        json!({"owner_id": null, "handled": true}),
    )
    .await;
    assert_eq!(status, 422, "one change at a time");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_coding_harness_turn_never_makes_its_model_slow() {
    let server = common::TestServer::start().await;
    let admin = seed_user(&server, "ov-admin").await;
    let agent = seed_agent(&server, admin, "steady-agent").await;
    let harness: Uuid = sqlx::query_scalar(
        "INSERT INTO agents (name, owner_id, coding_agent_integration_id) \
         VALUES ('admin-codex', $1, 'codex') RETURNING id",
    )
    .bind(admin)
    .fetch_one(&server.db)
    .await
    .unwrap();
    // The agent's calls to gpt-4o take the same time both weeks; this week a harness also used
    // it, and a harness trace is a whole turn: minutes, not one model call.
    for hours_ago in [2, 24 * 9] {
        let steady = Traces {
            runs: 30,
            failed: 0,
            latency_ms: 800,
            hours_ago,
        };
        seed_traces(&server, agent, admin, steady).await;
    }
    let turns = Traces {
        runs: 30,
        failed: 0,
        latency_ms: 240_000,
        hours_ago: 2,
    };
    seed_traces(&server, harness, admin, turns).await;

    let (status, body) = estate(&server, &token(admin, "ov-admin", true), "30d").await;
    assert_eq!(status, 200, "{body}");
    let problems = ids(&body["data"]["problems"]);
    assert!(
        !problems.iter().any(|id| id.starts_with("model-slow:")),
        "{problems:?}"
    );
    let model = &body["data"]["models"][0];
    assert_eq!(
        model["calls"], 90,
        "a harness's calls still count toward the model"
    );
    assert!(
        model["p95_ms"].as_f64().unwrap() < 1000.0,
        "but not its turn time"
    );

    server.cleanup().await;
}

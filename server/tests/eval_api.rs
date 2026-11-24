//! The replay-eval API (`/api/evals/*`) against a real database, with a fake batch runner and
//! no GitHub: where a session ran is recorded on ingest, sessions are judged eligible or not,
//! work is refused where it cannot run, every object is its owner's only, and a run is queued,
//! shown, cancelled and deleted. The job worker is not started, so queued jobs stay queued.
//!
//! Requires infra (Postgres :5432, Redis, S3); each test runs on its own throwaway database:
//!   cargo test -p nasiko-server --test eval_api -- --test-threads=1

#![recursion_limit = "256"]
#[allow(dead_code)]
mod common;

use std::sync::Arc;

use async_trait::async_trait;
use chrono::{TimeZone, Utc};
use nasiko_runtime::{BatchJob, BatchOutcome, BatchRunner};
use nasiko_types::{
    CODING_AGENT_EVENT_VERSION, CapturePolicy, CodingAgentEventV1, CodingAgentLlmCall,
    CodingAgentRepo, CodingAgentSession, CodingAgentSessionPlace, CodingAgentSource,
    CodingAgentTimestampQuality, CodingAgentToolAssociation, CodingAgentToolCall,
    CodingAgentToolCallStatus, CodingAgentTurn, coding_agent_event_id, coding_agent_session_id,
};
use serde_json::{Value, json};
use serial_test::serial;
use uuid::Uuid;

/// A runner that is there but never asked to run anything (no worker in these tests).
struct IdleRunner;

#[async_trait]
impl BatchRunner for IdleRunner {
    fn is_available(&self) -> bool {
        true
    }
    async fn build_image(&self, _: &[u8], tag: &str) -> nasiko_runtime::Result<String> {
        Ok(tag.to_string())
    }
    async fn run(&self, _: BatchJob) -> nasiko_runtime::Result<BatchOutcome> {
        unreachable!("no worker runs in these tests")
    }
    async fn cleanup(&self, _: Option<&str>) -> nasiko_runtime::Result<usize> {
        Ok(0)
    }
}

async fn start(enabled: bool) -> common::TestServer {
    let server = common::TestServer::start_with(|c| {
        c.evals.enabled = enabled;
        c.evals.router_url = Some("http://host.docker.internal:8082".into());
    })
    .await;
    if enabled {
        server.state.batch_runner.install(Arc::new(IdleRunner));
    }
    server
}

async fn seed_user(db: &sqlx::PgPool, username: &str) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO users (username, email, password_hash, role)
         VALUES ($1, $1 || '@test.local', 'x', 'member') RETURNING id",
    )
    .bind(username)
    .fetch_one(db)
    .await
    .unwrap()
}

async fn connect_claude(db: &sqlx::PgPool, user: Uuid, name: &str) {
    sqlx::query(
        "INSERT INTO agents (name, owner_id, coding_agent_integration_id) VALUES ($1, $2, 'claude')",
    )
    .bind(name)
    .bind(user)
    .execute(db)
    .await
    .unwrap();
}

async fn llm_config(db: &sqlx::PgPool, user: Uuid, name: &str, model: &str) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO llm_configs (created_by, name, provider, model) VALUES ($1, $2, 'anthropic', $3) \
         RETURNING id",
    )
    .bind(user)
    .bind(name)
    .bind(model)
    .fetch_one(db)
    .await
    .unwrap()
}

/// One Claude Code turn of `session`, synced with chat history, editing a file in the repo.
fn turn(
    agent_name: &str,
    session: &str,
    n: u32,
    place: Option<CodingAgentSessionPlace>,
) -> CodingAgentEventV1 {
    let at = Utc
        .timestamp_opt(1_790_000_000 + i64::from(n) * 60, 0)
        .unwrap();
    let turn_id = format!("turn-{n}");
    CodingAgentEventV1 {
        version: CODING_AGENT_EVENT_VERSION,
        event_id: coding_agent_event_id("claude", session, &turn_id),
        captured_at: at,
        source: CodingAgentSource {
            agent_id: "claude".into(),
            agent_name: agent_name.into(),
        },
        session: CodingAgentSession {
            id: coding_agent_session_id("claude", session),
            source_id: session.into(),
            title: Some("make b return 2".into()),
            parent: None,
            place,
        },
        turn: CodingAgentTurn {
            id: turn_id.clone(),
            prompt: Some("make b return 2".into()),
            response: Some("done".into()),
            started_at: at,
            ended_at: at,
            llm_calls: vec![CodingAgentLlmCall {
                id: format!("call-{n}"),
                provider: "anthropic".into(),
                model: "claude-opus-5-5".into(),
                input_tokens: 100,
                output_tokens: 50,
                cache_read_tokens: 0,
                cache_creation_tokens: 0,
                accounting: None,
                started_at: at,
                ended_at: at,
            }],
            tool_calls: vec![CodingAgentToolCall {
                id: format!("edit-{n}"),
                name: "Edit".into(),
                kind: "tool".into(),
                model_call_id: Some(format!("call-{n}")),
                status: CodingAgentToolCallStatus::Succeeded,
                arguments: Some(json!({
                    "file_path": "/src/app/src/lib.rs",
                    "old_string": "1",
                    "new_string": "2"
                })),
                output: None,
                raw: None,
                error: None,
                started_at: Some(at),
                ended_at: Some(at),
                duration_ms: None,
                association: CodingAgentToolAssociation::Exact,
                timestamp_quality: CodingAgentTimestampQuality::Exact,
            }],
        },
        capture_policy: CapturePolicy::Content,
    }
}

fn github_place() -> CodingAgentSessionPlace {
    CodingAgentSessionPlace {
        cwd: Some("/src/app".into()),
        git_branch: Some("main".into()),
        repo: Some(CodingAgentRepo {
            host: "github.com".into(),
            owner: "acme".into(),
            name: "app".into(),
            root: Some("/src/app".into()),
        }),
    }
}

async fn ingest(server: &common::TestServer, user: Uuid, events: &[CodingAgentEventV1]) {
    let reply: Value = common::as_member(
        server
            .client
            .post(server.url("/api/telemetry/coding-agent/events/batch")),
        &user.to_string(),
        "dev",
    )
    .json(&json!({ "events": events }))
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    for r in reply["data"]["results"]
        .as_array()
        .unwrap_or_else(|| panic!("{reply}"))
    {
        assert_ne!(r["status"], "rejected", "{reply}");
    }
}

async fn call(
    server: &common::TestServer,
    user: Uuid,
    method: reqwest::Method,
    path: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let request = server.client.request(method, server.url(path));
    let mut request = common::as_member(request, &user.to_string(), "dev");
    if let Some(body) = body {
        request = request.json(&body);
    }
    let response = request.send().await.unwrap();
    let status = response.status().as_u16();
    let text = response.text().await.unwrap();
    (
        status,
        serde_json::from_str(&text).unwrap_or(Value::String(text)),
    )
}

#[tokio::test]
#[serial]
async fn sessions_carry_their_repository_and_are_judged_eligible_or_not() {
    let server = start(false).await;
    let db = &server.db;
    let dev = seed_user(db, "dev").await;
    connect_claude(db, dev, "dev-claude").await;

    // A session synced with its repository, then replayed turns that only add it.
    ingest(
        &server,
        dev,
        &[turn("dev-claude", "placed", 1, Some(github_place()))],
    )
    .await;
    ingest(&server, dev, &[turn("dev-claude", "backfilled", 1, None)]).await;
    ingest(
        &server,
        dev,
        &[turn("dev-claude", "backfilled", 1, Some(github_place()))],
    )
    .await;
    // A session synced before its repository was recorded.
    ingest(&server, dev, &[turn("dev-claude", "unplaced", 1, None)]).await;
    // A session on another forge.
    let mut lab = github_place();
    lab.repo.as_mut().unwrap().host = "gitlab.com".into();
    ingest(&server, dev, &[turn("dev-claude", "gitlab", 1, Some(lab))]).await;

    let (status, body) = call(
        &server,
        dev,
        reqwest::Method::GET,
        "/api/evals/harnesses",
        None,
    )
    .await;
    assert_eq!(status, 200, "{body}");
    let harnesses = body["data"]["harnesses"].as_array().unwrap();
    assert_eq!(harnesses.len(), 1, "{body}");
    assert_eq!(harnesses[0]["id"], "claude");
    assert_eq!(harnesses[0]["sessions"], 4);
    assert_eq!(harnesses[0]["supported"], true);
    assert_eq!(body["data"]["github_connected"], false);
    assert_eq!(body["data"]["runner_available"], false);

    let (status, body) = call(
        &server,
        dev,
        reqwest::Method::GET,
        "/api/evals/harnesses/claude/sessions",
        None,
    )
    .await;
    assert_eq!(status, 200, "{body}");
    let by_source: std::collections::BTreeMap<String, Value> = body["data"]["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| (s["session_id"].as_str().unwrap().to_string(), s.clone()))
        .collect();
    let reasons: std::collections::BTreeSet<String> = by_source
        .values()
        .map(|s| {
            s["ineligible_reason"]
                .as_str()
                .unwrap_or("eligible")
                .to_string()
        })
        .collect();
    // GitHub is not connected, so the two GitHub sessions say so; the others say why not.
    assert_eq!(
        reasons,
        ["github_not_connected", "no_repo", "not_github"]
            .into_iter()
            .map(String::from)
            .collect()
    );
    let placed: Vec<&Value> = by_source
        .values()
        .filter(|s| s["repo"] == "acme/app")
        .collect();
    assert_eq!(placed.len(), 3, "placed, backfilled and gitlab: {body}");
    assert!(
        placed
            .iter()
            .all(|s| s["turns"] == 1 && s["models"] == json!(["claude-opus-5-5"]))
    );

    let stored: (Option<String>, Option<String>) = sqlx::query_as(
        "SELECT git_branch, repo_root FROM chat_sessions cs \
         JOIN coding_agent_telemetry_events e ON e.session_id = cs.session_id \
         WHERE e.source_session_id = 'backfilled'",
    )
    .fetch_one(db)
    .await
    .unwrap();
    assert_eq!(stored, (Some("main".into()), Some("/src/app".into())));

    // Another user sees none of it.
    let other = seed_user(db, "other").await;
    let (status, body) = call(
        &server,
        other,
        reqwest::Method::GET,
        "/api/evals/harnesses",
        None,
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(body["data"]["harnesses"], json!([]));
    let (status, body) = call(
        &server,
        other,
        reqwest::Method::GET,
        "/api/evals/harnesses/claude/sessions",
        None,
    )
    .await;
    assert_eq!(
        (status, body["code"].as_str()),
        (404, Some("harness_not_connected"))
    );
}

#[tokio::test]
#[serial]
async fn work_is_refused_where_it_cannot_run() {
    let disabled = start(false).await;
    let dev = seed_user(&disabled.db, "dev").await;
    connect_claude(&disabled.db, dev, "dev-claude").await;
    let body = json!({"harness": "claude", "session_ids": ["s"]});
    let (status, reply) = call(
        &disabled,
        dev,
        reqwest::Method::POST,
        "/api/evals/datasets",
        Some(body.clone()),
    )
    .await;
    assert_eq!(
        (status, reply["code"].as_str()),
        (503, Some("eval_runner_unavailable"))
    );

    let enabled = start(true).await;
    let dev = seed_user(&enabled.db, "dev").await;
    connect_claude(&enabled.db, dev, "dev-claude").await;
    let (status, reply) = call(
        &enabled,
        dev,
        reqwest::Method::POST,
        "/api/evals/datasets",
        Some(body),
    )
    .await;
    assert_eq!(
        (status, reply["code"].as_str()),
        (409, Some("github_not_connected")),
        "{reply}"
    );
}

/// A dataset with one verified task, as a finished harvest, build and screen leave it.
async fn ready_dataset(db: &sqlx::PgPool, user: Uuid, status: &str) -> (Uuid, Uuid) {
    let dataset: Uuid = sqlx::query_scalar(
        "INSERT INTO eval_datasets (user_id, harness, status, session_ids) \
         VALUES ($1, 'claude', 'ready', ARRAY['s']) RETURNING id",
    )
    .bind(user)
    .fetch_one(db)
    .await
    .unwrap();
    let mut instance: Value = serde_json::from_str(include_str!(
        "../../cli/src/commands/eval/testdata/task.json"
    ))
    .unwrap();
    instance["x_nasiko"]["status"] = json!(status);
    instance["x_nasiko"]["orig"]["model"] = json!("claude-opus-5-5");
    let source = json!({
        "session_id": "s", "source": "claude-code", "git_branch": null,
        "started_at": "2026-11-01T10:00:00Z", "ended_at": "2026-11-01T11:00:00Z",
        "task_messages": [{"at": "2026-11-01T10:00:00Z", "text": "make b return 2"}],
        "context_messages": [], "commit_subjects": [], "changed_files": [],
        "usage_by_model": []
    });
    let task: Uuid = sqlx::query_scalar(
        "INSERT INTO eval_tasks (dataset_id, repo, source_session_id, status, stage, instance, source, link) \
         VALUES ($1, 'acme/app', 's', $2, 'done', $3, $4, '{}') RETURNING id",
    )
    .bind(dataset)
    .bind(status)
    .bind(&instance)
    .bind(&source)
    .fetch_one(db)
    .await
    .unwrap();
    (dataset, task)
}

#[tokio::test]
#[serial]
async fn a_run_is_queued_shown_cancelled_and_deleted_by_its_owner_only() {
    let server = start(true).await;
    let db = &server.db;
    let dev = seed_user(db, "dev").await;
    let other = seed_user(db, "other").await;
    let (dataset, task) = ready_dataset(db, dev, "verified").await;
    let opus = llm_config(db, dev, "opus", "claude-opus-5-5").await;
    let haiku = llm_config(db, dev, "haiku", "claude-haiku-4-5").await;
    let foreign = llm_config(db, other, "theirs", "gpt-4o").await;

    let (status, body) = call(
        &server,
        dev,
        reqwest::Method::GET,
        &format!("/api/evals/datasets/{dataset}"),
        None,
    )
    .await;
    assert_eq!(status, 200, "{body}");
    let view = &body["data"];
    assert_eq!(view["status"], "ready");
    assert_eq!(view["repos"], json!(["acme/app"]));
    assert_eq!(view["tasks"][0]["status"], "verified");
    assert_eq!(view["tasks"][0]["original"]["model"], "claude-opus-5-5");
    let models: Vec<(String, bool)> = view["models"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| {
            (
                m["name"].as_str().unwrap().to_string(),
                m["used"].as_bool().unwrap(),
            )
        })
        .collect();
    assert_eq!(models, vec![("haiku".into(), false), ("opus".into(), true)]);

    let (status, _) = call(
        &server,
        other,
        reqwest::Method::GET,
        &format!("/api/evals/datasets/{dataset}"),
        None,
    )
    .await;
    assert_eq!(status, 404, "another user's dataset is not found");

    let runs = format!("/api/evals/datasets/{dataset}/runs");
    let (status, body) = call(
        &server,
        dev,
        reqwest::Method::POST,
        &runs,
        Some(json!({"task_ids": [task], "model_ids": [foreign]})),
    )
    .await;
    assert_eq!(
        (status, body["code"].as_str()),
        (400, Some("invalid_model"))
    );
    let (status, body) = call(
        &server,
        dev,
        reqwest::Method::POST,
        &runs,
        Some(json!({"task_ids": [Uuid::new_v4()], "model_ids": [haiku]})),
    )
    .await;
    assert_eq!(
        (status, body["code"].as_str()),
        (400, Some("invalid_tasks"))
    );
    let (status, _) = call(
        &server,
        other,
        reqwest::Method::POST,
        &runs,
        Some(json!({"task_ids": [task], "model_ids": [foreign]})),
    )
    .await;
    assert_eq!(status, 404);

    let (status, body) = call(
        &server,
        dev,
        reqwest::Method::POST,
        &runs,
        Some(json!({"task_ids": [task], "model_ids": [haiku, opus], "attempts": 2})),
    )
    .await;
    assert_eq!(status, 202, "{body}");
    let run = &body["data"];
    assert_eq!(run["status"], "queued");
    assert_eq!(run["progress"], json!({"done": 0, "total": 4}));
    assert_eq!(run["results"].as_array().unwrap().len(), 4);
    assert!(
        run["results"]
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r["status"] == "pending" && r["score"].is_null())
    );
    let run_id = run["id"].as_str().unwrap().to_string();
    let preflights: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM eval_jobs WHERE run_id = $1::uuid AND kind = 'run_preflight' AND status = 'pending'",
    )
    .bind(&run_id)
    .fetch_one(db)
    .await
    .unwrap();
    assert_eq!(preflights, 2, "one preflight per model");

    let (status, body) = call(&server, dev, reqwest::Method::GET, "/api/evals/runs", None).await;
    assert_eq!(status, 200);
    assert_eq!(body["data"].as_array().unwrap().len(), 1);
    assert_eq!(body["data"][0]["harness"], "claude");
    let (status, body) = call(
        &server,
        other,
        reqwest::Method::GET,
        "/api/evals/runs",
        None,
    )
    .await;
    assert_eq!((status, body["data"].as_array().unwrap().len()), (200, 0));
    let (status, _) = call(
        &server,
        other,
        reqwest::Method::GET,
        &format!("/api/evals/runs/{run_id}"),
        None,
    )
    .await;
    assert_eq!(status, 404);

    let (status, _) = call(
        &server,
        dev,
        reqwest::Method::DELETE,
        &format!("/api/evals/runs/{run_id}"),
        None,
    )
    .await;
    assert_eq!(
        status, 409,
        "a running run is cancelled before it is deleted"
    );
    let (status, body) = call(
        &server,
        dev,
        reqwest::Method::POST,
        &format!("/api/evals/runs/{run_id}/cancel"),
        None,
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["data"]["status"], "cancelled");
    assert!(
        body["data"]["results"]
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r["status"] == "cancelled")
    );
    let (status, _) = call(
        &server,
        dev,
        reqwest::Method::DELETE,
        &format!("/api/evals/runs/{run_id}"),
        None,
    )
    .await;
    assert_eq!(status, 204);
    let (status, _) = call(
        &server,
        dev,
        reqwest::Method::DELETE,
        &format!("/api/evals/datasets/{dataset}"),
        None,
    )
    .await;
    assert_eq!(status, 204);
}

#[tokio::test]
#[serial]
async fn a_person_decides_a_task_no_reviewer_could() {
    let server = start(true).await;
    let db = &server.db;
    let dev = seed_user(db, "dev").await;
    let (_, task) = ready_dataset(db, dev, "reproducible").await;
    let review = format!("/api/evals/tasks/{task}/review");
    let (status, body) = call(
        &server,
        dev,
        reqwest::Method::POST,
        &review,
        Some(json!({"clarity": 0, "test_validity": 1, "effort": "15 min - 1 hour"})),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["data"]["status"], "verified");
    assert_eq!(body["data"]["difficulty"], "15 min - 1 hour");

    let (status, body) = call(
        &server,
        dev,
        reqwest::Method::POST,
        &review,
        Some(json!({"clarity": 0, "test_validity": 0, "effort": "<15 min fix"})),
    )
    .await;
    assert_eq!(
        (status, body["code"].as_str()),
        (409, Some("not_reviewable"))
    );

    let other = seed_user(db, "other").await;
    let (status, _) = call(
        &server,
        other,
        reqwest::Method::GET,
        &format!("/api/evals/tasks/{task}"),
        None,
    )
    .await;
    assert_eq!(status, 404);
}

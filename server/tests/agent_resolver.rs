//! The shared agent-target resolver: wake-before-resolve for parked agents, the rule table by
//! (status, kind), `agent_acl`, the registry's proxy URLs, and open tasks holding a park. Also
//! Start on a parked agent, which wakes it through the same seam.
//!
//! Requires infra (Postgres :5432, Redis, S3):
//!   cargo test -p nasiko-server --test agent_resolver -- --test-threads=1

mod common;

use std::sync::Arc;

use axum::{Json, Router, routing::post};
use nasiko_server::acl::{check_agent_acl, require_agent_acl};
use nasiko_server::agent_resolver::{Caller, ResolveError, ResolvedTarget, resolve_target};
use serde_json::{Value, json};
use serial_test::serial;
use uuid::Uuid;

async fn init_admin(server: &common::TestServer) -> Uuid {
    let v = server
        .client
        .post(server.url("/api/auth/initialize-admin"))
        .json(&json!({"username": "admin", "email": "admin@test.local"}))
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    Uuid::parse_str(v["user_id"].as_str().unwrap()).unwrap()
}

struct Seed<'a> {
    name: &'a str,
    status: &'a str,
    kind: &'a str,
    policy: &'a str,
    state_mode: &'a str,
    writable: bool,
    url: &'a str,
}

impl<'a> Seed<'a> {
    fn parked(name: &'a str) -> Self {
        Self {
            name,
            status: "parked",
            kind: "agent",
            policy: "auto",
            state_mode: "durable",
            writable: false,
            url: "",
        }
    }
}

async fn seed_agent(server: &common::TestServer, owner: Uuid, seed: Seed<'_>) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO agents (name, owner_id, image, status, url, is_public, workload_kind, \
                             parking_policy, state_mode, writable) \
         VALUES ($1, $2, 'x:1.0.0', $3, $4, false, $5, $6, $7, $8) RETURNING id",
    )
    .bind(seed.name)
    .bind(owner)
    .bind(seed.status)
    .bind(seed.url)
    .bind(seed.kind)
    .bind(seed.policy)
    .bind(seed.state_mode)
    .bind(seed.writable)
    .fetch_one(&server.db)
    .await
    .unwrap()
}

/// A stub agent answering every POST with an A2A message and GET /echo with `{ok:true}`.
async fn start_stub_agent() -> String {
    let a2a = post(|| async {
        Json(json!({
            "jsonrpc": "2.0", "id": "1",
            "result": {"kind": "message", "parts": [{"kind": "text", "text": "ok"}]}
        }))
    });
    let app = Router::new().route("/", a2a).route(
        "/echo",
        axum::routing::get(|| async { Json(json!({"ok": true})) }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://127.0.0.1:{port}")
}

async fn status_and_last_request(server: &common::TestServer, id: Uuid) -> (String, bool) {
    sqlx::query_as("SELECT status, last_request_at IS NOT NULL FROM agents WHERE id = $1")
        .bind(id)
        .fetch_one(&server.db)
        .await
        .unwrap()
}

#[tokio::test]
#[serial]
async fn the_proxy_wakes_a_parked_agent_before_it_resolves_it() {
    let server = common::TestServer::start().await;
    // A runtime that, like a real one, has no workload for an agent nobody deployed.
    server.runtime.set_status_follows_deploys(true);
    let wake = common::RowWake::install(&server);
    let owner = init_admin(&server).await;
    let agent = seed_agent(&server, owner, Seed::parked("wake-me")).await;
    wake.serve_at(agent, &start_stub_agent().await);

    let res = common::as_superuser(
        server
            .client
            .get(server.url(&format!("/api/agents/{agent}/echo"))),
        &owner.to_string(),
        "admin",
    )
    .send()
    .await
    .unwrap();

    assert_eq!(res.status(), 200);
    assert_eq!(wake.woken_count(agent), 1);
    assert_eq!(
        status_and_last_request(&server, agent).await,
        ("running".to_owned(), true)
    );
    server.cleanup().await;
}

/// A parked agent the wake does not bring back is refused with a 503 the caller is told when to
/// retry, never forwarded to.
#[tokio::test]
#[serial]
async fn a_parked_agent_the_wake_leaves_parked_is_refused_with_a_retry_after() {
    let server = common::TestServer::start().await;
    server.runtime.set_status_follows_deploys(true);
    let owner = init_admin(&server).await;
    let agent = seed_agent(&server, owner, Seed::parked("stays-parked")).await;

    let res = common::as_superuser(
        server
            .client
            .get(server.url(&format!("/api/agents/{agent}/echo"))),
        &owner.to_string(),
        "admin",
    )
    .send()
    .await
    .unwrap();

    assert_eq!(res.status(), 503);
    assert_eq!(res.headers()["retry-after"], "2");
    assert_eq!(status_and_last_request(&server, agent).await.0, "parked");
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_resident_agent_is_served_without_a_wake() {
    let server = common::TestServer::start().await;
    let wake = common::RowWake::install(&server);
    let owner = init_admin(&server).await;
    let stub = start_stub_agent().await;
    let agent = seed_agent(
        &server,
        owner,
        Seed {
            status: "running",
            policy: "resident",
            url: &stub,
            ..Seed::parked("stay-put")
        },
    )
    .await;

    let res = common::as_superuser(
        server
            .client
            .get(server.url(&format!("/api/agents/{agent}/echo"))),
        &owner.to_string(),
        "admin",
    )
    .send()
    .await
    .unwrap();

    assert_eq!(res.status(), 200);
    assert_eq!(wake.woken_count(agent), 0);
    // Every served request restarts the agent's idle clock, a resident one's too (where `resident`
    // is not honoured, the Free plane parks it by that clock).
    assert_eq!(
        status_and_last_request(&server, agent).await,
        ("running".to_owned(), true)
    );
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn dispatch_by_name_wakes_a_parked_agent() {
    let server = common::TestServer::start().await;
    server.runtime.set_status_follows_deploys(true);
    let wake = common::RowWake::install(&server);
    let owner = init_admin(&server).await;
    let agent = seed_agent(&server, owner, Seed::parked("dispatch-me")).await;
    wake.serve_at(agent, &start_stub_agent().await);

    let body = json!({
        "jsonrpc": "2.0", "method": "message/stream", "id": "1",
        "params": {
            "message": {
                "messageId": Uuid::new_v4().to_string(),
                "role": "ROLE_USER",
                "parts": [{ "text": "hello" }]
            },
            "metadata": { "agent_id": "dispatch-me" }
        }
    });
    let res = common::as_superuser(
        server.client.post(server.url("/api/orchestrator/a2a")),
        &owner.to_string(),
        "admin",
    )
    .json(&body)
    .send()
    .await
    .unwrap();

    assert!(res.status().is_success(), "{}", res.status());
    assert_eq!(wake.woken_count(agent), 1);
    assert_eq!(
        status_and_last_request(&server, agent).await,
        ("running".to_owned(), true)
    );
    server.cleanup().await;
}

/// `POST /api/containers/{id}/{action}` as the agent's owner (the admin).
async fn lifecycle(
    server: &common::TestServer,
    owner: Uuid,
    agent: Uuid,
    action: &str,
) -> (u16, String) {
    let res = common::as_superuser(
        server
            .client
            .post(server.url(&format!("/api/containers/{agent}/{action}"))),
        &owner.to_string(),
        "admin",
    )
    .send()
    .await
    .unwrap();
    (res.status().as_u16(), res.text().await.unwrap())
}

/// Start on a parked agent is the wake its next message would get, so it comes back with its
/// address and idle clock, not as a `running` row nothing serves.
#[tokio::test]
#[serial]
async fn starting_a_parked_agent_wakes_it() {
    let server = common::TestServer::start().await;
    server.runtime.set_status_follows_deploys(true);
    let wake = common::RowWake::install(&server);
    let owner = init_admin(&server).await;
    let agent = seed_agent(&server, owner, Seed::parked("start-me")).await;
    let stub = start_stub_agent().await;
    wake.serve_at(agent, &stub);

    assert_eq!(lifecycle(&server, owner, agent, "start").await.0, 200);
    assert_eq!(wake.woken_count(agent), 1);
    assert_eq!(
        status_and_last_request(&server, agent).await,
        ("running".to_owned(), true)
    );
    let url: Option<String> = sqlx::query_scalar("SELECT url FROM agents WHERE id = $1")
        .bind(agent)
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert_eq!(url.as_deref(), Some(stub.as_str()));
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_parked_agent_the_wake_cannot_start_is_refused_and_stays_parked() {
    let server = common::TestServer::start().await;
    server.runtime.set_status_follows_deploys(true);
    common::RowWake::install(&server);
    let owner = init_admin(&server).await;
    let agent = seed_agent(&server, owner, Seed::parked("cannot-start")).await;

    let (status, body) = lifecycle(&server, owner, agent, "start").await;

    assert_eq!(status, 503);
    assert!(body.contains("could not be started"), "{body}");
    assert_eq!(status_and_last_request(&server, agent).await.0, "parked");
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn resolution_by_status_and_kind() {
    let server = common::TestServer::start().await;
    server.runtime.set_status_follows_deploys(true);
    let owner = init_admin(&server).await;
    let stored = "http://stored.test:9000";

    let running_resident = seed_agent(
        &server,
        owner,
        Seed {
            status: "running",
            policy: "resident",
            url: stored,
            ..Seed::parked("running-resident")
        },
    )
    .await;
    let running_parkable = seed_agent(
        &server,
        owner,
        Seed {
            status: "running",
            url: stored,
            ..Seed::parked("running-parkable")
        },
    )
    .await;
    let parked_parkable = seed_agent(&server, owner, Seed::parked("parked-parkable")).await;
    let parked_resident = seed_agent(
        &server,
        owner,
        Seed {
            policy: "resident",
            ..Seed::parked("parked-resident")
        },
    )
    .await;
    let writable = seed_agent(
        &server,
        owner,
        Seed {
            status: "running",
            writable: true,
            url: stored,
            ..Seed::parked("writable")
        },
    )
    .await;
    let stopped = seed_agent(
        &server,
        owner,
        Seed {
            status: "stopped",
            ..Seed::parked("stopped")
        },
    )
    .await;
    let harness = seed_agent(
        &server,
        owner,
        Seed {
            status: "running",
            kind: "harness",
            ..Seed::parked("harness")
        },
    )
    .await;
    let deleted = seed_agent(
        &server,
        owner,
        Seed {
            status: "running",
            url: stored,
            ..Seed::parked("deleted")
        },
    )
    .await;
    sqlx::query("UPDATE agents SET deleted_at = now() WHERE id = $1")
        .bind(deleted)
        .execute(&server.db)
        .await
        .unwrap();

    let state = &server.state;
    let resolve = |id| async move { resolve_target(state, id, &Caller::default()).await };

    assert!(matches!(
        resolve(Uuid::new_v4()).await,
        Err(ResolveError::NotFound)
    ));
    assert!(matches!(
        resolve(deleted).await,
        Err(ResolveError::NotFound)
    ));
    assert!(matches!(
        resolve(harness).await,
        Ok(ResolvedTarget::Harness)
    ));
    assert!(matches!(
        resolve(stopped).await,
        Err(ResolveError::NotRunning(s)) if s == "stopped"
    ));
    // Every running agent is served by its own workload, whatever its policy.
    for running in [running_resident, running_parkable, writable] {
        assert!(matches!(
            resolve(running).await,
            Ok(ResolvedTarget::Container(t)) if t.base.url == stored
        ));
    }
    // A parked agent nothing woke (no wake is installed here) is refused for a retry, whatever its
    // policy, and left parked for the next call's wake.
    for parked in [parked_parkable, parked_resident] {
        assert!(matches!(
            resolve(parked).await,
            Err(ResolveError::ParkedMidRequest)
        ));
        assert_eq!(status_and_last_request(&server, parked).await.0, "parked");
    }
    // A resident agent whose workload is gone is the caller's error: the row is not flipped.
    assert_eq!(
        status_and_last_request(&server, running_resident).await.0,
        "running"
    );

    // A parked agent the wake brings back resolves to its workload, its policy notwithstanding.
    let wake = common::RowWake::install(&server);
    wake.serve_at(parked_resident, "http://woken.test:9000");
    assert!(matches!(
        resolve(parked_resident).await,
        Ok(ResolvedTarget::Container(t)) if t.base.url == "http://woken.test:9000"
    ));
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn agent_acl_is_an_allowlist_enforced_before_a_leg() {
    let server = common::TestServer::start().await;
    let owner = init_admin(&server).await;
    let caller = seed_agent(&server, owner, Seed::parked("acl-caller")).await;
    let listed = seed_agent(&server, owner, Seed::parked("acl-listed")).await;
    let unlisted = seed_agent(&server, owner, Seed::parked("acl-unlisted")).await;

    // No rows: unrestricted.
    assert!(check_agent_acl(&server.db, caller, unlisted).await.unwrap());

    sqlx::query("INSERT INTO agent_acl (caller_agent_id, target_agent_id) VALUES ($1, $2)")
        .bind(caller)
        .bind(listed)
        .execute(&server.db)
        .await
        .unwrap();

    assert!(require_agent_acl(&server.db, caller, listed).await.is_ok());
    assert_eq!(
        require_agent_acl(&server.db, caller, unlisted).await,
        Err(axum::http::StatusCode::FORBIDDEN)
    );
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn the_registry_hands_out_proxy_urls_for_every_routable_agent() {
    let server = common::TestServer::start().await;
    let owner = init_admin(&server).await;
    let parked = seed_agent(&server, owner, Seed::parked("reg-parked")).await;
    let running = seed_agent(
        &server,
        owner,
        Seed {
            status: "running",
            policy: "resident",
            url: "http://172.31.0.68:31640",
            ..Seed::parked("reg-running")
        },
    )
    .await;
    let stopped = seed_agent(
        &server,
        owner,
        Seed {
            status: "stopped",
            ..Seed::parked("reg-stopped")
        },
    )
    .await;
    // Discovery without credentials lists public agents only.
    sqlx::query("UPDATE agents SET is_public = true WHERE id = ANY($1)")
        .bind(vec![parked, running, stopped])
        .execute(&server.db)
        .await
        .unwrap();

    let body: Value = server
        .client
        .post(server.url("/a2a/v1"))
        .json(&json!({"jsonrpc": "2.0", "id": 1, "method": "message/send", "params": {}}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let agents = body
        .pointer("/result/artifacts/0/parts/0/data/agents")
        .and_then(Value::as_array)
        .unwrap();

    // The agent proxy path, relative to this server: never a container address.
    let url_of = |id: Uuid| {
        agents
            .iter()
            .find(|a| a["id"] == id.to_string())
            .map(|a| a["url"].as_str().unwrap().to_owned())
    };
    assert_eq!(
        url_of(parked).as_deref(),
        Some(format!("/api/agents/{parked}").as_str())
    );
    assert_eq!(
        url_of(running).as_deref(),
        Some(format!("/api/agents/{running}").as_str())
    );
    assert_eq!(url_of(stopped), None);
    server.cleanup().await;
}

/// A stub agent whose `message/send` answers a task in whatever state `state` currently holds.
async fn start_task_agent(state: Arc<std::sync::Mutex<&'static str>>) -> String {
    let a2a = post(move || {
        let current = *state.lock().unwrap();
        async move {
            Json(json!({
                "jsonrpc": "2.0", "id": "1",
                "result": {"kind": "task", "id": "task-1", "status": {"state": current}}
            }))
        }
    });
    let app = Router::new().route("/", a2a);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://127.0.0.1:{port}")
}

async fn send_message(server: &common::TestServer, owner: Uuid, agent: Uuid) {
    let body = json!({
        "jsonrpc": "2.0", "method": "message/send", "id": "1",
        "params": {"message": {
            "messageId": Uuid::new_v4().to_string(),
            "role": "ROLE_USER",
            "parts": [{ "text": "hello" }]
        }}
    });
    let res = common::as_superuser(
        server
            .client
            .post(server.url(&format!("/api/agents/{agent}"))),
        &owner.to_string(),
        "admin",
    )
    .json(&body)
    .send()
    .await
    .unwrap();
    assert!(res.status().is_success(), "{}", res.status());
}

/// The recorder runs off the request path, so wait for the open-task count to settle.
async fn wait_for_open_tasks(server: &common::TestServer, agent: Uuid, want: i64) {
    for _ in 0..50 {
        let n: i64 =
            sqlx::query_scalar("SELECT count(*) FROM agent_open_tasks WHERE agent_id = $1")
                .bind(agent)
                .fetch_one(&server.db)
                .await
                .unwrap();
        if n == want {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    panic!("open task count never reached {want}");
}

/// Sweep as if the agent's last request and its last deploy were older than the park window. A
/// task row the agent's reply left open stays young, as an open pause does. The table's trigger
/// stamps `updated_at` on every UPDATE, so it is held off for the write that ages the row.
async fn sweep_idle_agent(server: &common::TestServer, agent: Uuid) {
    let age = (server.state.config.agent_park_idle_secs + 60) as f64;
    let mut tx = server.db.begin().await.unwrap();
    sqlx::query("ALTER TABLE agents DISABLE TRIGGER trg_agents_updated_at")
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE agents SET last_request_at = now() - make_interval(secs => $2), \
                updated_at = now() - make_interval(secs => $2) \
         WHERE id = $1",
    )
    .bind(agent)
    .bind(age)
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query("ALTER TABLE agents ENABLE TRIGGER trg_agents_updated_at")
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    nasiko_server::sessions::reaper::sweep(
        &server.state,
        &mut nasiko_server::sessions::reaper::ReaperMemory::default(),
        nasiko_server::sessions::reaper::SweepScope::Full,
    )
    .await
    .unwrap();
}

#[tokio::test]
#[serial]
async fn a_working_task_blocks_the_park_until_a_response_reports_it_completed() {
    let server = common::TestServer::start_with(|c| {
        c.coding_agent_idle_ttl_secs = 60;
        c.session_idle_grace_secs = 0;
        c.session_cpu_quiescent_millicores = 20;
    })
    .await;
    server.runtime.set_status_follows_deploys(true);
    let owner = init_admin(&server).await;
    let state = Arc::new(std::sync::Mutex::new("TASK_STATE_WORKING"));
    let stub = start_task_agent(state.clone()).await;
    let agent = seed_agent(
        &server,
        owner,
        Seed {
            status: "running",
            url: &stub,
            ..Seed::parked("task-holder")
        },
    )
    .await;

    send_message(&server, owner, agent).await;
    wait_for_open_tasks(&server, agent, 1).await;
    sweep_idle_agent(&server, agent).await;
    assert_eq!(status_and_last_request(&server, agent).await.0, "running");

    *state.lock().unwrap() = "TASK_STATE_COMPLETED";
    send_message(&server, owner, agent).await;
    wait_for_open_tasks(&server, agent, 0).await;
    sweep_idle_agent(&server, agent).await;
    assert_eq!(status_and_last_request(&server, agent).await.0, "parked");

    server.cleanup().await;
}

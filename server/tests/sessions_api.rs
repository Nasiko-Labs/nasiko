//! Integration tests for harness sessions: `GET /api/harnesses`, `POST /api/sessions` (which
//! creates the user's harness agent and starts the session's sandbox), `GET` and `DELETE
//! /api/sessions/{id}`, `POST /api/sessions/{id}/suspend` and the idle reaper, against the fake
//! sandbox provider.
//!
//! A test whose subject is not opening a session creates its session without a start
//! ([`parked_session`]), so no start in the background races the state the test sets by hand.
//!
//! Requires infra (Postgres :5432, Redis, S3); skips when Postgres is unreachable:
//!   cargo test -p nasiko-server --test sessions_api -- --test-threads=1

mod common;

use nasiko_config::Config;
use nasiko_runtime::{
    RuntimeError, SandboxHandle, SandboxKey, SandboxProvider, SandboxSpec, SandboxState,
    SandboxStats, TransportKind,
};
use nasiko_server::coding::lifecycle::{TurnLease, heartbeat, release_turn};
use nasiko_server::coding::questions::TurnQuestions;
use nasiko_server::sessions::own_agent;
use nasiko_server::sessions::reaper::{
    ReaperMemory, SweepOutcome, SweepScope, reap_once, recover_orphans, recover_stuck, sweep,
};
use nasiko_server::sessions::{HarnessCatalog, SessionRuntimeState, SessionStore};
use serde_json::{Value, json};
use serial_test::serial;
use std::collections::HashMap;
use std::time::Duration;
use uuid::Uuid;

/// The `AGENT_JWT_SECRET` these tests run with. Without one a sandbox's credentials cannot be
/// signed, so sessions are not ready (`sessions::readiness`) and opening one is a 503.
const AGENT_JWT_SECRET: &str = "sessions-api-secret";

/// A server that can run sessions, with `configure` applied on top.
async fn sessions_server_with(configure: impl FnOnce(&mut Config)) -> common::TestServer {
    common::TestServer::start_with(move |c| {
        c.agent_jwt_secret = AGENT_JWT_SECRET.into();
        configure(c);
    })
    .await
}

async fn sessions_server() -> common::TestServer {
    sessions_server_with(|_| {}).await
}

async fn init_admin(server: &common::TestServer) -> String {
    let body: Value = server
        .client
        .post(server.url("/api/auth/initialize-admin"))
        .json(&json!({"username": "admin", "email": "admin@test.local"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    body["user_id"].as_str().unwrap().to_owned()
}

async fn create_member(server: &common::TestServer, admin_id: &str) -> String {
    let body: Value = common::as_superuser(
        server.client.post(server.url("/api/users")),
        admin_id,
        "admin",
    )
    .json(&json!({"username": "bob", "email": "bob@test.local"}))
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    body["id"].as_str().unwrap().to_owned()
}

/// `POST /api/sessions` for `harness`, as the superuser `admin_id`.
async fn post_session(
    server: &common::TestServer,
    admin_id: &str,
    harness: &str,
) -> reqwest::Response {
    common::as_superuser(
        server.client.post(server.url("/api/sessions")),
        admin_id,
        "admin",
    )
    .json(&json!({ "harness": harness }))
    .send()
    .await
    .unwrap()
}

/// Open a session through the API with `post` (a `POST /api/sessions`, body set), assert the 201
/// and wait for the start it began to settle. Returns the 201's `data`: the row as the start had
/// left it when the response was written.
async fn open_session(server: &common::TestServer, post: reqwest::RequestBuilder) -> Value {
    let resp = post.send().await.unwrap();
    assert_eq!(resp.status(), 201);
    let data = resp.json::<Value>().await.unwrap()["data"].clone();
    wait_for_start(server, data["session_id"].as_str().unwrap()).await;
    data
}

/// [`open_session`] of `harness` as the superuser `admin_id`.
async fn create_session(server: &common::TestServer, admin_id: &str, harness: &str) -> Value {
    let post = common::as_superuser(
        server.client.post(server.url("/api/sessions")),
        admin_id,
        "admin",
    )
    .json(&json!({ "harness": harness }));
    open_session(server, post).await
}

/// Wait, for at most five seconds, until no start holds the session's lease, so the test reads
/// the row as the start left it.
async fn wait_for_start(server: &common::TestServer, sid: &str) {
    for _ in 0..100 {
        let held: bool = sqlx::query_scalar(
            "SELECT turn_owner IS NOT NULL FROM chat_sessions WHERE session_id = $1",
        )
        .bind(sid)
        .fetch_one(&server.db)
        .await
        .unwrap();
        if !held {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("the start of session {sid} still holds its lease");
}

/// The user's own harness agent for `harness`, created as opening a session creates it.
async fn own_harness_agent(server: &common::TestServer, user: Uuid, harness: &str) -> Uuid {
    let harness = HarnessCatalog::new(server.db.clone())
        .get(harness)
        .await
        .unwrap()
        .expect("the harness is in the catalog");
    own_agent::ensure(
        &server.db,
        user,
        &harness,
        &server.state.config.harness_image_prefix,
    )
    .await
    .unwrap()
}

/// A session of `user`'s on `harness`, bound to their own harness agent and parked with no
/// sandbox, with nothing starting one: what a session a message created is before that message's
/// turn. For tests whose subject is not opening a session.
async fn parked_session(server: &common::TestServer, user: &str, harness: &str) -> String {
    let user = Uuid::parse_str(user).unwrap();
    let agent = own_harness_agent(server, user, harness).await;
    SessionStore::new(server.db.clone())
        .create(agent, user, None)
        .await
        .unwrap()
        .session_id
}

/// The harnesses `GET /api/harnesses` offers `admin_id`: the response's `data`.
async fn catalog(server: &common::TestServer, admin_id: &str) -> Value {
    let body: Value = common::as_superuser(
        server.client.get(server.url("/api/harnesses")),
        admin_id,
        "admin",
    )
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    body["data"].clone()
}

/// No harness agent and no session row exists.
async fn assert_nothing_written(server: &common::TestServer) {
    let (agents, sessions): (i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM agents WHERE workload_kind = 'harness'), \
                (SELECT count(*) FROM chat_sessions)",
    )
    .fetch_one(&server.db)
    .await
    .unwrap();
    assert_eq!((agents, sessions), (0, 0), "(harness agents, sessions)");
}

/// A server that can run sessions lists every enabled harness of the catalog.
#[tokio::test]
#[serial]
async fn harnesses_lists_the_three_seeds() {
    let server = sessions_server().await;
    let admin = init_admin(&server).await;

    let harnesses = catalog(&server, &admin).await;

    let ids: Vec<&str> = harnesses
        .as_array()
        .unwrap()
        .iter()
        .map(|h| h["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["claude-code", "codex", "opencode"]);
    server.cleanup().await;
}

/// Opening a session starts its sandbox straight away: the 201 answers it `starting` (or
/// `running`, when the start was quick), and once the start settles the row is `running` on the
/// one sandbox it acquired, with no turn recorded. Only its owner sees it; anyone else gets the
/// 404 an unknown id gets.
#[tokio::test]
#[serial]
async fn a_session_is_created_and_started_and_visible_only_to_its_owner() {
    let server = sessions_server().await;
    let admin = init_admin(&server).await;
    let bob = create_member(&server, &admin).await;

    let created = create_session(&server, &admin, "opencode").await;
    assert!(
        matches!(
            created["runtime_state"].as_str(),
            Some("starting" | "running")
        ),
        "{created}"
    );
    assert_eq!(created["harness"], "opencode");
    let sid = created["session_id"].as_str().unwrap();

    let (state, sandbox_ref, turn_ran): (String, Option<String>, bool) = sqlx::query_as(
        "SELECT runtime_state, sandbox_ref, last_turn_at IS NOT NULL \
         FROM chat_sessions WHERE session_id = $1",
    )
    .bind(sid)
    .fetch_one(&server.db)
    .await
    .unwrap();
    assert_eq!(state, "running");
    assert!(
        sandbox_ref
            .as_deref()
            .is_some_and(|r| r.starts_with("fake:")),
        "{sandbox_ref:?}"
    );
    assert!(!turn_ran, "a start is not a turn");
    let key = SandboxKey::Session {
        session_id: Uuid::parse_str(sid).unwrap(),
    };
    assert_eq!(server.sandbox.acquire_count(&key), 1);

    let own = common::as_superuser(
        server
            .client
            .get(server.url(&format!("/api/sessions/{sid}"))),
        &admin,
        "admin",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(own.status(), 200);
    assert_eq!(
        own.json::<Value>().await.unwrap()["data"]["runtime_state"],
        "running"
    );

    let other = common::as_member(
        server
            .client
            .get(server.url(&format!("/api/sessions/{sid}"))),
        &bob,
        "bob",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(other.status(), 404);
    assert_eq!(
        other.json::<Value>().await.unwrap()["code"],
        "session_not_found"
    );
    server.cleanup().await;
}

/// A start whose sandbox cannot be had does not fail the request that opened the session: the
/// session is still created (201) and is left as it was, parked with no sandbox and its lease
/// given up, so its first message starts the sandbox itself.
#[tokio::test]
#[serial]
async fn a_start_whose_acquire_fails_leaves_the_opened_session_parked() {
    let server = sessions_server().await;
    let admin = init_admin(&server).await;
    server
        .sandbox
        .fail_next_acquire(RuntimeError::Internal("no capacity".into()));

    let created = create_session(&server, &admin, "codex").await;
    let sid = created["session_id"].as_str().unwrap();

    assert_eq!(row_state(&server, sid).await, ("parked".to_owned(), None));
    assert!(lease_is_clear(&server, sid).await);
    let key = SandboxKey::Session {
        session_id: Uuid::parse_str(sid).unwrap(),
    };
    assert_eq!(
        server.sandbox.acquire_count(&key),
        1,
        "the start tried once"
    );
    assert_eq!(server.sandbox.creations(&key), 0);
    server.cleanup().await;
}

/// Without `AGENT_JWT_SECRET` no sandbox credential can be signed, so sessions are not ready: no
/// harness is listed and opening a session is `503 harness_not_ready`, with nothing written.
#[tokio::test]
#[serial]
async fn without_an_agent_jwt_secret_sessions_are_not_ready() {
    let server = common::TestServer::start_with(|c| c.agent_jwt_secret = String::new()).await;
    let admin = init_admin(&server).await;

    assert_not_ready(&server, &admin).await;
    server.cleanup().await;
}

/// A sandbox provider that cannot serve a session makes sessions not ready, as a missing secret
/// does.
#[tokio::test]
#[serial]
async fn with_the_sandbox_provider_unavailable_sessions_are_not_ready() {
    let server = sessions_server().await;
    let admin = init_admin(&server).await;
    server.sandbox.set_unavailable(true);

    assert_not_ready(&server, &admin).await;
    server.cleanup().await;
}

/// No harness is listed, and opening one is `503 harness_not_ready` with nothing written.
async fn assert_not_ready(server: &common::TestServer, admin: &str) {
    assert_eq!(catalog(server, admin).await, json!([]));
    let resp = post_session(server, admin, "opencode").await;
    assert_eq!(resp.status(), 503);
    assert_eq!(
        resp.json::<Value>().await.unwrap()["code"],
        "harness_not_ready"
    );
    assert_nothing_written(server).await;
}

fn agent_of(session: &Value) -> Uuid {
    session["agent_id"].as_str().unwrap().parse().unwrap()
}

/// A session is bound to its user's own harness agent: created by their first session of that
/// harness and reused by the next, owned by them, never public, parked until a provider runs it.
/// Each user gets their own.
#[tokio::test]
#[serial]
async fn a_session_is_bound_to_its_users_own_harness_agent() {
    let server = sessions_server().await;
    let admin = init_admin(&server).await;
    let bob = create_member(&server, &admin).await;

    let first = create_session(&server, &admin, "opencode").await;
    let second = create_session(&server, &admin, "opencode").await;
    let bobs = open_session(
        &server,
        common::as_member(server.client.post(server.url("/api/sessions")), &bob, "bob")
            .json(&json!({ "harness": "opencode" })),
    )
    .await;

    assert_eq!(agent_of(&first), agent_of(&second));
    assert_ne!(agent_of(&first), agent_of(&bobs));
    for (session, owner) in [(&first, &admin), (&bobs, &bob)] {
        let row: (String, Uuid, bool, String, String) = sqlx::query_as(
            "SELECT name, owner_id, is_public, status, workload_kind FROM agents WHERE id = $1",
        )
        .bind(agent_of(session))
        .fetch_one(&server.db)
        .await
        .unwrap();
        assert_eq!(
            (
                row.0.as_str(),
                row.1.to_string(),
                row.2,
                row.3.as_str(),
                row.4.as_str()
            ),
            (
                "harness-opencode",
                owner.clone(),
                false,
                "parked",
                "harness"
            )
        );
    }
    server.cleanup().await;
}

/// An agent of the user's that already holds the name their harness agent takes is never taken
/// over: the session is refused with a 409 and nothing is written.
#[tokio::test]
#[serial]
async fn a_session_is_refused_when_its_harness_agents_name_is_taken() {
    let server = sessions_server().await;
    let admin = init_admin(&server).await;
    let admin_id: Uuid = admin.parse().unwrap();
    sqlx::query(
        "INSERT INTO agents (name, owner_id, status, workload_kind) \
         VALUES ('harness-opencode', $1, 'running', 'agent')",
    )
    .bind(admin_id)
    .execute(&server.db)
    .await
    .unwrap();

    let resp = post_session(&server, &admin, "opencode").await;
    assert_eq!(resp.status(), 409);
    assert_eq!(
        resp.json::<Value>().await.unwrap()["code"],
        "harness_agent_name_taken"
    );
    let sessions: i64 = sqlx::query_scalar("SELECT count(*) FROM chat_sessions WHERE user_id = $1")
        .bind(admin_id)
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert_eq!(sessions, 0);
    server.cleanup().await;
}

/// A harness the catalog does not have is a 400, even on a server that can run sessions.
#[tokio::test]
#[serial]
async fn unknown_harness_is_a_400() {
    let server = sessions_server().await;
    let admin = init_admin(&server).await;
    let resp = post_session(&server, &admin, "nope").await;
    assert_eq!(resp.status(), 400);
    assert_eq!(
        resp.json::<Value>().await.unwrap()["code"],
        "unknown_harness"
    );
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn suspend_calls_the_provider_and_parks_the_session() {
    let server = sessions_server().await;
    let admin = init_admin(&server).await;
    let sid = &parked_session(&server, &admin, "codex").await;

    // Give the session a live fake sandbox and a running row, as the turn path would.
    let key = SandboxKey::Session {
        session_id: Uuid::parse_str(sid).unwrap(),
    };
    let handle = acquire_fake(&server, key).await;
    sqlx::query("UPDATE chat_sessions SET sandbox_ref = $2, runtime_state = 'running' WHERE session_id = $1")
        .bind(sid)
        .bind(handle.sandbox_ref())
        .execute(&server.db)
        .await
        .unwrap();

    let resp = common::as_superuser(
        server
            .client
            .post(server.url(&format!("/api/sessions/{sid}/suspend"))),
        &admin,
        "admin",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(resp.status(), 204);

    assert_eq!(
        server.sandbox.status(&handle).await.unwrap(),
        SandboxState::Suspended
    );
    let (state, suspended): (String, bool) = sqlx::query_as(
        "SELECT runtime_state, suspended_at IS NOT NULL FROM chat_sessions WHERE session_id = $1",
    )
    .bind(sid)
    .fetch_one(&server.db)
    .await
    .unwrap();
    assert_eq!((state.as_str(), suspended), ("parked", true));
    server.cleanup().await;
}

async fn acquire_fake(server: &common::TestServer, key: SandboxKey) -> SandboxHandle {
    server
        .sandbox
        .acquire(&SandboxSpec {
            key,
            image: "nasiko/harness-codex:0.1.0".into(),
            command: vec![],
            env: HashMap::new(),
            resources: None,
            workspace: None,
            runtime_class: None,
            ports: vec![7000],
            transport: TransportKind::WebSocket,
            image_pull_secret: None,
        })
        .await
        .unwrap()
}

// ─── repo, ownership of ids, harness-only visibility ─────────────────────────

#[tokio::test]
#[serial]
async fn repo_is_validated_persisted_and_echoed() {
    let server = sessions_server().await;
    let admin = init_admin(&server).await;
    let post = |repo: &str| {
        common::as_superuser(
            server.client.post(server.url("/api/sessions")),
            &admin,
            "admin",
        )
        .json(&json!({ "harness": "opencode", "repo": repo }))
        .send()
    };

    let bad = post("https://evil.example/o/r").await.unwrap();
    assert_eq!(bad.status(), 400);
    assert_eq!(bad.json::<Value>().await.unwrap()["code"], "invalid_repo");
    assert_eq!(post("http://github.com/o/r").await.unwrap().status(), 400);

    let ok = post("https://github.com/o/r.git").await.unwrap();
    assert_eq!(ok.status(), 201);
    let data = ok.json::<Value>().await.unwrap()["data"].clone();
    assert_eq!(data["workspace_repo"], "https://github.com/o/r.git");
    let sid = data["session_id"].as_str().unwrap();
    wait_for_start(&server, sid).await;
    let stored: Option<String> =
        sqlx::query_scalar("SELECT workspace_repo FROM chat_sessions WHERE session_id = $1")
            .bind(sid)
            .fetch_one(&server.db)
            .await
            .unwrap();
    assert_eq!(stored.as_deref(), Some("https://github.com/o/r.git"));
    server.cleanup().await;
}

/// A contextId names one user's session: a message creates it parked on the sender's own harness
/// agent, and the same contextId from another user finds nothing, not the first user's session.
/// Nor does another user's new contextId open a session on someone else's harness agent.
#[tokio::test]
#[serial]
async fn find_or_create_refuses_a_foreign_context_id() {
    let server = sessions_server().await;
    let admin = init_admin(&server).await;
    let bob = create_member(&server, &admin).await;
    let store = SessionStore::new(server.db.clone());
    let (admin, bob) = (
        Uuid::parse_str(&admin).unwrap(),
        Uuid::parse_str(&bob).unwrap(),
    );
    let agent = own_harness_agent(&server, admin, "opencode").await;

    let mine = store
        .find_or_create_for_harness(agent, admin, "ctx-1")
        .await
        .unwrap();
    assert_eq!(mine.unwrap().runtime_state, "parked");
    assert!(
        store
            .find_or_create_for_harness(agent, bob, "ctx-1")
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .find_or_create_for_harness(agent, bob, "ctx-2")
            .await
            .unwrap()
            .is_none()
    );
    let written: i64 =
        sqlx::query_scalar("SELECT count(*) FROM chat_sessions WHERE session_id = 'ctx-2'")
            .fetch_one(&server.db)
            .await
            .unwrap();
    assert_eq!(written, 0);
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_plain_chat_session_is_not_a_harness_session() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let plain: Value = common::as_superuser(
        server.client.post(server.url("/api/chat/sessions")),
        &admin,
        "admin",
    )
    .json(&json!({"title": "plain"}))
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    let sid = plain["data"]["session_id"].as_str().unwrap();
    for (method, path) in [
        ("GET", format!("/api/sessions/{sid}")),
        ("POST", format!("/api/sessions/{sid}/suspend")),
    ] {
        let rb = if method == "GET" {
            server.client.get(server.url(&path))
        } else {
            server.client.post(server.url(&path))
        };
        let resp = common::as_superuser(rb, &admin, "admin")
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 404, "{method} {path}");
    }
    server.cleanup().await;
}

// ─── fenced suspend and the reaper ───────────────────────────────────────────

async fn reaper_server() -> common::TestServer {
    sessions_server_with(|c| {
        c.coding_agent_idle_ttl_secs = 60;
        c.session_idle_grace_secs = 0;
        c.session_cpu_quiescent_millicores = 20;
    })
    .await
}

/// A session whose sandbox is live in the fake and which has been idle for an hour.
async fn idle_running_session(server: &common::TestServer, admin: &str) -> (String, SandboxHandle) {
    let sid = parked_session(server, admin, "opencode").await;
    let key = SandboxKey::Session {
        session_id: Uuid::parse_str(&sid).unwrap(),
    };
    let handle = acquire_fake(server, key).await;
    sqlx::query(
        "UPDATE chat_sessions SET sandbox_ref = $2, runtime_state = 'running', \
         last_turn_at = now() - interval '1 hour' WHERE session_id = $1",
    )
    .bind(&sid)
    .bind(handle.sandbox_ref())
    .execute(&server.db)
    .await
    .unwrap();
    (sid, handle)
}

async fn row_state(server: &common::TestServer, sid: &str) -> (String, Option<String>) {
    sqlx::query_as("SELECT runtime_state, sandbox_ref FROM chat_sessions WHERE session_id = $1")
        .bind(sid)
        .fetch_one(&server.db)
        .await
        .unwrap()
}

fn cpu(millicores: u32) -> SandboxStats {
    SandboxStats {
        cpu_millicores: millicores,
        memory_bytes: 0,
        sampled_at: chrono::Utc::now(),
        ..Default::default()
    }
}

#[tokio::test]
#[serial]
async fn a_turn_starting_after_the_list_wins_the_race_and_the_sandbox_survives() {
    let server = reaper_server().await;
    let admin = init_admin(&server).await;
    let (sid, handle) = idle_running_session(&server, &admin).await;
    let store = SessionStore::new(server.db.clone());

    let listed = store.list_idle(60).await.unwrap();
    assert_eq!(listed.len(), 1);
    // A turn acquires the lease between the list and the suspend.
    sqlx::query("UPDATE chat_sessions SET turn_owner = 'r1', turn_fence = turn_fence + 1 WHERE session_id = $1")
        .bind(&sid)
        .execute(&server.db)
        .await
        .unwrap();

    assert!(
        store
            .begin_suspend(&sid, listed[0].turn_fence)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        server.sandbox.status(&handle).await.unwrap(),
        SandboxState::Running
    );
    assert_eq!(row_state(&server, &sid).await.0, "running");
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn reaper_suspends_a_quiet_idle_sandbox_and_leaves_a_busy_one() {
    let server = reaper_server().await;
    let admin = init_admin(&server).await;
    let (sid, handle) = idle_running_session(&server, &admin).await;

    server.sandbox.set_stats(&handle, cpu(500));
    let mut quiet = ReaperMemory::default();
    reap_once(&server.state, &mut quiet).await.unwrap();
    assert_eq!(
        server.sandbox.status(&handle).await.unwrap(),
        SandboxState::Running
    );
    assert_eq!(row_state(&server, &sid).await.0, "running");

    server.sandbox.set_stats(&handle, cpu(5));
    reap_once(&server.state, &mut quiet).await.unwrap();
    assert_eq!(
        server.sandbox.status(&handle).await.unwrap(),
        SandboxState::Suspended
    );
    let (state, sandbox_ref) = row_state(&server, &sid).await;
    assert_eq!(state, "parked");
    assert!(sandbox_ref.is_some(), "suspend keeps the sandbox ref");
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn reaper_forgets_a_sandbox_the_provider_no_longer_has() {
    let server = reaper_server().await;
    let admin = init_admin(&server).await;
    let (sid, handle) = idle_running_session(&server, &admin).await;

    server.sandbox.release(&handle).await.unwrap();
    reap_once(&server.state, &mut ReaperMemory::default())
        .await
        .unwrap();

    assert_eq!(row_state(&server, &sid).await, ("parked".to_owned(), None));
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn reaper_records_a_sandbox_suspended_out_of_band() {
    let server = reaper_server().await;
    let admin = init_admin(&server).await;
    let (sid, handle) = idle_running_session(&server, &admin).await;

    server.sandbox.suspend(&handle).await.unwrap();
    reap_once(&server.state, &mut ReaperMemory::default())
        .await
        .unwrap();

    let (state, sandbox_ref) = row_state(&server, &sid).await;
    assert_eq!(state, "parked");
    assert!(sandbox_ref.is_some());
    let suspended: bool = sqlx::query_scalar(
        "SELECT suspended_at IS NOT NULL FROM chat_sessions WHERE session_id = $1",
    )
    .bind(&sid)
    .fetch_one(&server.db)
    .await
    .unwrap();
    assert!(suspended);
    server.cleanup().await;
}

// ─── recovery, rollback, crashed turns, the manual route's fence ─────────────

/// Make the fake report this sandbox as an hour old, past the orphan pass's minimum age.
fn age(server: &common::TestServer, handle: &SandboxHandle) {
    server.sandbox.set_started_at(
        handle,
        Some(chrono::Utc::now() - chrono::Duration::hours(1)),
    );
}

async fn set_row(server: &common::TestServer, sid: &str, assignments: &str) {
    sqlx::query(&format!(
        "UPDATE chat_sessions SET {assignments} WHERE session_id = $1"
    ))
    .bind(sid)
    .execute(&server.db)
    .await
    .unwrap();
}

async fn lease_is_clear(server: &common::TestServer, sid: &str) -> bool {
    sqlx::query_scalar(
        "SELECT turn_owner IS NULL AND lease_expires_at IS NULL FROM chat_sessions WHERE session_id = $1",
    )
    .bind(sid)
    .fetch_one(&server.db)
    .await
    .unwrap()
}

#[tokio::test]
#[serial]
async fn a_fence_bump_alone_defeats_the_claim() {
    let server = reaper_server().await;
    let admin = init_admin(&server).await;
    let (sid, handle) = idle_running_session(&server, &admin).await;
    let store = SessionStore::new(server.db.clone());
    let listed = store.list_idle(60).await.unwrap();

    // A turn came and went between the list and the claim: no owner, but the fence moved.
    set_row(&server, &sid, "turn_fence = turn_fence + 1").await;

    assert!(
        store
            .begin_suspend(&sid, listed[0].turn_fence)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        server.sandbox.status(&handle).await.unwrap(),
        SandboxState::Running
    );
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_failed_suspend_rolls_the_claim_back_to_running() {
    let server = reaper_server().await;
    let admin = init_admin(&server).await;
    let (sid, handle) = idle_running_session(&server, &admin).await;

    server
        .sandbox
        .fail_next_suspend(RuntimeError::Internal("boom".into()));
    reap_once(&server.state, &mut ReaperMemory::default())
        .await
        .unwrap();

    assert_eq!(row_state(&server, &sid).await.0, "running");
    assert!(lease_is_clear(&server, &sid).await);
    assert_eq!(
        server.sandbox.status(&handle).await.unwrap(),
        SandboxState::Running
    );
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_stuck_stopping_row_is_recovered_once_its_deadline_passes() {
    let server = reaper_server().await;
    let admin = init_admin(&server).await;
    let (sid, handle) = idle_running_session(&server, &admin).await;

    // A claimant died mid-suspend. Inside the deadline the row is left alone.
    set_row(
        &server,
        &sid,
        "runtime_state = 'stopping', lease_expires_at = now() + interval '1 minute'",
    )
    .await;
    reap_once(&server.state, &mut ReaperMemory::default())
        .await
        .unwrap();
    assert_eq!(row_state(&server, &sid).await.0, "stopping");

    set_row(
        &server,
        &sid,
        "lease_expires_at = now() - interval '1 second'",
    )
    .await;
    let outcome = reap_once(&server.state, &mut ReaperMemory::default())
        .await
        .unwrap();

    assert_eq!(
        outcome,
        SweepOutcome::Swept {
            suspended: 1,
            skipped: 0
        }
    );
    assert_eq!(row_state(&server, &sid).await.0, "parked");
    assert_eq!(
        server.sandbox.status(&handle).await.unwrap(),
        SandboxState::Suspended
    );
    assert!(lease_is_clear(&server, &sid).await);
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_sandbox_that_is_not_running_is_suspended_not_assumed_stopped() {
    let server = reaper_server().await;
    let admin = init_admin(&server).await;
    let (sid, handle) = idle_running_session(&server, &admin).await;

    server.sandbox.set_state(&handle, SandboxState::Starting);
    reap_once(&server.state, &mut ReaperMemory::default())
        .await
        .unwrap();

    // The fake's `suspend` is what moves Starting to Suspended, so this proves it was called.
    assert_eq!(
        server.sandbox.status(&handle).await.unwrap(),
        SandboxState::Suspended
    );
    assert_eq!(row_state(&server, &sid).await.0, "parked");
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_crashed_turn_with_a_lapsed_lease_is_reaped() {
    let server = reaper_server().await;
    let admin = init_admin(&server).await;
    let (sid, handle) = idle_running_session(&server, &admin).await;

    set_row(
        &server,
        &sid,
        "turn_owner = 'dead-replica', lease_expires_at = now() - interval '5 minutes'",
    )
    .await;
    reap_once(&server.state, &mut ReaperMemory::default())
        .await
        .unwrap();

    assert_eq!(row_state(&server, &sid).await.0, "parked");
    assert_eq!(
        server.sandbox.status(&handle).await.unwrap(),
        SandboxState::Suspended
    );
    assert!(lease_is_clear(&server, &sid).await);
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn the_manual_suspend_is_refused_while_a_turn_holds_the_lease() {
    let server = reaper_server().await;
    let admin = init_admin(&server).await;
    let (sid, handle) = idle_running_session(&server, &admin).await;
    set_row(
        &server,
        &sid,
        "turn_owner = 'r1', lease_expires_at = now() + interval '1 minute'",
    )
    .await;

    let resp = common::as_superuser(
        server
            .client
            .post(server.url(&format!("/api/sessions/{sid}/suspend"))),
        &admin,
        "admin",
    )
    .send()
    .await
    .unwrap();

    assert_eq!(resp.status(), 409);
    assert_eq!(resp.json::<Value>().await.unwrap()["code"], "session_busy");
    assert_eq!(
        server.sandbox.status(&handle).await.unwrap(),
        SandboxState::Running
    );
    assert_eq!(row_state(&server, &sid).await.0, "running");
    server.cleanup().await;
}

// ─── the claim takes the lease and the fence ─────────────────────────────────

async fn row_lease(server: &common::TestServer, sid: &str) -> (Option<String>, i64, bool) {
    sqlx::query_as(
        "SELECT turn_owner, turn_fence, lease_expires_at > now() FROM chat_sessions WHERE session_id = $1",
    )
    .bind(sid)
    .fetch_one(&server.db)
    .await
    .unwrap()
}

#[tokio::test]
#[serial]
async fn a_claim_strips_the_owner_and_bumps_the_fence_so_the_old_turn_misses() {
    let server = reaper_server().await;
    let admin = init_admin(&server).await;
    let (sid, _handle) = idle_running_session(&server, &admin).await;
    set_row(
        &server,
        &sid,
        "turn_owner = 'dead', turn_fence = 5, lease_expires_at = now() - interval '5 minutes'",
    )
    .await;
    let store = SessionStore::new(server.db.clone());

    let claim = store.begin_suspend(&sid, 5).await.unwrap();

    assert_eq!(claim, Some(6));
    let (owner, fence, deadline_holds) = row_lease(&server, &sid).await;
    assert_eq!(
        (owner, fence, deadline_holds),
        (None, 6, true),
        "begin_suspend stamps the deadline itself"
    );
    // The dead turn wakes up: neither its heartbeat nor its release can disturb the claim.
    let old = TurnLease {
        session_id: sid.clone(),
        owner: "dead".into(),
        fence: 5,
    };
    assert!(!heartbeat(&server.db, &old, 60).await.unwrap());
    release_turn(&server.db, &old).await.unwrap();
    assert_eq!(row_state(&server, &sid).await.0, "stopping");
    assert!(
        row_lease(&server, &sid).await.2,
        "the deadline survived the late release"
    );
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_superseded_claimant_cannot_release_or_settle() {
    let server = reaper_server().await;
    let admin = init_admin(&server).await;
    let (sid, _handle) = idle_running_session(&server, &admin).await;
    let store = SessionStore::new(server.db.clone());

    let first = store.begin_suspend(&sid, 0).await.unwrap().unwrap();
    set_row(
        &server,
        &sid,
        "lease_expires_at = now() - interval '1 second'",
    )
    .await;
    let second = store.reclaim_suspend(&sid, first).await.unwrap().unwrap();
    assert!(second > first);

    assert!(
        !store
            .release_suspend(&sid, first, SessionRuntimeState::Running)
            .await
            .unwrap()
    );
    assert!(!store.settle_suspended(&sid, first).await.unwrap());
    assert_eq!(row_state(&server, &sid).await.0, "stopping");
    assert!(
        store
            .release_suspend(&sid, second, SessionRuntimeState::Running)
            .await
            .unwrap()
    );
    assert_eq!(row_state(&server, &sid).await.0, "running");
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_crashed_turns_own_model_calls_do_not_keep_it_alive() {
    let server = reaper_server().await;
    let admin = init_admin(&server).await;
    let (sid, handle) = idle_running_session(&server, &admin).await;
    set_row(
        &server,
        &sid,
        "turn_owner = 'dead', lease_expires_at = now() - interval '5 minutes'",
    )
    .await;
    // A router call made after the last finished turn but before the lease lapsed: the dead
    // turn's own.
    sqlx::query(
        "INSERT INTO trace_usage (trace_id, agent_name, user_id, agent_id, provider, model, \
         started_at, materialized_at) \
         SELECT gen_random_uuid()::text, 'agent', user_id, agent_id, 'p', 'm', \
         now() - interval '30 minutes', now() - interval '30 minutes' \
         FROM chat_sessions WHERE session_id = $1",
    )
    .bind(&sid)
    .execute(&server.db)
    .await
    .unwrap();

    reap_once(&server.state, &mut ReaperMemory::default())
        .await
        .unwrap();

    assert_eq!(row_state(&server, &sid).await.0, "parked");
    assert_eq!(
        server.sandbox.status(&handle).await.unwrap(),
        SandboxState::Suspended
    );
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn stuck_claims_are_recovered_without_the_idle_sweep() {
    let server = reaper_server().await;
    let admin = init_admin(&server).await;
    let (stuck, stuck_handle) = idle_running_session(&server, &admin).await;
    let (idle, idle_handle) = idle_running_session(&server, &admin).await;
    let store = SessionStore::new(server.db.clone());
    store.begin_suspend(&stuck, 0).await.unwrap().unwrap();
    set_row(
        &server,
        &stuck,
        "lease_expires_at = now() - interval '1 second'",
    )
    .await;

    // The function `spawn_reaper` calls on every tick, with idle suspension off (TTL 0).
    sweep(
        &server.state,
        &mut ReaperMemory::default(),
        SweepScope::RecoveryOnly,
    )
    .await
    .unwrap();

    assert_eq!(row_state(&server, &stuck).await.0, "parked");
    assert_eq!(
        server.sandbox.status(&stuck_handle).await.unwrap(),
        SandboxState::Suspended
    );
    assert_eq!(
        row_state(&server, &idle).await.0,
        "running",
        "the idle sweep did not run"
    );
    assert_eq!(
        server.sandbox.status(&idle_handle).await.unwrap(),
        SandboxState::Running
    );
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn the_manual_suspend_reports_a_live_claim_and_finishes_a_lapsed_one() {
    let server = reaper_server().await;
    let admin = init_admin(&server).await;
    let (sid, handle) = idle_running_session(&server, &admin).await;
    let store = SessionStore::new(server.db.clone());
    store.begin_suspend(&sid, 0).await.unwrap().unwrap();
    let suspend = || {
        common::as_superuser(
            server
                .client
                .post(server.url(&format!("/api/sessions/{sid}/suspend"))),
            &admin,
            "admin",
        )
        .send()
    };

    let live = suspend().await.unwrap();
    assert_eq!(live.status(), 202);
    assert_eq!(
        live.json::<Value>().await.unwrap()["data"]["status"],
        "suspending"
    );
    assert_eq!(
        server.sandbox.status(&handle).await.unwrap(),
        SandboxState::Running
    );

    set_row(
        &server,
        &sid,
        "lease_expires_at = now() - interval '1 second'",
    )
    .await;
    assert_eq!(suspend().await.unwrap().status(), 204);
    assert_eq!(row_state(&server, &sid).await.0, "parked");
    assert_eq!(
        server.sandbox.status(&handle).await.unwrap(),
        SandboxState::Suspended
    );
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_vanished_sandbox_clears_the_ref_on_a_stuck_claim() {
    let server = reaper_server().await;
    let admin = init_admin(&server).await;
    let (sid, handle) = idle_running_session(&server, &admin).await;
    let store = SessionStore::new(server.db.clone());
    store.begin_suspend(&sid, 0).await.unwrap().unwrap();
    set_row(
        &server,
        &sid,
        "lease_expires_at = now() - interval '1 second'",
    )
    .await;
    server.sandbox.release(&handle).await.unwrap();

    recover_stuck(&server.state, &mut ReaperMemory::default())
        .await
        .unwrap();

    assert_eq!(row_state(&server, &sid).await, ("parked".to_owned(), None));
    server.cleanup().await;
}

// ─── bounded model-call lookback, claim restore, fenced turn writes ──────────

/// A model call attributed to one of `sid`'s own flows, `age` ago: a `flows` row naming the
/// session, and the `trace_usage` row the router would write for that flow.
async fn insert_usage_for(server: &common::TestServer, sid: &str, age: &str) {
    let flow_id = format!("flow-{}", Uuid::new_v4());
    sqlx::query(
        "INSERT INTO flows (flow_id, user_id, root_agent_id, status, session_id) \
         SELECT $2, user_id, agent_id, 'completed', session_id \
         FROM chat_sessions WHERE session_id = $1",
    )
    .bind(sid)
    .bind(&flow_id)
    .execute(&server.db)
    .await
    .unwrap();
    sqlx::query(&format!(
        "INSERT INTO trace_usage (trace_id, agent_name, user_id, agent_id, provider, model, \
         started_at, materialized_at) \
         SELECT $2, 'agent', user_id, agent_id, 'p', 'm', now() - interval '{age}', \
         now() - interval '{age}' \
         FROM chat_sessions WHERE session_id = $1"
    ))
    .bind(sid)
    .bind(&flow_id)
    .execute(&server.db)
    .await
    .unwrap();
}

#[tokio::test]
#[serial]
async fn a_crashed_turn_whose_first_suspend_failed_is_reaped_on_a_later_sweep() {
    let server = reaper_server().await;
    let admin = init_admin(&server).await;
    let (sid, handle) = idle_running_session(&server, &admin).await;
    set_row(
        &server,
        &sid,
        "turn_owner = 'dead', lease_expires_at = now() - interval '5 minutes'",
    )
    .await;
    insert_usage_for(&server, &sid, "30 minutes").await;
    let mut memory = ReaperMemory::default();

    server
        .sandbox
        .fail_next_suspend(RuntimeError::Internal("transient".into()));
    reap_once(&server.state, &mut memory).await.unwrap();
    assert_eq!(row_state(&server, &sid).await.0, "running");

    // The claim cleared the dead owner; the dead turn's old call must not pin the session now.
    reap_once(&server.state, &mut memory).await.unwrap();
    assert_eq!(row_state(&server, &sid).await.0, "parked");
    assert_eq!(
        server.sandbox.status(&handle).await.unwrap(),
        SandboxState::Suspended
    );
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn an_active_sibling_session_does_not_pin_an_idle_one() {
    let server = reaper_server().await;
    let admin = init_admin(&server).await;
    let (a, a_handle) = idle_running_session(&server, &admin).await;
    let (b, _) = idle_running_session(&server, &admin).await;
    insert_usage_for(&server, &b, "10 seconds").await;

    reap_once(&server.state, &mut ReaperMemory::default())
        .await
        .unwrap();

    assert_eq!(row_state(&server, &a).await.0, "parked");
    assert_eq!(
        server.sandbox.status(&a_handle).await.unwrap(),
        SandboxState::Suspended
    );
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_model_call_inside_the_idle_ttl_blocks_the_suspend() {
    let server = reaper_server().await;
    let admin = init_admin(&server).await;
    let (sid, handle) = idle_running_session(&server, &admin).await;
    insert_usage_for(&server, &sid, "10 seconds").await;

    reap_once(&server.state, &mut ReaperMemory::default())
        .await
        .unwrap();

    assert_eq!(row_state(&server, &sid).await.0, "running");
    assert_eq!(
        server.sandbox.status(&handle).await.unwrap(),
        SandboxState::Running
    );
    server.cleanup().await;
}

// ─── a question lives only as long as its turn ──────────────────────────────

/// A question the session's harness asked `age` ago and nobody has answered: the
/// `hitl_requests` row a turn's permission delegate records, here one a crashed turn left open.
/// Asked before the session went idle (an hour ago, see [`idle_running_session`]) whenever `age`
/// is longer than that.
async fn insert_open_question(server: &common::TestServer, sid: &str, age: &str) -> Uuid {
    sqlx::query_scalar(&format!(
        "INSERT INTO hitl_requests (kind, origin, agent_id, owner_user_id, context_id, \
         chat_session_id, tool_name, question, created_at) \
         SELECT 'tool_approval', 'coding_agent', agent_id, user_id, session_id, session_id, \
         'bash', '{{}}'::jsonb, now() - interval '{age}' \
         FROM chat_sessions WHERE session_id = $1 RETURNING id"
    ))
    .bind(sid)
    .fetch_one(&server.db)
    .await
    .unwrap()
}

async fn question_status(server: &common::TestServer, id: Uuid) -> String {
    sqlx::query_scalar("SELECT status FROM hitl_requests WHERE id = $1")
        .bind(id)
        .fetch_one(&server.db)
        .await
        .unwrap()
}

#[tokio::test]
#[serial]
async fn the_reapers_suspend_closes_the_questions_a_crashed_turn_left_open() {
    let server = reaper_server().await;
    let admin = init_admin(&server).await;
    let (sid, handle) = idle_running_session(&server, &admin).await;
    let question = insert_open_question(&server, &sid, "2 hours").await;

    reap_once(&server.state, &mut ReaperMemory::default())
        .await
        .unwrap();

    // The open question did not keep the idle session up, and the suspend closed it.
    assert_eq!(row_state(&server, &sid).await.0, "parked");
    assert_eq!(
        server.sandbox.status(&handle).await.unwrap(),
        SandboxState::Suspended
    );
    assert_eq!(question_status(&server, question).await, "expired");
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn the_manual_suspend_closes_the_questions_a_crashed_turn_left_open() {
    let server = reaper_server().await;
    let admin = init_admin(&server).await;
    let (sid, _) = idle_running_session(&server, &admin).await;
    let question = insert_open_question(&server, &sid, "2 hours").await;

    let resp = common::as_superuser(
        server
            .client
            .post(server.url(&format!("/api/sessions/{sid}/suspend"))),
        &admin,
        "admin",
    )
    .send()
    .await
    .unwrap();

    assert_eq!(resp.status(), 204);
    assert_eq!(question_status(&server, question).await, "expired");
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_turn_closes_only_its_own_unanswered_questions_and_only_once() {
    let server = reaper_server().await;
    let admin = init_admin(&server).await;
    let (sid, _) = idle_running_session(&server, &admin).await;
    let unanswered = insert_open_question(&server, &sid, "1 minute").await;
    let answered = insert_open_question(&server, &sid, "2 minutes").await;
    let not_this_turns = insert_open_question(&server, &sid, "3 minutes").await;
    sqlx::query("UPDATE hitl_requests SET status = 'resolved' WHERE id = $1")
        .bind(answered)
        .execute(&server.db)
        .await
        .unwrap();
    let turn = TurnQuestions::default();
    turn.record(unanswered);
    turn.record(answered);

    assert_eq!(turn.close_open(&server.db).await.unwrap(), 1);
    assert_eq!(turn.close_open(&server.db).await.unwrap(), 0, "idempotent");

    assert_eq!(question_status(&server, unanswered).await, "expired");
    assert_eq!(question_status(&server, answered).await, "resolved");
    assert_eq!(question_status(&server, not_this_turns).await, "pending");
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_failed_suspend_restores_a_starting_session_to_starting() {
    let server = reaper_server().await;
    let admin = init_admin(&server).await;
    let (sid, _handle) = idle_running_session(&server, &admin).await;
    set_row(&server, &sid, "runtime_state = 'starting'").await;

    server
        .sandbox
        .fail_next_suspend(RuntimeError::Internal("boom".into()));
    reap_once(&server.state, &mut ReaperMemory::default())
        .await
        .unwrap();

    assert_eq!(row_state(&server, &sid).await.0, "starting");
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn turn_writes_fenced_on_the_old_lease_miss_after_a_claim() {
    let server = reaper_server().await;
    let admin = init_admin(&server).await;
    let (sid, _handle) = idle_running_session(&server, &admin).await;
    let store = SessionStore::new(server.db.clone());
    store.begin_suspend(&sid, 0).await.unwrap().unwrap();
    let old = TurnLease {
        session_id: sid.clone(),
        owner: "r1".into(),
        fence: 0,
    };

    assert!(
        !store
            .set_runtime_state_fenced(&old, SessionRuntimeState::Running)
            .await
            .unwrap()
    );
    assert!(!store.set_sandbox_ref_fenced(&old, "fake:x").await.unwrap());
    assert!(
        !nasiko_server::coding::lifecycle::record_acp_session_fenced(&server.db, &old, "acp-1")
            .await
            .unwrap()
    );
    let acp: Option<String> =
        sqlx::query_scalar("SELECT acp_session_id FROM chat_sessions WHERE session_id = $1")
            .bind(&sid)
            .fetch_one(&server.db)
            .await
            .unwrap();
    assert_eq!(acp, None);
    assert_eq!(row_state(&server, &sid).await.0, "stopping");
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_stuck_claim_with_an_unusable_ref_is_cleared() {
    let server = reaper_server().await;
    let admin = init_admin(&server).await;
    let (sid, _handle) = idle_running_session(&server, &admin).await;
    set_row(
        &server,
        &sid,
        "runtime_state = 'stopping', sandbox_ref = 'pod:gone', \
         lease_expires_at = now() - interval '1 second'",
    )
    .await;

    recover_stuck(&server.state, &mut ReaperMemory::default())
        .await
        .unwrap();

    assert_eq!(row_state(&server, &sid).await, ("parked".to_owned(), None));
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_manual_suspend_on_a_starting_session_without_a_sandbox_is_busy() {
    let server = reaper_server().await;
    let admin = init_admin(&server).await;
    let sid = &parked_session(&server, &admin, "opencode").await;
    set_row(
        &server,
        sid,
        "runtime_state = 'starting', turn_owner = 'r1', \
         lease_expires_at = now() + interval '1 minute'",
    )
    .await;

    let resp = common::as_superuser(
        server
            .client
            .post(server.url(&format!("/api/sessions/{sid}/suspend"))),
        &admin,
        "admin",
    )
    .send()
    .await
    .unwrap();

    assert_eq!(resp.status(), 409);
    server.cleanup().await;
}

// ─── grace inside the TTL, orphans, revoke before settle ─────────────────────

#[tokio::test]
#[serial]
async fn a_session_idle_for_the_ttl_is_suspended_on_the_first_quiet_sweep() {
    let server = sessions_server_with(|c| {
        c.coding_agent_idle_ttl_secs = 60;
        c.session_idle_grace_secs = 2;
        c.session_cpu_quiescent_millicores = 20;
    })
    .await;
    let admin = init_admin(&server).await;
    let (sid, handle) = idle_running_session(&server, &admin).await;

    reap_once(&server.state, &mut ReaperMemory::default())
        .await
        .unwrap();

    assert_eq!(row_state(&server, &sid).await.0, "parked");
    assert_eq!(
        server.sandbox.status(&handle).await.unwrap(),
        SandboxState::Suspended
    );
    server.cleanup().await;
}

/// Agent parking and session suspension are separate switches: with `AGENT_PARK_IDLE_SECS=0` a
/// sweep leaves an idle agent's workload up, and still suspends an idle session.
#[tokio::test]
#[serial]
async fn with_agent_parking_off_a_sweep_parks_no_agent_but_still_suspends_idle_sessions() {
    let server = sessions_server_with(|c| {
        c.coding_agent_idle_ttl_secs = 60;
        c.session_idle_grace_secs = 0;
        c.session_cpu_quiescent_millicores = 20;
        c.agent_park_idle_secs = 0;
    })
    .await;
    let admin = init_admin(&server).await;
    let (sid, handle) = idle_running_session(&server, &admin).await;
    let agent: Uuid = sqlx::query_scalar(
        "INSERT INTO agents (name, owner_id, image, status, workload_kind, parking_policy, \
                             last_request_at, updated_at) \
         VALUES ('idle-agent', $1, 'x:1.0.0', 'running', 'agent', 'auto', \
                 now() - interval '2 hours', now() - interval '2 hours') RETURNING id",
    )
    .bind(Uuid::parse_str(&admin).unwrap())
    .fetch_one(&server.db)
    .await
    .unwrap();

    sweep(
        &server.state,
        &mut ReaperMemory::default(),
        SweepScope::Full,
    )
    .await
    .unwrap();

    assert_eq!(row_state(&server, &sid).await.0, "parked");
    assert_eq!(
        server.sandbox.status(&handle).await.unwrap(),
        SandboxState::Suspended
    );
    let agent_status: String = sqlx::query_scalar("SELECT status FROM agents WHERE id = $1")
        .bind(agent)
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert_eq!(agent_status, "running");
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_busy_sample_restarts_the_grace_window() {
    let server = sessions_server_with(|c| {
        c.coding_agent_idle_ttl_secs = 60;
        c.session_idle_grace_secs = 2;
        c.session_cpu_quiescent_millicores = 20;
    })
    .await;
    let admin = init_admin(&server).await;
    let (sid, handle) = idle_running_session(&server, &admin).await;
    let mut memory = ReaperMemory::default();

    server.sandbox.set_stats(&handle, cpu(500));
    reap_once(&server.state, &mut memory).await.unwrap();
    server.sandbox.set_stats(&handle, cpu(1));
    reap_once(&server.state, &mut memory).await.unwrap();

    // Quiet again, but only just: the run restarted, so the 2 s grace has not elapsed.
    assert_eq!(row_state(&server, &sid).await.0, "running");
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_sandbox_of_a_deleted_session_is_released() {
    let server = reaper_server().await;
    let admin = init_admin(&server).await;
    let (sid, handle) = idle_running_session(&server, &admin).await;
    age(&server, &handle);
    set_row(&server, &sid, "deleted_at = now()").await;

    recover_orphans(&server.state).await.unwrap();

    assert_eq!(
        server.sandbox.status(&handle).await.unwrap(),
        SandboxState::Absent
    );
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_sandbox_with_no_session_row_is_released() {
    let server = reaper_server().await;
    let key = SandboxKey::Session {
        session_id: Uuid::new_v4(),
    };
    let handle = acquire_fake(&server, key).await;
    age(&server, &handle);

    recover_orphans(&server.state).await.unwrap();

    assert_eq!(
        server.sandbox.status(&handle).await.unwrap(),
        SandboxState::Absent
    );
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_live_sandbox_under_a_parked_row_is_suspended_and_recorded() {
    let server = reaper_server().await;
    let admin = init_admin(&server).await;
    // Parked, no sandbox_ref (a failed start that left the sandbox up), sandbox Running.
    let sid = parked_session(&server, &admin, "opencode").await;
    let handle = acquire_fake(
        &server,
        SandboxKey::Session {
            session_id: Uuid::parse_str(&sid).unwrap(),
        },
    )
    .await;

    age(&server, &handle);
    recover_orphans(&server.state).await.unwrap();

    assert_eq!(
        server.sandbox.status(&handle).await.unwrap(),
        SandboxState::Suspended
    );
    let (state, sandbox_ref) = row_state(&server, &sid).await;
    assert_eq!(state, "parked");
    assert_eq!(sandbox_ref.as_deref(), Some(handle.sandbox_ref().as_str()));
    assert!(lease_is_clear(&server, &sid).await);
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_live_sandbox_under_a_leased_row_is_left_alone() {
    let server = reaper_server().await;
    let admin = init_admin(&server).await;
    let (sid, handle) = idle_running_session(&server, &admin).await;
    age(&server, &handle);
    set_row(
        &server,
        &sid,
        "runtime_state = 'parked', turn_owner = 'r1', lease_expires_at = now() + interval '1 minute'",
    )
    .await;

    recover_orphans(&server.state).await.unwrap();

    assert_eq!(
        server.sandbox.status(&handle).await.unwrap(),
        SandboxState::Running
    );
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn the_manual_suspend_of_a_parked_row_with_a_running_sandbox_suspends_it() {
    let server = reaper_server().await;
    let admin = init_admin(&server).await;
    let (sid, handle) = idle_running_session(&server, &admin).await;
    set_row(&server, &sid, "runtime_state = 'parked'").await;

    let resp = common::as_superuser(
        server
            .client
            .post(server.url(&format!("/api/sessions/{sid}/suspend"))),
        &admin,
        "admin",
    )
    .send()
    .await
    .unwrap();

    assert_eq!(resp.status(), 204);
    assert_eq!(
        server.sandbox.status(&handle).await.unwrap(),
        SandboxState::Suspended
    );
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_vanished_sandbox_has_its_token_revoked_before_the_row_is_cleared() {
    let server = reaper_server().await;
    let admin = init_admin(&server).await;
    let (sid, handle) = idle_running_session(&server, &admin).await;
    set_row(&server, &sid, "sandbox_token_jti = 'jti-vanished'").await;
    server.sandbox.release(&handle).await.unwrap();

    reap_once(&server.state, &mut ReaperMemory::default())
        .await
        .unwrap();

    assert_eq!(row_state(&server, &sid).await, ("parked".to_owned(), None));
    let revoked: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM revoked_session_tokens WHERE jti = 'jti-vanished')",
    )
    .fetch_one(&server.db)
    .await
    .unwrap();
    assert!(revoked);
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_first_turns_sandbox_of_a_free_text_session_survives_the_orphan_pass() {
    let server = reaper_server().await;
    let admin = Uuid::parse_str(&init_admin(&server).await).unwrap();
    let agent = own_harness_agent(&server, admin, "opencode").await;
    // A client-minted contextId, as a first message creates its session: its sandbox key is a
    // hash, not the id.
    let sid = "ses_free_text_1";
    let key = SandboxKey::Session {
        session_id: SessionStore::new(server.db.clone())
            .find_or_create_for_harness(agent, admin, sid)
            .await
            .unwrap()
            .unwrap()
            .sandbox_uuid(),
    };
    let handle = acquire_fake(&server, key.clone()).await;
    server.sandbox.set_state(&handle, SandboxState::Starting);
    server.sandbox.set_started_at(&handle, None);
    // The turn holds the lease and has recorded only the key, not the ref yet.
    set_row(
        &server,
        sid,
        &format!(
            "runtime_state = 'starting', turn_owner = 'r1', sandbox_key = '{}', \
             lease_expires_at = now() + interval '1 minute'",
            key.name()
        ),
    )
    .await;

    recover_orphans(&server.state).await.unwrap();

    assert_eq!(
        server.sandbox.status(&handle).await.unwrap(),
        SandboxState::Starting
    );
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_young_sandbox_with_no_row_survives_and_an_old_one_is_released() {
    let server = reaper_server().await;
    let young = acquire_fake(
        &server,
        SandboxKey::Session {
            session_id: Uuid::new_v4(),
        },
    )
    .await;
    let old = acquire_fake(
        &server,
        SandboxKey::Session {
            session_id: Uuid::new_v4(),
        },
    )
    .await;
    age(&server, &old);

    recover_orphans(&server.state).await.unwrap();

    assert_eq!(
        server.sandbox.status(&young).await.unwrap(),
        SandboxState::Running
    );
    assert_eq!(
        server.sandbox.status(&old).await.unwrap(),
        SandboxState::Absent
    );
    server.cleanup().await;
}

// ─── list and delete ─────────────────────────────────────────────────────────

async fn list_as_admin(server: &common::TestServer, admin: &str, query: &str) -> Value {
    let resp = common::as_superuser(
        server
            .client
            .get(server.url(&format!("/api/sessions{query}"))),
        admin,
        "admin",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(resp.status(), 200, "{query}");
    resp.json().await.unwrap()
}

fn session_ids(page: &Value) -> Vec<String> {
    page["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["session_id"].as_str().unwrap().to_owned())
        .collect()
}

async fn delete_as_admin(server: &common::TestServer, admin: &str, sid: &str) -> reqwest::Response {
    common::as_superuser(
        server
            .client
            .delete(server.url(&format!("/api/sessions/{sid}"))),
        admin,
        "admin",
    )
    .send()
    .await
    .unwrap()
}

#[tokio::test]
#[serial]
async fn list_shows_only_the_callers_sessions_filtered_and_paged() {
    let server = sessions_server().await;
    let admin = init_admin(&server).await;
    let bob = create_member(&server, &admin).await;

    let oldest = parked_session(&server, &admin, "opencode").await;
    let middle = parked_session(&server, &admin, "codex").await;
    let newest = parked_session(&server, &admin, "opencode").await;
    set_row(
        &server,
        &oldest,
        "last_turn_at = now() - interval '3 hours'",
    )
    .await;
    set_row(
        &server,
        &middle,
        "last_turn_at = now() - interval '2 hours'",
    )
    .await;
    set_row(&server, &newest, "runtime_state = 'running'").await;
    parked_session(&server, &bob, "opencode").await;

    let all = list_as_admin(&server, &admin, "").await;
    assert_eq!(
        session_ids(&all),
        [newest.clone(), middle.clone(), oldest.clone()]
    );
    assert_eq!(all["has_more"], false);
    assert!(all["next_cursor"].is_null());
    assert_eq!(all["data"][0]["harness"], "opencode");

    let by_harness = list_as_admin(&server, &admin, "?harness=codex").await;
    assert_eq!(session_ids(&by_harness), std::slice::from_ref(&middle));
    let by_state = list_as_admin(&server, &admin, "?state=running").await;
    assert_eq!(session_ids(&by_state), std::slice::from_ref(&newest));

    let first = list_as_admin(&server, &admin, "?limit=2").await;
    assert_eq!(session_ids(&first), [newest.clone(), middle.clone()]);
    assert_eq!(first["has_more"], true);
    let cursor = first["next_cursor"].as_str().unwrap();
    let second = list_as_admin(&server, &admin, &format!("?limit=2&cursor={cursor}")).await;
    assert_eq!(session_ids(&second), [oldest]);
    assert_eq!(second["has_more"], false);

    let bad = common::as_superuser(
        server.client.get(server.url("/api/sessions?limit=0")),
        &admin,
        "admin",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(bad.status(), 400);

    let foreign = common::as_member(
        server
            .client
            .get(server.url(&format!("/api/sessions?user_id={admin}"))),
        &bob,
        "bob",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(foreign.status(), 403);
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn deleting_a_parked_session_releases_its_sandbox_revokes_its_token_and_removes_the_row() {
    let server = sessions_server().await;
    let admin = init_admin(&server).await;
    let sid = &parked_session(&server, &admin, "codex").await;
    let key = SandboxKey::Session {
        session_id: Uuid::parse_str(sid).unwrap(),
    };
    let handle = acquire_fake(&server, key).await;
    set_row(
        &server,
        sid,
        &format!(
            "sandbox_ref = '{}', sandbox_token_jti = 'jti-rm'",
            handle.sandbox_ref()
        ),
    )
    .await;

    let resp = delete_as_admin(&server, &admin, sid).await;
    assert_eq!(resp.status(), 204);

    assert_eq!(
        server.sandbox.status(&handle).await.unwrap(),
        SandboxState::Absent
    );
    let revoked: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM revoked_session_tokens WHERE jti = 'jti-rm')",
    )
    .fetch_one(&server.db)
    .await
    .unwrap();
    assert!(revoked);
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM chat_sessions WHERE session_id = $1")
        .bind(sid)
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert_eq!(rows, 0);
    assert_eq!(delete_as_admin(&server, &admin, sid).await.status(), 404);
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn deleting_during_a_live_lease_is_busy_and_changes_nothing() {
    let server = reaper_server().await;
    let admin = init_admin(&server).await;
    let (sid, handle) = idle_running_session(&server, &admin).await;
    set_row(
        &server,
        &sid,
        "turn_owner = 'r1', lease_expires_at = now() + interval '1 minute'",
    )
    .await;

    let resp = delete_as_admin(&server, &admin, &sid).await;

    assert_eq!(resp.status(), 409);
    assert_eq!(resp.json::<Value>().await.unwrap()["code"], "session_busy");
    assert_eq!(
        server.sandbox.status(&handle).await.unwrap(),
        SandboxState::Running
    );
    assert_eq!(row_state(&server, &sid).await.0, "running");
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn deleting_a_foreign_session_is_a_404_and_leaves_it_alone() {
    let server = sessions_server().await;
    let admin = init_admin(&server).await;
    let bob = create_member(&server, &admin).await;
    let sid = parked_session(&server, &admin, "codex").await;

    let resp = common::as_member(
        server
            .client
            .delete(server.url(&format!("/api/sessions/{sid}"))),
        &bob,
        "bob",
    )
    .send()
    .await
    .unwrap();

    assert_eq!(resp.status(), 404);
    assert_eq!(
        resp.json::<Value>().await.unwrap()["code"],
        "session_not_found"
    );
    assert_eq!(row_state(&server, &sid).await.0, "parked");
    server.cleanup().await;
}

/// `HARNESS_SESSIONS_ENABLED` off, on a server that could otherwise run sessions: no harness is
/// listed, and opening a session is refused `harness_sessions_disabled` with nothing written.
#[tokio::test]
#[serial]
async fn with_sessions_switched_off_the_harnesses_are_hidden_and_sessions_refused() {
    let server = sessions_server_with(|c| c.harness_sessions_enabled = false).await;
    let admin = init_admin(&server).await;

    assert_eq!(catalog(&server, &admin).await, json!([]));
    let resp = post_session(&server, &admin, "codex").await;
    assert_eq!(resp.status(), 403);
    assert_eq!(
        resp.json::<Value>().await.unwrap()["code"],
        "harness_sessions_disabled"
    );
    assert_nothing_written(&server).await;
    server.cleanup().await;
}

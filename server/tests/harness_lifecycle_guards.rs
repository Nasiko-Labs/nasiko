//! Lifecycle guards that keep provider-backed harness sessions and the legacy exec-based
//! coding sessions from stepping on each other: the startup reconciler must leave a session
//! with a `sandbox_ref` to the sessions reaper, and a session whose sandbox is being suspended
//! must not be acquirable for a turn.
//!
//! Requires infra (Postgres :5432); skips when Postgres is unreachable:
//!   cargo test -p nasiko-server --test harness_lifecycle_guards -- --test-threads=1

mod common;

use std::collections::HashSet;

use nasiko_server::coding::lifecycle::{
    Beat, TurnLease, acquire_turn, heartbeat_retrying, reconcile_startup, release_turn,
};
use nasiko_server::sessions::pending::{clear_pending, set_pending};
use nasiko_server::sessions::token::{
    load_sandbox_token, revoke_sandbox_token_for_turn, store_sandbox_token,
};
use serial_test::serial;
use sqlx::PgPool;
use uuid::Uuid;

async fn insert_session(
    db: &PgPool,
    state: &str,
    handle: Option<&str>,
    sandbox: Option<&str>,
) -> String {
    let user: Uuid =
        sqlx::query_scalar("INSERT INTO users (username, email) VALUES ($1, $2) RETURNING id")
            .bind(format!("u{}", Uuid::new_v4().simple()))
            .bind(format!("{}@test.local", Uuid::new_v4().simple()))
            .fetch_one(db)
            .await
            .unwrap();
    let session_id = format!("ses_{}", Uuid::new_v4().simple());
    sqlx::query(
        "INSERT INTO chat_sessions (session_id, user_id, title, runtime_state, runtime_handle, sandbox_ref) \
         VALUES ($1, $2, 't', $3, $4, $5)",
    )
    .bind(&session_id)
    .bind(user)
    .bind(state)
    .bind(handle)
    .bind(sandbox)
    .execute(db)
    .await
    .unwrap();
    session_id
}

async fn state_of(db: &PgPool, session_id: &str) -> String {
    sqlx::query_scalar("SELECT runtime_state FROM chat_sessions WHERE session_id = $1")
        .bind(session_id)
        .fetch_one(db)
        .await
        .unwrap()
}

#[tokio::test]
#[serial]
async fn reconciler_parks_dead_exec_rows_but_leaves_sandbox_backed_rows() {
    let server = common::TestServer::start().await;
    let legacy = insert_session(&server.db, "running", Some("gone-container"), None).await;
    let harness = insert_session(&server.db, "running", None, Some("fake:fake-x")).await;

    // A harness session mid-acquire: `starting`, no sandbox yet, a live lease.
    let acquiring = insert_session(&server.db, "starting", None, None).await;
    make_harness_session(&server.db, &acquiring, true).await;
    let acquiring_fence = fence_of(&server.db, &acquiring).await;
    // A legacy session whose lease is live is not the reconciler's either; one whose lease has
    // lapsed is, and its fence moves on.
    let live_legacy = insert_session(&server.db, "running", Some("gone-container"), None).await;
    sqlx::query("UPDATE chat_sessions SET turn_owner = 'r', lease_expires_at = now() + interval '1 minute' WHERE session_id = $1")
        .bind(&live_legacy)
        .execute(&server.db)
        .await
        .unwrap();
    let legacy_fence = fence_of(&server.db, &legacy).await;

    reconcile_startup(&server.db, &HashSet::new())
        .await
        .unwrap();

    assert_eq!(state_of(&server.db, &legacy).await, "parked");
    assert_eq!(fence_of(&server.db, &legacy).await, legacy_fence + 1);
    assert_eq!(state_of(&server.db, &live_legacy).await, "running");
    assert_eq!(state_of(&server.db, &harness).await, "running");
    assert_eq!(state_of(&server.db, &acquiring).await, "starting");
    assert_eq!(fence_of(&server.db, &acquiring).await, acquiring_fence);
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_session_being_suspended_cannot_take_a_turn_until_it_is_parked() {
    let server = common::TestServer::start().await;
    let sid = insert_session(&server.db, "stopping", None, Some("fake:fake-x")).await;

    let lease = acquire_turn(&server.db, &sid, "replica-a", "turn-1", 60)
        .await
        .unwrap();
    assert!(lease.is_none(), "a stopping session is not acquirable");

    sqlx::query("UPDATE chat_sessions SET runtime_state = 'parked' WHERE session_id = $1")
        .bind(&sid)
        .execute(&server.db)
        .await
        .unwrap();
    let lease = acquire_turn(&server.db, &sid, "replica-a", "turn-1", 60)
        .await
        .unwrap();
    assert!(lease.is_some(), "once parked it can be acquired");
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn deleting_a_session_revokes_its_sandbox_token() {
    let server = common::TestServer::start().await;
    let sid = insert_session(&server.db, "parked", None, None).await;
    let owner: Uuid = sqlx::query_scalar("SELECT user_id FROM chat_sessions WHERE session_id = $1")
        .bind(&sid)
        .fetch_one(&server.db)
        .await
        .unwrap();
    sqlx::query("UPDATE chat_sessions SET sandbox_token_jti = 'jti-del' WHERE session_id = $1")
        .bind(&sid)
        .execute(&server.db)
        .await
        .unwrap();

    let resp = common::as_member(
        server
            .client
            .delete(server.url(&format!("/api/chat/sessions/{sid}"))),
        &owner.to_string(),
        "owner",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(resp.status(), 204);
    let revoked: i64 =
        sqlx::query_scalar("SELECT count(*) FROM revoked_session_tokens WHERE jti = 'jti-del'")
            .fetch_one(&server.db)
            .await
            .unwrap();
    assert_eq!(revoked, 1);
    server.cleanup().await;
}

async fn fence_of(db: &PgPool, session_id: &str) -> i64 {
    sqlx::query_scalar("SELECT turn_fence FROM chat_sessions WHERE session_id = $1")
        .bind(session_id)
        .fetch_one(db)
        .await
        .unwrap()
}

/// Point a session at a harness agent, with a live or a lapsed turn lease.
async fn make_harness_session(db: &PgPool, session_id: &str, live_lease: bool) {
    // The agent has its own owner, so the session's user can be deleted without owning agents.
    let owner: Uuid =
        sqlx::query_scalar("INSERT INTO users (username, email) VALUES ($1, $2) RETURNING id")
            .bind(format!("ao{}", Uuid::new_v4().simple()))
            .bind(format!("{}@test.local", Uuid::new_v4().simple()))
            .fetch_one(db)
            .await
            .unwrap();
    let agent: Uuid = sqlx::query_scalar(
        "INSERT INTO agents (name, owner_id, status, workload_kind, harness_id) \
         VALUES ($1, $2, 'running', 'harness', 'opencode') RETURNING id",
    )
    .bind(format!("h-{}", Uuid::new_v4().simple()))
    .bind(owner)
    .fetch_one(db)
    .await
    .unwrap();
    let lease = if live_lease { "1 minute" } else { "-1 minute" };
    sqlx::query(&format!(
        "UPDATE chat_sessions SET agent_id = $2, turn_owner = 'r', \
         lease_expires_at = now() + interval '{lease}' WHERE session_id = $1"
    ))
    .bind(session_id)
    .bind(agent)
    .execute(db)
    .await
    .unwrap();
}

async fn live_lease(db: &PgPool, session_id: &str) -> TurnLease {
    acquire_turn(db, session_id, "replica-a", "t1", 60)
        .await
        .unwrap()
        .expect("the session is free")
}

async fn is_revoked(db: &PgPool, jti: &str) -> bool {
    sqlx::query_scalar::<_, i64>("SELECT count(*) FROM revoked_session_tokens WHERE jti = $1")
        .bind(jti)
        .fetch_one(db)
        .await
        .unwrap()
        > 0
}

#[tokio::test]
#[serial]
async fn a_stale_turn_cannot_revoke_the_token_its_successor_stored() {
    let server = common::TestServer::start().await;
    let sid = insert_session(&server.db, "running", None, None).await;
    let agent = Uuid::new_v4();
    let old = live_lease(&server.db, &sid).await;
    // The old turn's lease lapses and a successor takes the session and stores its own token.
    sqlx::query("UPDATE chat_sessions SET lease_expires_at = now() - interval '1 second' WHERE session_id = $1")
        .bind(&sid)
        .execute(&server.db)
        .await
        .unwrap();
    let new = acquire_turn(&server.db, &sid, "replica-b", "t2", 60)
        .await
        .unwrap()
        .expect("the lapsed lease is takeable");
    assert!(
        store_sandbox_token(&server.db, &new, agent, "tok-new", "jti-new")
            .await
            .unwrap()
    );

    // The stale turn's revoke, expecting the token it once loaded, does nothing.
    assert!(
        !revoke_sandbox_token_for_turn(&server.db, &old, "jti-old")
            .await
            .unwrap()
    );
    assert!(
        !revoke_sandbox_token_for_turn(&server.db, &old, "jti-new")
            .await
            .unwrap()
    );
    assert!(!is_revoked(&server.db, "jti-new").await);
    // The current holder revoking a token it did not load is also refused; the right one works.
    assert!(
        !revoke_sandbox_token_for_turn(&server.db, &new, "jti-other")
            .await
            .unwrap()
    );
    assert!(
        revoke_sandbox_token_for_turn(&server.db, &new, "jti-new")
            .await
            .unwrap()
    );
    assert!(is_revoked(&server.db, "jti-new").await);
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_superseded_store_writes_nothing() {
    let server = common::TestServer::start().await;
    let sid = insert_session(&server.db, "running", None, None).await;
    let stale = TurnLease {
        session_id: sid.clone(),
        owner: "nobody".into(),
        fence: 0,
    };
    assert!(
        !store_sandbox_token(&server.db, &stale, Uuid::new_v4(), "t", "j")
            .await
            .unwrap()
    );
    let jti: Option<String> =
        sqlx::query_scalar("SELECT sandbox_token_jti FROM chat_sessions WHERE session_id = $1")
            .bind(&sid)
            .fetch_one(&server.db)
            .await
            .unwrap();
    assert_eq!(jti, None);
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_sealed_token_does_not_open_on_another_sessions_row() {
    let server = common::TestServer::start().await;
    let agent = Uuid::new_v4();
    let a = insert_session(&server.db, "running", None, None).await;
    let b = insert_session(&server.db, "running", None, None).await;
    let lease = live_lease(&server.db, &a).await;
    assert!(
        store_sandbox_token(&server.db, &lease, agent, "secret-token", "j")
            .await
            .unwrap()
    );
    assert_eq!(
        load_sandbox_token(&server.db, &a, agent)
            .await
            .unwrap()
            .as_deref(),
        Some("secret-token")
    );
    // Copy the ciphertext to another session's row: it is bound to its own session id.
    sqlx::query(
        "UPDATE chat_sessions SET sandbox_token_enc = \
         (SELECT sandbox_token_enc FROM chat_sessions WHERE session_id = $1) WHERE session_id = $2",
    )
    .bind(&a)
    .bind(&b)
    .execute(&server.db)
    .await
    .unwrap();
    assert!(load_sandbox_token(&server.db, &b, agent).await.is_err());
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn the_pending_pointer_is_cleared_even_after_the_lease_is_released() {
    let server = common::TestServer::start().await;
    let sid = insert_session(&server.db, "running", None, None).await;
    let lease = live_lease(&server.db, &sid).await;
    let hitl = Uuid::new_v4();
    set_pending(&server.db, &lease, hitl, "call-1")
        .await
        .unwrap();
    // The turn timed out: its driver released the lease (clearing the owner) and only then did the
    // abandoned wait's guard run.
    release_turn(&server.db, &lease).await.unwrap();
    clear_pending(&server.db, &lease, hitl).await.unwrap();
    let pending: Option<serde_json::Value> =
        sqlx::query_scalar("SELECT pending_permission FROM chat_sessions WHERE session_id = $1")
            .bind(&sid)
            .fetch_one(&server.db)
            .await
            .unwrap();
    assert_eq!(pending, None);
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_heartbeat_distinguishes_a_lost_lease_from_an_unreachable_database() {
    let server = common::TestServer::start().await;
    let sid = insert_session(&server.db, "running", None, None).await;
    let lease = live_lease(&server.db, &sid).await;
    let budget = std::time::Duration::from_millis(300);
    assert_eq!(
        heartbeat_retrying(&server.db, &lease, 60, budget).await,
        Beat::Renewed
    );

    let stale = TurnLease {
        fence: lease.fence + 1,
        ..lease.clone()
    };
    assert_eq!(
        heartbeat_retrying(&server.db, &stale, 60, budget).await,
        Beat::Lost
    );

    let dead = sqlx::postgres::PgPoolOptions::new()
        .acquire_timeout(std::time::Duration::from_millis(100))
        .connect_lazy("postgres://x:x@127.0.0.1:1/x")
        .unwrap();
    assert_eq!(
        heartbeat_retrying(&dead, &lease, 60, budget).await,
        Beat::Unreachable
    );
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn deleting_a_user_releases_their_sandboxes_and_revokes_their_tokens() {
    use nasiko_runtime::{SandboxKey, SandboxProvider as _};
    let server = common::TestServer::start().await;
    let sid = insert_session(&server.db, "running", None, None).await;
    let owner: Uuid = sqlx::query_scalar("SELECT user_id FROM chat_sessions WHERE session_id = $1")
        .bind(&sid)
        .fetch_one(&server.db)
        .await
        .unwrap();
    // A harness session with a live sandbox recorded on the row, and a stored token.
    make_harness_session(&server.db, &sid, false).await;
    let row = nasiko_server::sessions::SessionStore::new(server.db.clone())
        .get(&sid)
        .await
        .unwrap()
        .unwrap();
    let key = SandboxKey::Session {
        session_id: row.sandbox_uuid(),
    };
    let spec = nasiko_runtime::SandboxSpec {
        key: key.clone(),
        image: "img".into(),
        command: vec![],
        env: Default::default(),
        resources: None,
        workspace: None,
        runtime_class: None,
        image_pull_secret: None,
        ports: vec![7000],
        transport: nasiko_runtime::TransportKind::WebSocket,
    };
    let handle = server.sandbox.acquire(&spec).await.unwrap();
    sqlx::query("UPDATE chat_sessions SET sandbox_ref = $2, sandbox_token_jti = 'jti-user' WHERE session_id = $1")
        .bind(&sid)
        .bind(handle.sandbox_ref())
        .execute(&server.db)
        .await
        .unwrap();

    let admin: Uuid = sqlx::query_scalar(
        "INSERT INTO users (username, email, is_superuser) VALUES ('adm', 'adm@test.local', true) RETURNING id",
    )
    .fetch_one(&server.db)
    .await
    .unwrap();
    let resp = common::as_superuser(
        server
            .client
            .delete(server.url(&format!("/api/users/{owner}"))),
        &admin.to_string(),
        "admin",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(resp.status(), 204);
    assert!(is_revoked(&server.db, "jti-user").await);
    assert!(server.sandbox.list().await.unwrap().is_empty());
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn hard_deleting_an_agent_revokes_its_sessions_tokens() {
    let server = common::TestServer::start().await;
    let sid = insert_session(&server.db, "parked", None, None).await;
    let owner: Uuid = sqlx::query_scalar("SELECT user_id FROM chat_sessions WHERE session_id = $1")
        .bind(&sid)
        .fetch_one(&server.db)
        .await
        .unwrap();
    let agent: Uuid = sqlx::query_scalar(
        "INSERT INTO agents (name, owner_id, status, workload_kind, harness_id) \
         VALUES ($1, $2, 'running', 'harness', 'opencode') RETURNING id",
    )
    .bind(format!("h-{}", Uuid::new_v4().simple()))
    .bind(owner)
    .fetch_one(&server.db)
    .await
    .unwrap();
    sqlx::query("UPDATE chat_sessions SET agent_id = $2, sandbox_token_jti = 'jti-agent' WHERE session_id = $1")
        .bind(&sid)
        .bind(agent)
        .execute(&server.db)
        .await
        .unwrap();
    nasiko_server::sessions::teardown::revoke_tokens_of_agent(&server.db, agent).await;
    assert!(is_revoked(&server.db, "jti-agent").await);
    server.cleanup().await;
}

//! End to end: a harness session's turn on the `SandboxProvider`. The real harness shim runs
//! in-process over a fake `sh` ACP child; the fake sandbox provider points `attach` at it, so
//! the whole path is exercised — proxy, session resolve, lease, acquire, WebSocket connect, ACP
//! handshake, prompt, SSE, persistence — with no Docker.
//!
//! Requires infra (Postgres :5432, Redis, S3); skips when Postgres is unreachable:
//!   cargo test -p nasiko-server --test harness_turn_e2e -- --test-threads=1

mod common;

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use harness_shim::server::{Config as ShimConfig, run};
use nasiko_runtime::{SandboxKey, SandboxProvider as _};
use nasiko_server::sessions::admission::{SESSION_REFUSED, SessionAdmission, SessionRefusal};
use nasiko_server::sessions::identity::shim_token;
use serde_json::{Value, json};
use serial_test::serial;
use sqlx::PgPool;
use tokio::sync::oneshot;
use uuid::Uuid;

/// A fake ACP agent (request ids are the client's own JSON, echoed back verbatim): answers `initialize`, `session/new` and `session/prompt` (a tool call and
/// a text chunk, then `end_turn`). Does not advertise `loadSession`, so every turn opens a
/// fresh agent session.
const FAKE_ACP: &str = r#"
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -E 's/^\{"jsonrpc":"2\.0","id":("[^"]*"|[0-9]+),.*/\1/')
  case "$line" in
    *'"method":"initialize"'*)
      echo "{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":{\"protocolVersion\":1,\"agentCapabilities\":{}}}" ;;
    *'"method":"session/new"'*)
      echo "{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":{\"sessionId\":\"sh-session\"}}" ;;
    *'"method":"session/prompt"'*)
      echo '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"sh-session","update":{"sessionUpdate":"tool_call","toolCallId":"t1","title":"read file","kind":"read","status":"in_progress"}}}'
      echo '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"sh-session","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"hello from shim"}}}}'
      echo "{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":{\"stopReason\":\"end_turn\"}}" ;;
  esac
done
"#;

/// A fake agent whose prompt never answers, so a turn stays in flight.
const SLOW_ACP: &str = r#"
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -E 's/^\{"jsonrpc":"2\.0","id":("[^"]*"|[0-9]+),.*/\1/')
  case "$line" in
    *'"method":"initialize"'*)
      echo "{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":{\"protocolVersion\":1,\"agentCapabilities\":{}}}" ;;
    *'"method":"session/new"'*)
      echo "{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":{\"sessionId\":\"sh-session\"}}" ;;
    *'"method":"session/prompt"'*) sleep 60 ;;
  esac
done
"#;

const SECRET: &str = "harness-turn-e2e-secret";

async fn start_shim(script: &str, token: Option<String>) -> SocketAddr {
    let config = ShimConfig {
        listen: "127.0.0.1:0".parse().unwrap(),
        buffer_lines: 100,
        argv: Some(vec!["sh".into(), "-c".into(), script.into()]),
        token,
        workspace: None,
        mode: harness_shim::handshake::Mode::Env,
        workspace_mount: "/workspace".into(),
        files: vec![],
    };
    let (tx, rx) = oneshot::channel();
    tokio::spawn(run(config, Some(tx)));
    rx.await.expect("shim bound")
}

async fn init_admin(server: &common::TestServer) {
    server
        .client
        .post(server.url("/api/auth/initialize-admin"))
        .json(&json!({"username": "admin", "email": "admin@test.local"}))
        .send()
        .await
        .unwrap();
}

async fn insert_user(db: &PgPool, name: &str) -> Uuid {
    sqlx::query_scalar("INSERT INTO users (username, email) VALUES ($1, $2) RETURNING id")
        .bind(name)
        .bind(format!("{name}@test.local"))
        .fetch_one(db)
        .await
        .unwrap()
}

fn stream_body(context_id: &str, text: &str) -> Value {
    json!({
        "jsonrpc": "2.0", "id": "1", "method": "message/stream",
        "params": { "message": {
            "role": "user", "messageId": Uuid::new_v4().to_string(),
            "contextId": context_id, "parts": [{ "text": text }]
        }}
    })
}

async fn turn_over_proxy(
    server: &common::TestServer,
    agent_id: Uuid,
    user: (Uuid, &str),
    context_id: &str,
) -> reqwest::Response {
    common::as_member(
        server
            .client
            .post(server.url(&format!("/api/agents/{agent_id}")))
            .json(&stream_body(context_id, "say hello")),
        &user.0.to_string(),
        user.1,
    )
    .send()
    .await
    .unwrap()
}

/// The session row once its turn has fully wound down (lease released).
async fn settled_session(db: &PgPool, session_id: &str) -> sqlx::postgres::PgRow {
    for _ in 0..50 {
        let row = sqlx::query(
            "SELECT sandbox_ref, runtime_state, last_turn_at, acp_session_id, turn_owner \
             FROM chat_sessions WHERE session_id = $1",
        )
        .bind(session_id)
        .fetch_one(db)
        .await
        .unwrap();
        let owner: Option<String> = sqlx::Row::get(&row, "turn_owner");
        if owner.is_none() {
            return row;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("turn lease was never released");
}

struct World {
    server: common::TestServer,
    agent_id: Uuid,
    alice: Uuid,
    bob: Uuid,
}

async fn world() -> World {
    world_with(|_| {}).await
}

async fn world_with(configure: impl FnOnce(&mut nasiko_config::Config)) -> World {
    let server = common::TestServer::start_with(|c| {
        c.agent_jwt_secret = SECRET.into();
        configure(c);
    })
    .await;
    init_admin(&server).await;
    nasiko_server::seed::seed_harness_agents(&server.state).await;
    let agent_id: Uuid = sqlx::query_scalar(
        "SELECT id FROM agents WHERE workload_kind = 'harness' AND harness_id = 'opencode'",
    )
    .fetch_one(&server.db)
    .await
    .unwrap();
    let alice = insert_user(&server.db, "alice").await;
    let bob = insert_user(&server.db, "bob").await;
    World {
        server,
        agent_id,
        alice,
        bob,
    }
}

/// A shim that demands the right bearer, wired as the sandbox of a fresh session.
async fn session_with_shim(server: &common::TestServer) -> (SandboxKey, String) {
    session_with_script(server, FAKE_ACP).await
}

/// As [`session_with_shim`], with the fake agent's behaviour chosen by the caller.
async fn session_with_script(server: &common::TestServer, script: &str) -> (SandboxKey, String) {
    let context_id = Uuid::new_v4();
    let key = SandboxKey::Session {
        session_id: context_id,
    };
    let shim = start_shim(script, Some(shim_token(SECRET, &key).unwrap())).await;
    server
        .sandbox
        .set_transport_url(&key, format!("ws://{shim}/acp"));
    (key, context_id.to_string())
}

#[tokio::test]
#[serial]
async fn harness_turn_runs_on_the_sandbox_and_streams_over_sse() {
    let World {
        server,
        agent_id,
        alice,
        bob,
    } = world().await;
    let (key, sid) = session_with_shim(&server).await;

    // First turn: acquires the sandbox, streams the activity, the text and a terminal frame.
    let resp = turn_over_proxy(&server, agent_id, (alice, "alice"), &sid).await;
    assert_eq!(resp.status(), 200);
    let text = tokio::time::timeout(Duration::from_secs(30), resp.text())
        .await
        .expect("SSE completes")
        .unwrap();
    assert!(text.contains("TASK_STATE_WORKING"), "{text}");
    assert!(text.contains("hello from shim"), "{text}");
    assert!(text.contains(r#""kind":"tool_call""#), "{text}");
    assert!(text.contains("TASK_STATE_COMPLETED"), "{text}");

    let recorded_key: Option<String> =
        sqlx::query_scalar("SELECT sandbox_key FROM chat_sessions WHERE session_id = $1")
            .bind(&sid)
            .fetch_one(&server.db)
            .await
            .unwrap();
    assert_eq!(recorded_key.as_deref(), Some(key.name().as_str()));
    let first_env = server
        .sandbox
        .last_acquire_spec(&key)
        .expect("turn 1 acquired")
        .env;
    let row = settled_session(&server.db, &sid).await;
    let sandbox_ref: Option<String> = sqlx::Row::get(&row, "sandbox_ref");
    let state: String = sqlx::Row::get(&row, "runtime_state");
    let last_turn: Option<chrono::DateTime<chrono::Utc>> = sqlx::Row::get(&row, "last_turn_at");
    let acp: Option<String> = sqlx::Row::get(&row, "acp_session_id");
    assert!(sandbox_ref.is_some_and(|r| r.starts_with("fake:")));
    assert_eq!(state, "running");
    assert!(last_turn.is_some());
    assert_eq!(acp.as_deref(), Some("sh-session"));
    let reply: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM chat_messages WHERE session_id = $1 AND role = 'assistant' \
         AND content = 'hello from shim'",
    )
    .bind(&sid)
    .fetch_one(&server.db)
    .await
    .unwrap();
    assert_eq!(reply, 1, "the reply is persisted to the transcript");

    // Second turn, same contextId: same session row, the provider is asked again (resume).
    let resp = turn_over_proxy(&server, agent_id, (alice, "alice"), &sid).await;
    assert_eq!(resp.status(), 200);
    let text = resp.text().await.unwrap();
    assert!(text.contains("TASK_STATE_COMPLETED"), "{text}");
    settled_session(&server.db, &sid).await;
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM chat_sessions WHERE session_id = $1")
        .bind(&sid)
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert_eq!(rows, 1);
    assert_eq!(server.sandbox.acquire_count(&key), 2);

    // The sandbox's credential is the one minted when it was created, and it is still live
    // on turn 2; the per-turn tokens are revoked as their turns end.
    assert_sandbox_credentials_live(&server, &key, &sid, agent_id, &first_env).await;

    // Someone else's contextId is a 404 (not a 403, which would confirm it exists) and writes
    // nothing under it: no transcript row, no flow.
    let flows_before: i64 = sqlx::query_scalar("SELECT count(*) FROM flows")
        .fetch_one(&server.db)
        .await
        .unwrap();
    let before: i64 =
        sqlx::query_scalar("SELECT count(*) FROM chat_messages WHERE session_id = $1")
            .bind(&sid)
            .fetch_one(&server.db)
            .await
            .unwrap();
    let resp = turn_over_proxy(&server, agent_id, (bob, "bob"), &sid).await;
    assert_eq!(resp.status(), 404);
    assert_eq!(server.sandbox.acquire_count(&key), 2);
    let after: i64 = sqlx::query_scalar("SELECT count(*) FROM chat_messages WHERE session_id = $1")
        .bind(&sid)
        .fetch_one(&server.db)
        .await
        .unwrap();
    let flows_after: i64 = sqlx::query_scalar("SELECT count(*) FROM flows")
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert_eq!(
        flows_before, flows_after,
        "a refused turn must not open a flow"
    );
    assert_eq!(
        before, after,
        "a refused turn must not write to the transcript"
    );

    // The dispatch path resolves the session first and answers 404 for another user's context.
    let body = json!({
        "jsonrpc": "2.0", "id": "1", "method": "message/stream",
        "params": {
            "message": {
                "role": "ROLE_USER", "messageId": Uuid::new_v4().to_string(),
                "contextId": sid, "parts": [{ "text": "hi" }]
            },
            "metadata": { "agent_id": agent_id.to_string() }
        }
    });
    let resp = common::as_member(
        server
            .client
            .post(server.url("/api/orchestrator/a2a"))
            .json(&body),
        &bob.to_string(),
        "bob",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(resp.status(), 404);
    assert_eq!(server.sandbox.acquire_count(&key), 2);
    server.cleanup().await;
}

/// Every acquire is given the same full env, and it is the stored, never-revoked sandbox token;
/// exactly the two turn tokens are revoked.
async fn assert_sandbox_credentials_live(
    server: &common::TestServer,
    key: &SandboxKey,
    sid: &str,
    agent_id: Uuid,
    first_env: &std::collections::HashMap<String, String>,
) {
    let env = server
        .sandbox
        .last_acquire_spec(key)
        .expect("turn 2 acquired")
        .env;
    assert_eq!(
        &env, first_env,
        "the env passed to acquire is stable across turns"
    );
    let token = &env["NASIKO_SESSION_TOKEN"];
    assert_eq!(&env["OPENAI_API_KEY"], token);
    assert_eq!(&env["ANTHROPIC_API_KEY"], token);
    assert_eq!(env["HARNESS_SHIM_TOKEN"], shim_token(SECRET, key).unwrap());
    let stored_token =
        nasiko_server::sessions::token::load_sandbox_token(&server.db, sid, agent_id)
            .await
            .unwrap();
    assert_eq!(stored_token.as_deref(), Some(token.as_str()));
    let claims = nasiko_session_token::verify_session_token(
        token,
        SECRET,
        nasiko_session_token::parse_algorithm("HS256"),
    )
    .expect("the sandbox token verifies");
    let stored: Option<String> =
        sqlx::query_scalar("SELECT sandbox_token_jti FROM chat_sessions WHERE session_id = $1")
            .bind(sid)
            .fetch_one(&server.db)
            .await
            .unwrap();
    assert_eq!(stored.as_deref(), Some(claims.jti.as_str()));
    let revoked: i64 =
        sqlx::query_scalar("SELECT count(*) FROM revoked_session_tokens WHERE jti = $1")
            .bind(&claims.jti)
            .fetch_one(&server.db)
            .await
            .unwrap();
    assert_eq!(
        revoked, 0,
        "the sandbox token must outlive the turns that used the sandbox"
    );
    let turn_tokens: i64 = sqlx::query_scalar("SELECT count(*) FROM revoked_session_tokens")
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert_eq!(
        turn_tokens, 2,
        "exactly the two turn tokens are revoked, never the sandbox's"
    );
}

#[tokio::test]
#[serial]
async fn a_turn_on_a_busy_session_is_a_409_on_both_front_doors() {
    let World {
        server,
        agent_id,
        alice,
        ..
    } = world().await;
    let (_key, sid) = session_with_shim(&server).await;
    nasiko_server::sessions::SessionStore::new(server.db.clone())
        .find_or_create_for_harness(agent_id, alice, &sid)
        .await
        .unwrap()
        .expect("own session");
    // Another turn holds the lease.
    nasiko_server::coding::lifecycle::acquire_turn(&server.db, &sid, "other-replica", "t0", 600)
        .await
        .unwrap()
        .expect("lease");

    let resp = turn_over_proxy(&server, agent_id, (alice, "alice"), &sid).await;
    assert_eq!(resp.status(), 409);

    let body = json!({
        "jsonrpc": "2.0", "id": "1", "method": "message/stream",
        "params": {
            "message": {
                "role": "ROLE_USER", "messageId": Uuid::new_v4().to_string(),
                "contextId": sid, "parts": [{ "text": "hi" }]
            },
            "metadata": { "agent_id": agent_id.to_string() }
        }
    });
    let resp = common::as_member(
        server
            .client
            .post(server.url("/api/orchestrator/a2a"))
            .json(&body),
        &alice.to_string(),
        "alice",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(resp.status(), 409);
    server.cleanup().await;
}

/// A message refused with a 409 while turn N runs must not become the session's "newest flow":
/// the session token's attribution stays on turn N.
#[tokio::test]
#[serial]
async fn a_busy_409_does_not_move_attribution_off_the_running_turn() {
    let World {
        server,
        agent_id,
        alice,
        ..
    } = world().await;
    let (_key, sid) = session_with_shim(&server).await;
    nasiko_server::sessions::SessionStore::new(server.db.clone())
        .find_or_create_for_harness(agent_id, alice, &sid)
        .await
        .unwrap()
        .expect("own session");
    let turn_n = "abababababababababababababababab";
    sqlx::query("INSERT INTO flows (flow_id, user_id, session_id) VALUES ($1, $2, $3)")
        .bind(turn_n)
        .bind(alice)
        .bind(&sid)
        .execute(&server.db)
        .await
        .unwrap();
    sqlx::query("INSERT INTO flow_participants (flow_id, agent_id) VALUES ($1, $2)")
        .bind(turn_n)
        .bind(agent_id)
        .execute(&server.db)
        .await
        .unwrap();
    nasiko_server::coding::lifecycle::acquire_turn(&server.db, &sid, "other-replica", "t0", 600)
        .await
        .unwrap()
        .expect("lease");

    let resp = turn_over_proxy(&server, agent_id, (alice, "alice"), &sid).await;
    assert_eq!(resp.status(), 409);

    let claims = nasiko_session_token::SessionClaims {
        agent_id: agent_id.to_string(),
        user_id: alice.to_string(),
        session_id: sid.clone(),
        flow_id: turn_n.into(),
        jti: "jti".into(),
        aud: String::new(),
        iat: 0,
        exp: 0,
    };
    let attributed =
        nasiko_session_token::attribution::attributed_flow(&server.db, None, &claims).await;
    assert_eq!(
        attributed.flow_id, turn_n,
        "the rejected message's flow must not win"
    );
    let rejected: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM flows WHERE session_id = $1 AND status = 'failed'",
    )
    .bind(&sid)
    .fetch_one(&server.db)
    .await
    .unwrap();
    assert_eq!(
        rejected, 1,
        "the 409'd message's flow is failed, not left running"
    );
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_failed_acquire_parks_a_fresh_session_and_leaves_a_live_one_running() {
    let World {
        server,
        agent_id,
        alice,
        ..
    } = world().await;

    // Fresh session: nothing was ever running, so it is parked and no token is left behind.
    let (_key, fresh) = session_with_shim(&server).await;
    server
        .sandbox
        .fail_next_acquire(nasiko_runtime::RuntimeError::Internal("boom".into()));
    let resp = turn_over_proxy(&server, agent_id, (alice, "alice"), &fresh).await;
    assert_eq!(resp.status(), 502);
    let (state, sandbox_ref, jti): (String, Option<String>, Option<String>) = sqlx::query_as(
        "SELECT runtime_state, sandbox_ref, sandbox_token_jti FROM chat_sessions WHERE session_id = $1",
    )
    .bind(&fresh)
    .fetch_one(&server.db)
    .await
    .unwrap();
    assert_eq!((state.as_str(), sandbox_ref), ("parked", None));
    // The token minted for the attempt is kept for the next one: a sandbox that survived the
    // failed acquire still carries it, so it must not be revoked.
    let jti = jti.expect("the sandbox token is stored");
    let revoked: i64 =
        sqlx::query_scalar("SELECT count(*) FROM revoked_session_tokens WHERE jti = $1")
            .bind(&jti)
            .fetch_one(&server.db)
            .await
            .unwrap();
    assert_eq!(revoked, 0);

    // A session that already owns a running sandbox keeps its state: parking it would orphan it.
    let (_key, live) = session_with_shim(&server).await;
    nasiko_server::sessions::SessionStore::new(server.db.clone())
        .find_or_create_for_harness(agent_id, alice, &live)
        .await
        .unwrap()
        .expect("own session");
    sqlx::query(
        "UPDATE chat_sessions SET sandbox_ref = 'fake:fake-x', runtime_state = 'running' \
         WHERE session_id = $1",
    )
    .bind(&live)
    .execute(&server.db)
    .await
    .unwrap();
    server
        .sandbox
        .fail_next_acquire(nasiko_runtime::RuntimeError::Internal("boom".into()));
    let resp = turn_over_proxy(&server, agent_id, (alice, "alice"), &live).await;
    assert_eq!(resp.status(), 502);
    let state: String =
        sqlx::query_scalar("SELECT runtime_state FROM chat_sessions WHERE session_id = $1")
            .bind(&live)
            .fetch_one(&server.db)
            .await
            .unwrap();
    assert_eq!(state, "running");
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_suspend_claim_between_the_lease_and_acquire_stops_the_turn_with_a_409() {
    let World {
        server,
        agent_id,
        alice,
        ..
    } = world().await;
    let (key, sid) = session_with_shim(&server).await;
    // Stands in for a suspend claim that lands right after the turn takes its lease: the fence
    // moves on, so the turn's first fenced write misses.
    for statement in [
        "CREATE FUNCTION bump_fence() RETURNS trigger AS $$ BEGIN \
         UPDATE chat_sessions SET turn_fence = turn_fence + 1 WHERE session_id = NEW.session_id; \
         RETURN NULL; END $$ LANGUAGE plpgsql",
        "CREATE TRIGGER bump_fence AFTER UPDATE ON chat_sessions FOR EACH ROW \
         WHEN (OLD.turn_owner IS NULL AND NEW.turn_owner IS NOT NULL) EXECUTE FUNCTION bump_fence()",
    ] {
        sqlx::query(statement).execute(&server.db).await.unwrap();
    }

    let resp = turn_over_proxy(&server, agent_id, (alice, "alice"), &sid).await;
    assert_eq!(resp.status(), 409);
    assert_eq!(
        server.sandbox.acquire_count(&key),
        0,
        "a superseded turn must never reach the sandbox"
    );
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn losing_the_lease_mid_turn_fails_the_stream_with_the_suspend_reason() {
    let World {
        server,
        agent_id,
        alice,
        ..
    } = world_with(|c| c.coding_agent_lease_heartbeat_secs = 1).await;
    let (_key, sid) = session_with_script(&server, SLOW_ACP).await;

    // The response arrives once the prompt is in flight.
    let resp = turn_over_proxy(&server, agent_id, (alice, "alice"), &sid).await;
    assert_eq!(resp.status(), 200);
    // A suspend claim takes the lease, bumps the fence and marks the row `stopping`.
    sqlx::query(
        "UPDATE chat_sessions SET turn_fence = turn_fence + 1, turn_owner = NULL, \
         lease_expires_at = NULL, runtime_state = 'stopping' WHERE session_id = $1",
    )
    .bind(&sid)
    .execute(&server.db)
    .await
    .unwrap();

    let text = tokio::time::timeout(Duration::from_secs(15), resp.text())
        .await
        .expect("the stream ends on the next heartbeat")
        .unwrap();
    assert!(text.contains("TASK_STATE_FAILED"), "{text}");
    assert!(text.contains("session was suspended"), "{text}");
    server.cleanup().await;
}

async fn completed_text(resp: reqwest::Response) -> String {
    assert_eq!(resp.status(), 200);
    let text = tokio::time::timeout(Duration::from_secs(30), resp.text())
        .await
        .expect("SSE completes")
        .unwrap();
    assert!(text.contains("TASK_STATE_COMPLETED"), "{text}");
    text
}

#[tokio::test]
#[serial]
async fn a_reconciler_tick_during_acquire_leaves_the_turn_alive() {
    let World {
        server,
        agent_id,
        alice,
        ..
    } = world().await;
    let (key, sid) = session_with_shim(&server).await;
    let expected_key = key.name();
    server.sandbox.delay_next_acquire(Duration::from_secs(2));

    let turn = turn_over_proxy(&server, agent_id, (alice, "alice"), &sid);
    // The coding-agent reconciler's tick lands while the provider is still acquiring: the row is
    // `starting` with no `sandbox_ref` yet.
    let tick = async {
        tokio::time::sleep(Duration::from_secs(1)).await;
        // Mid-acquire: the sandbox's key is on the row though its ref is not, so whatever finds a
        // session from its sandbox can tell this one from an orphan.
        let (key, sandbox_ref): (Option<String>, Option<String>) = sqlx::query_as(
            "SELECT sandbox_key, sandbox_ref FROM chat_sessions WHERE session_id = $1",
        )
        .bind(&sid)
        .fetch_one(&server.db)
        .await
        .unwrap();
        assert_eq!(key.as_deref(), Some(expected_key.as_str()));
        assert_eq!(sandbox_ref, None);
        nasiko_server::coding::lifecycle::reconcile_startup(
            &server.db,
            &std::collections::HashSet::new(),
        )
        .await
        .unwrap()
    };
    let (resp, reset) = tokio::join!(turn, tick);
    assert_eq!(reset, 0, "a harness row is not the reconciler's");
    completed_text(resp).await;
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn the_lease_is_kept_alive_while_the_sandbox_is_being_acquired() {
    let World {
        server,
        agent_id,
        alice,
        ..
    } = world_with(|c| {
        c.coding_agent_turn_lease_secs = 3;
        c.coding_agent_lease_heartbeat_secs = 1;
    })
    .await;
    let (_key, sid) = session_with_shim(&server).await;
    // The provider takes longer than the lease.
    server.sandbox.delay_next_acquire(Duration::from_secs(5));

    let turn = turn_over_proxy(&server, agent_id, (alice, "alice"), &sid);
    let rival = async {
        tokio::time::sleep(Duration::from_millis(4200)).await;
        nasiko_server::coding::lifecycle::acquire_turn(&server.db, &sid, "rival", "t-rival", 60)
            .await
            .unwrap()
    };
    let (resp, rival_lease) = tokio::join!(turn, rival);
    assert!(
        rival_lease.is_none(),
        "the starting turn's heartbeat keeps its lease past the lease length"
    );
    completed_text(resp).await;
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn deleting_a_session_mid_turn_ends_it_as_deleted_and_releases_the_sandbox() {
    let World {
        server,
        agent_id,
        alice,
        ..
    } = world_with(|c| c.coding_agent_lease_heartbeat_secs = 1).await;
    let (key, sid) = session_with_script(&server, SLOW_ACP).await;

    let resp = turn_over_proxy(&server, agent_id, (alice, "alice"), &sid).await;
    assert_eq!(resp.status(), 200);
    let deleted = common::as_member(
        server
            .client
            .delete(server.url(&format!("/api/chat/sessions/{sid}"))),
        &alice.to_string(),
        "alice",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(deleted.status(), 204);

    let text = tokio::time::timeout(Duration::from_secs(15), resp.text())
        .await
        .expect("the stream ends on the next heartbeat")
        .unwrap();
    assert!(text.contains("session was deleted"), "{text}");
    assert!(
        server.sandbox.list().await.unwrap().is_empty(),
        "the deleted session's sandbox is released"
    );
    let _ = key;
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_token_near_expiry_or_already_revoked_is_replaced_and_the_sandbox_rebuilt() {
    let World {
        server,
        agent_id,
        alice,
        ..
    } = world().await;
    let (key, sid) = session_with_shim(&server).await;
    completed_text(turn_over_proxy(&server, agent_id, (alice, "alice"), &sid).await).await;
    settled_session(&server.db, &sid).await;
    let first = server.sandbox.last_acquire_spec(&key).unwrap().env["NASIKO_SESSION_TOKEN"].clone();

    // Near expiry: store a token that has under a day left.
    let near = nasiko_session_token::mint_session_token(
        &nasiko_session_token::SessionTokenInput {
            agent_id: agent_id.to_string(),
            user_id: alice.to_string(),
            session_id: sid.clone(),
            flow_id: "f".into(),
        },
        SECRET,
        3600,
        nasiko_session_token::parse_algorithm("HS256"),
    )
    .unwrap();
    store_raw_token(&server, &sid, agent_id, &near.token, &near.jti).await;
    completed_text(turn_over_proxy(&server, agent_id, (alice, "alice"), &sid).await).await;
    settled_session(&server.db, &sid).await;
    let second =
        server.sandbox.last_acquire_spec(&key).unwrap().env["NASIKO_SESSION_TOKEN"].clone();
    assert_ne!(second, near.token, "the near-expiry token was replaced");
    assert_ne!(second, first);
    assert_eq!(revoked_count(&server, &near.jti).await, 1, "and revoked");
    // One acquire per turn, and the sandbox was released first: the creation spec (a resume keeps
    // it) carries the new token, so the sandbox was created again.
    assert_eq!(server.sandbox.acquire_count(&key), 2);
    assert_eq!(
        server.sandbox.spec_of(&key).unwrap().env["NASIKO_SESSION_TOKEN"],
        second
    );

    // A token revoked but still stored (a crash between the two writes) is not reused.
    let stored_jti: String =
        sqlx::query_scalar("SELECT sandbox_token_jti FROM chat_sessions WHERE session_id = $1")
            .bind(&sid)
            .fetch_one(&server.db)
            .await
            .unwrap();
    sqlx::query("INSERT INTO revoked_session_tokens (jti, expires_at) VALUES ($1, now() + interval '1 day')")
        .bind(&stored_jti)
        .execute(&server.db)
        .await
        .unwrap();
    completed_text(turn_over_proxy(&server, agent_id, (alice, "alice"), &sid).await).await;
    settled_session(&server.db, &sid).await;
    let third = server.sandbox.last_acquire_spec(&key).unwrap().env["NASIKO_SESSION_TOKEN"].clone();
    assert_ne!(third, second);
    let live: String =
        sqlx::query_scalar("SELECT sandbox_token_jti FROM chat_sessions WHERE session_id = $1")
            .bind(&sid)
            .fetch_one(&server.db)
            .await
            .unwrap();
    assert_ne!(live, stored_jti);
    assert_eq!(revoked_count(&server, &live).await, 0);
    server.cleanup().await;
}

/// As above, on a provider whose resumed sandbox runs on the resuming env (the bare pod, shared
/// runtime pods): the sandbox is suspended and resumed on the new token, never released, so it
/// keeps its state and files.
#[tokio::test]
#[serial]
async fn a_token_near_expiry_is_replaced_by_suspending_a_sandbox_that_resumes_on_the_new_env() {
    let World {
        server,
        agent_id,
        alice,
        ..
    } = world().await;
    server.sandbox.set_resume_takes_new_env(true);
    let (key, sid) = session_with_shim(&server).await;
    completed_text(turn_over_proxy(&server, agent_id, (alice, "alice"), &sid).await).await;
    settled_session(&server.db, &sid).await;

    let near = nasiko_session_token::mint_session_token(
        &nasiko_session_token::SessionTokenInput {
            agent_id: agent_id.to_string(),
            user_id: alice.to_string(),
            session_id: sid.clone(),
            flow_id: "f".into(),
        },
        SECRET,
        3600,
        nasiko_session_token::parse_algorithm("HS256"),
    )
    .unwrap();
    store_raw_token(&server, &sid, agent_id, &near.token, &near.jti).await;
    completed_text(turn_over_proxy(&server, agent_id, (alice, "alice"), &sid).await).await;
    settled_session(&server.db, &sid).await;

    let sent = server.sandbox.last_acquire_spec(&key).unwrap().env["NASIKO_SESSION_TOKEN"].clone();
    assert_ne!(sent, near.token, "the near-expiry token was replaced");
    assert_eq!(revoked_count(&server, &near.jti).await, 1, "and revoked");
    assert_eq!(server.sandbox.acquire_count(&key), 2);
    assert_eq!(
        server.sandbox.creations(&key),
        1,
        "the sandbox was suspended and resumed, never released and rebuilt"
    );
    assert_eq!(
        server.sandbox.spec_of(&key).unwrap().env["NASIKO_SESSION_TOKEN"],
        sent,
        "the resumed sandbox runs on the new token"
    );
    server.cleanup().await;
}

/// A shim in handshake mode: no session settings, no bearer, until the server configures it.
async fn start_handshake_shim() -> SocketAddr {
    let config = ShimConfig {
        listen: "127.0.0.1:0".parse().unwrap(),
        buffer_lines: 100,
        argv: None,
        token: None,
        workspace: None,
        mode: harness_shim::handshake::Mode::Handshake,
        workspace_mount: "/workspace".into(),
        files: vec![],
    };
    let (tx, rx) = oneshot::channel();
    tokio::spawn(run(config, Some(tx)));
    rx.await.expect("shim bound")
}

/// `child_generation` from the shim's own `/control/status`.
async fn child_generation(shim: SocketAddr, key: &SandboxKey) -> u64 {
    let status: Value = reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .get(format!("http://{shim}/control/status"))
        .bearer_auth(shim_token(SECRET, key).unwrap())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    status["child_generation"]
        .as_u64()
        .expect("child_generation")
}

/// Puts an executable `opencode` (the harness command the catalog names) running `script` first
/// on PATH, restoring it on drop, so a handshake-mode shim spawns the stub from the configure env.
struct StubHarness {
    old_path: Option<std::ffi::OsString>,
    dir: std::path::PathBuf,
}

impl StubHarness {
    fn install(script: &str) -> Self {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = std::env::temp_dir().join(format!("stub-harness-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let bin = dir.join("opencode");
        std::fs::write(&bin, format!("#!/bin/sh\n{script}\n")).unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        let old_path = std::env::var_os("PATH");
        let mut paths = vec![dir.clone()];
        paths.extend(std::env::split_paths(&old_path.clone().unwrap_or_default()));
        // SAFETY: the tests of this file run serially, and nothing else reads PATH meanwhile.
        unsafe { std::env::set_var("PATH", std::env::join_paths(paths).unwrap()) };
        Self { old_path, dir }
    }
}

impl Drop for StubHarness {
    fn drop(&mut self) {
        // SAFETY: as in `install`.
        unsafe {
            match &self.old_path {
                Some(p) => std::env::set_var("PATH", p),
                None => std::env::remove_var("PATH"),
            }
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[tokio::test]
#[serial]
async fn a_handshake_mode_shim_is_configured_once_and_restarted_only_when_its_env_changes() {
    let env_dump = std::env::temp_dir().join(format!("harness-env-{}", Uuid::new_v4()));
    let _stub = StubHarness::install(&format!("env > {}\n{FAKE_ACP}", env_dump.display()));
    let World {
        server,
        agent_id,
        alice,
        ..
    } = world().await;
    let context_id = Uuid::new_v4();
    let key = SandboxKey::Session {
        session_id: context_id,
    };
    let sid = context_id.to_string();
    let shim = start_handshake_shim().await;
    server
        .sandbox
        .set_transport_url(&key, format!("ws://{shim}/acp"));

    // Turn 1: the server configures the shim, waits for the harness to start, then connects.
    let text =
        completed_text(turn_over_proxy(&server, agent_id, (alice, "alice"), &sid).await).await;
    assert!(text.contains("hello from shim"), "{text}");
    settled_session(&server.db, &sid).await;
    let first = child_generation(shim, &key).await;
    // The platform's OTEL env, which a provider's decorator adds at acquire, reaches the harness
    // through the configure body too.
    let child_env = std::fs::read_to_string(&env_dump).expect("the stub harness dumped its env");
    assert!(
        child_env.contains("OTEL_EXPORTER_OTLP_ENDPOINT="),
        "{child_env}"
    );
    assert!(
        child_env
            .lines()
            .any(|l| l == format!("OTEL_SERVICE_NAME=harness-{sid}")),
        "{child_env}"
    );
    let _ = std::fs::remove_file(&env_dump);

    // Turn 2, same env: connect only; the harness is not restarted.
    let text =
        completed_text(turn_over_proxy(&server, agent_id, (alice, "alice"), &sid).await).await;
    assert!(text.contains("hello from shim"), "{text}");
    settled_session(&server.db, &sid).await;
    assert_eq!(child_generation(shim, &key).await, first);

    // Turn 3 after the sandbox token is replaced: the env changed, so the harness restarts once.
    let near = nasiko_session_token::mint_session_token(
        &nasiko_session_token::SessionTokenInput {
            agent_id: agent_id.to_string(),
            user_id: alice.to_string(),
            session_id: sid.clone(),
            flow_id: "f".into(),
        },
        SECRET,
        3600,
        nasiko_session_token::parse_algorithm("HS256"),
    )
    .unwrap();
    store_raw_token(&server, &sid, agent_id, &near.token, &near.jti).await;
    let text =
        completed_text(turn_over_proxy(&server, agent_id, (alice, "alice"), &sid).await).await;
    assert!(text.contains("hello from shim"), "{text}");
    settled_session(&server.db, &sid).await;
    assert_eq!(child_generation(shim, &key).await, first + 1);
    server.cleanup().await;
}

async fn revoked_count(server: &common::TestServer, jti: &str) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM revoked_session_tokens WHERE jti = $1")
        .bind(jti)
        .fetch_one(&server.db)
        .await
        .unwrap()
}

/// Put a token on the session row the way the turn path would have, bypassing its fence.
async fn store_raw_token(
    server: &common::TestServer,
    sid: &str,
    agent_id: Uuid,
    token: &str,
    jti: &str,
) {
    let sealed = nasiko_secrets::SecretsCrypto::try_for_sandbox_token(agent_id)
        .unwrap()
        .encrypt_with_aad(token, sid.as_bytes());
    sqlx::query(
        "UPDATE chat_sessions SET sandbox_token_jti = $2, sandbox_token_enc = $3 WHERE session_id = $1",
    )
    .bind(sid)
    .bind(jti)
    .bind(sealed.into_bytes())
    .execute(&server.db)
    .await
    .unwrap();
}

#[tokio::test]
#[serial]
async fn the_sessions_repo_reaches_the_sandbox_env_with_the_users_github_token_only_for_github() {
    let World {
        server,
        agent_id,
        alice,
        ..
    } = world().await;
    let github_token = "gho_alice_token";
    let sealed = nasiko_secrets::SecretsCrypto::for_user(alice).encrypt(github_token);
    sqlx::query(
        "INSERT INTO user_identities (user_id, provider, provider_id, provider_metadata) \
         VALUES ($1, 'github', 'alice-gh', $2)",
    )
    .bind(alice)
    .bind(json!({ "access_token": sealed }))
    .execute(&server.db)
    .await
    .unwrap();

    for (repo, token) in [
        ("https://github.com/acme/app.git", Some(github_token)),
        ("https://gitlab.com/acme/app.git", None),
    ] {
        let (key, sid) = session_with_shim(&server).await;
        nasiko_server::sessions::SessionStore::new(server.db.clone())
            .find_or_create_for_harness(agent_id, alice, &sid)
            .await
            .unwrap()
            .expect("own session");
        sqlx::query("UPDATE chat_sessions SET workspace_repo = $2 WHERE session_id = $1")
            .bind(&sid)
            .bind(repo)
            .execute(&server.db)
            .await
            .unwrap();

        completed_text(turn_over_proxy(&server, agent_id, (alice, "alice"), &sid).await).await;
        let env = server.sandbox.last_acquire_spec(&key).unwrap().env;
        assert_eq!(env["WORKSPACE_REPO"], repo);
        assert_eq!(
            env.get("WORKSPACE_REPO_TOKEN").map(String::as_str),
            token,
            "{repo}"
        );
    }
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_claim_after_the_key_is_recorded_stops_the_turn_and_revokes_the_token_it_minted() {
    let World {
        server,
        agent_id,
        alice,
        ..
    } = world().await;
    let (key, sid) = session_with_shim(&server).await;
    // A suspend claim lands right after the sandbox key is written: the turn's next fenced write,
    // storing the token it just minted, misses.
    for statement in [
        "CREATE FUNCTION bump_fence() RETURNS trigger AS $$ BEGIN \
         UPDATE chat_sessions SET turn_fence = turn_fence + 1 WHERE session_id = NEW.session_id; \
         RETURN NULL; END $$ LANGUAGE plpgsql",
        "CREATE TRIGGER bump_fence AFTER UPDATE ON chat_sessions FOR EACH ROW \
         WHEN (OLD.sandbox_key IS DISTINCT FROM NEW.sandbox_key) EXECUTE FUNCTION bump_fence()",
    ] {
        sqlx::query(statement).execute(&server.db).await.unwrap();
    }

    let resp = turn_over_proxy(&server, agent_id, (alice, "alice"), &sid).await;
    assert_eq!(resp.status(), 409);
    assert_eq!(server.sandbox.acquire_count(&key), 0);
    let stored: Option<String> =
        sqlx::query_scalar("SELECT sandbox_token_jti FROM chat_sessions WHERE session_id = $1")
            .bind(&sid)
            .fetch_one(&server.db)
            .await
            .unwrap();
    assert_eq!(stored, None, "the minted token was never stored");
    let revoked: i64 = sqlx::query_scalar("SELECT count(*) FROM revoked_session_tokens")
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert_eq!(
        revoked, 2,
        "the minted sandbox token and the turn's own token are both revoked"
    );
    server.cleanup().await;
}

// ---- A2A task methods -------------------------------------------------------------------------

/// A fake agent whose prompt streams one chunk and then waits: it answers only when it sees
/// `session/cancel`, which it records in `<dir>/seen`.
fn watching_acp(dir: &std::path::Path) -> String {
    let dir = dir.display();
    format!(
        r#"
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -E 's/^\{{"jsonrpc":"2\.0","id":("[^"]*"|[0-9]+),.*/\1/')
  case "$line" in
    *'"method":"initialize"'*)
      echo "{{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":{{\"protocolVersion\":1,\"agentCapabilities\":{{}}}}}}" ;;
    *'"method":"session/new"'*)
      echo "{{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":{{\"sessionId\":\"sh-session\"}}}}" ;;
    *'"method":"session/prompt"'*)
      echo "$id" > {dir}/prompt_id
      echo '{{"jsonrpc":"2.0","method":"session/update","params":{{"sessionId":"sh-session","update":{{"sessionUpdate":"agent_message_chunk","content":{{"type":"text","text":"working on it"}}}}}}}}' ;;
    *'"method":"session/cancel"'*)
      echo cancel >> {dir}/seen
      pid=$(cat {dir}/prompt_id)
      echo "{{\"jsonrpc\":\"2.0\",\"id\":$pid,\"result\":{{\"stopReason\":\"cancelled\"}}}}" ;;
  esac
done
"#
    )
}

/// Read chunks until the stream shows the task id and `needle`; returns the task id.
async fn read_until(resp: &mut reqwest::Response, needle: &str) -> (String, String) {
    let mut seen = String::new();
    tokio::time::timeout(Duration::from_secs(20), async {
        while let Some(chunk) = resp.chunk().await.unwrap() {
            seen.push_str(&String::from_utf8_lossy(&chunk));
            if seen.contains(needle) && seen.contains(r#""taskId":""#) {
                break;
            }
        }
    })
    .await
    .expect("the stream shows what was expected");
    let from = seen.find(r#""taskId":""#).unwrap() + r#""taskId":""#.len();
    let task_id = seen[from..].split('"').next().unwrap().to_owned();
    (task_id, seen)
}

fn task_rpc(method: &str, task_id: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": "r1", "method": method, "params": {"id": task_id}})
}

async fn proxy_rpc(
    server: &common::TestServer,
    agent_id: Uuid,
    user: (Uuid, &str),
    method: &str,
    task_id: &str,
) -> reqwest::Response {
    common::as_member(
        server
            .client
            .post(server.url(&format!("/api/agents/{agent_id}")))
            .json(&task_rpc(method, task_id)),
        &user.0.to_string(),
        user.1,
    )
    .send()
    .await
    .unwrap()
}

async fn dispatch_rpc(
    server: &common::TestServer,
    agent_id: Uuid,
    user: (Uuid, &str),
    method: &str,
    task_id: &str,
) -> Value {
    let mut body = task_rpc(method, task_id);
    body["params"]["metadata"] = json!({ "agent_id": agent_id.to_string() });
    common::as_member(
        server
            .client
            .post(server.url("/api/orchestrator/a2a"))
            .json(&body),
        &user.0.to_string(),
        user.1,
    )
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap()
}

#[tokio::test]
#[serial]
async fn a_live_turn_can_be_read_resubscribed_and_cancelled_over_both_front_doors() {
    let World {
        server,
        agent_id,
        alice,
        bob,
    } = world().await;
    let dir = std::env::temp_dir().join(format!("harness-e2e-{}", Uuid::new_v4().simple()));
    std::fs::create_dir_all(&dir).unwrap();
    let (_key, sid) = session_with_script(&server, &watching_acp(&dir)).await;

    let mut stream = turn_over_proxy(&server, agent_id, (alice, "alice"), &sid).await;
    assert_eq!(stream.status(), 200);
    let (task_id, _) = read_until(&mut stream, "working on it").await;

    // tasks/get while the turn runs: working. Over the proxy and over dispatch.
    let got: Value = proxy_rpc(&server, agent_id, (alice, "alice"), "tasks/get", &task_id)
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(got["id"], "r1");
    assert_eq!(got["result"]["id"], task_id.as_str());
    assert_eq!(got["result"]["contextId"], sid.as_str());
    assert_eq!(
        got["result"]["status"]["state"], "TASK_STATE_WORKING",
        "{got}"
    );
    let got = dispatch_rpc(&server, agent_id, (alice, "alice"), "tasks/get", &task_id).await;
    assert_eq!(
        got["result"]["status"]["state"], "TASK_STATE_WORKING",
        "{got}"
    );

    // Someone else's session is "not found", on both doors, like an unknown id.
    let foreign: Value = proxy_rpc(&server, agent_id, (bob, "bob"), "tasks/get", &task_id)
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(foreign["error"]["code"], -32001, "{foreign}");
    let unknown: Value = proxy_rpc(&server, agent_id, (alice, "alice"), "tasks/get", "nope")
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(unknown["error"]["code"], -32001, "{unknown}");
    let foreign = dispatch_rpc(&server, agent_id, (bob, "bob"), "tasks/cancel", &task_id).await;
    assert_eq!(foreign["error"]["code"], -32001, "{foreign}");

    // tasks/resubscribe: a second stream that receives what is published from now on.
    let mut resub = proxy_rpc(
        &server,
        agent_id,
        (alice, "alice"),
        "tasks/resubscribe",
        &task_id,
    )
    .await;
    assert_eq!(resub.status(), 200);
    assert!(
        resub
            .headers()
            .get("content-type")
            .is_some_and(|v| v.to_str().unwrap().contains("text/event-stream"))
    );

    // tasks/cancel (dispatch): the agent sees session/cancel and the Task comes back canceled.
    let cancelled = dispatch_rpc(
        &server,
        agent_id,
        (alice, "alice"),
        "tasks/cancel",
        &task_id,
    )
    .await;
    assert_eq!(
        cancelled["result"]["status"]["state"], "TASK_STATE_CANCELED",
        "{cancelled}"
    );
    let seen = std::fs::read_to_string(dir.join("seen")).unwrap_or_default();
    assert!(seen.contains("cancel"), "the agent was sent session/cancel");

    // Both streams end canceled.
    let (_, rest) = read_until(&mut stream, "TASK_STATE_CANCELED").await;
    assert!(rest.contains("TASK_STATE_CANCELED"), "{rest}");
    let mut resub_text = String::new();
    while let Some(chunk) = tokio::time::timeout(Duration::from_secs(10), resub.chunk())
        .await
        .expect("resubscribed stream ends")
        .unwrap()
    {
        resub_text.push_str(&String::from_utf8_lossy(&chunk));
    }
    assert!(resub_text.contains("TASK_STATE_CANCELED"), "{resub_text}");

    // After the turn: get says canceled, cancel is not allowed, resubscribe finds no live task.
    settled_session(&server.db, &sid).await;
    let got: Value = proxy_rpc(&server, agent_id, (alice, "alice"), "tasks/get", &task_id)
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(
        got["result"]["status"]["state"], "TASK_STATE_CANCELED",
        "{got}"
    );
    let again: Value = proxy_rpc(
        &server,
        agent_id,
        (alice, "alice"),
        "tasks/cancel",
        &task_id,
    )
    .await
    .json()
    .await
    .unwrap();
    assert_eq!(again["error"]["code"], -32002, "{again}");
    let late: Value = proxy_rpc(
        &server,
        agent_id,
        (alice, "alice"),
        "tasks/resubscribe",
        &task_id,
    )
    .await
    .json()
    .await
    .unwrap();
    assert_eq!(late["error"]["code"], -32001, "{late}");
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_finished_turn_reads_back_completed() {
    let World {
        server,
        agent_id,
        alice,
        ..
    } = world().await;
    let (_key, sid) = session_with_shim(&server).await;
    let mut resp = turn_over_proxy(&server, agent_id, (alice, "alice"), &sid).await;
    let (task_id, _) = read_until(&mut resp, "hello from shim").await;
    while resp.chunk().await.unwrap().is_some() {}
    settled_session(&server.db, &sid).await;

    let got: Value = proxy_rpc(&server, agent_id, (alice, "alice"), "GetTask", &task_id)
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(
        got["result"]["status"]["state"], "TASK_STATE_COMPLETED",
        "{got}"
    );
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn the_harness_card_advertises_streaming_and_no_push() {
    let World {
        server,
        agent_id,
        alice,
        ..
    } = world().await;
    let card: Value = common::as_member(
        server.client.get(server.url(&format!(
            "/api/agents/{agent_id}/.well-known/agent-card.json"
        ))),
        &alice.to_string(),
        "alice",
    )
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    assert_eq!(card["capabilities"]["streaming"], true, "{card}");
    assert_eq!(card["capabilities"]["pushNotifications"], false);
    assert_eq!(card["capabilities"]["stateTransitionHistory"], false);
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_cancel_flag_on_the_row_stops_a_turn_the_request_did_not_reach() {
    let World {
        server,
        agent_id,
        alice,
        ..
    } = world_with(|c| c.coding_agent_lease_heartbeat_secs = 1).await;
    let dir = std::env::temp_dir().join(format!("harness-e2e-{}", Uuid::new_v4().simple()));
    std::fs::create_dir_all(&dir).unwrap();
    let (_key, sid) = session_with_script(&server, &watching_acp(&dir)).await;
    let mut stream = turn_over_proxy(&server, agent_id, (alice, "alice"), &sid).await;
    read_until(&mut stream, "working on it").await;

    // What a cancel served by another replica leaves behind: only the row flag.
    sqlx::query("UPDATE chat_sessions SET cancel_requested_at = now() WHERE session_id = $1")
        .bind(&sid)
        .execute(&server.db)
        .await
        .unwrap();
    let mut text = String::new();
    while let Some(chunk) = tokio::time::timeout(Duration::from_secs(15), stream.chunk())
        .await
        .expect("the heartbeat notices the flag")
        .unwrap()
    {
        text.push_str(&String::from_utf8_lossy(&chunk));
    }
    assert!(text.contains("TASK_STATE_CANCELED"), "{text}");
    assert!(
        std::fs::read_to_string(dir.join("seen"))
            .unwrap()
            .contains("cancel")
    );
    server.cleanup().await;
}

// ---- review round: lease-live state, RAII registry, cancel answers permissions ---------------

/// Streams a chunk, waits a second, then completes: a turn that finishes by itself, slowly enough
/// for a client to walk away from it.
const SLOW_COMPLETE_ACP: &str = r#"
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -E 's/^\{"jsonrpc":"2\.0","id":("[^"]*"|[0-9]+),.*/\1/')
  case "$line" in
    *'"method":"initialize"'*)
      echo "{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":{\"protocolVersion\":1,\"agentCapabilities\":{}}}" ;;
    *'"method":"session/new"'*)
      echo "{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":{\"sessionId\":\"sh-session\"}}" ;;
    *'"method":"session/prompt"'*)
      echo '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"sh-session","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"slow reply"}}}}'
      sleep 1
      echo "{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":{\"stopReason\":\"end_turn\"}}" ;;
  esac
done
"#;

async fn get_state(
    server: &common::TestServer,
    agent_id: Uuid,
    user: (Uuid, &str),
    task: &str,
) -> String {
    let got: Value = proxy_rpc(server, agent_id, user, "tasks/get", task)
        .await
        .json()
        .await
        .unwrap();
    got["result"]["status"]["state"]
        .as_str()
        .unwrap_or_else(|| panic!("no state: {got}"))
        .to_owned()
}

#[tokio::test]
#[serial]
async fn a_turn_survives_its_client_walking_away_and_reads_back_completed() {
    let World {
        server,
        agent_id,
        alice,
        ..
    } = world().await;
    let (_key, sid) = session_with_script(&server, SLOW_COMPLETE_ACP).await;
    let mut stream = turn_over_proxy(&server, agent_id, (alice, "alice"), &sid).await;
    let (task_id, _) = read_until(&mut stream, "slow reply").await;
    drop(stream);

    // Mid-turn it is still working; a second after, it has finished on its own.
    assert_eq!(
        get_state(&server, agent_id, (alice, "alice"), &task_id).await,
        "TASK_STATE_WORKING"
    );
    settled_session(&server.db, &sid).await;
    assert_eq!(
        get_state(&server, agent_id, (alice, "alice"), &task_id).await,
        "TASK_STATE_COMPLETED"
    );
    let reply: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM chat_messages WHERE session_id = $1 AND role = 'assistant' AND content = 'slow reply'",
    )
    .bind(&sid)
    .fetch_one(&server.db)
    .await
    .unwrap();
    assert_eq!(
        reply, 1,
        "the reply was persisted though no client was listening"
    );
    // The finished Task carries the reply.
    let got: Value = proxy_rpc(&server, agent_id, (alice, "alice"), "tasks/get", &task_id)
        .await
        .json()
        .await
        .unwrap();
    assert!(got.to_string().contains("slow reply"), "{got}");
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_turn_whose_lease_lapsed_is_failed_not_working() {
    let World {
        server,
        agent_id,
        alice,
        ..
    } = world().await;
    let sid = Uuid::new_v4().to_string();
    nasiko_server::sessions::SessionStore::new(server.db.clone())
        .find_or_create_for_harness(agent_id, alice, &sid)
        .await
        .unwrap()
        .expect("own session");
    // A crashed replica's turn: it still names the task, but nothing renews its lease.
    sqlx::query(
        "UPDATE chat_sessions SET active_turn_id = 'dead-task', turn_owner = 'gone', \
         lease_expires_at = now() - interval '1 minute' WHERE session_id = $1",
    )
    .bind(&sid)
    .execute(&server.db)
    .await
    .unwrap();

    assert_eq!(
        get_state(&server, agent_id, (alice, "alice"), "dead-task").await,
        "TASK_STATE_FAILED"
    );
    let cancel: Value = proxy_rpc(
        &server,
        agent_id,
        (alice, "alice"),
        "tasks/cancel",
        "dead-task",
    )
    .await
    .json()
    .await
    .unwrap();
    assert_eq!(cancel["error"]["code"], -32002, "{cancel}");
    let resub: Value = proxy_rpc(
        &server,
        agent_id,
        (alice, "alice"),
        "tasks/resubscribe",
        "dead-task",
    )
    .await
    .json()
    .await
    .unwrap();
    assert_eq!(resub["error"]["code"], -32001, "{resub}");
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_request_dropped_while_provisioning_leaves_nothing_registered() {
    let World {
        server,
        agent_id,
        alice,
        ..
    } = world().await;
    let (_key, sid) = session_with_shim(&server).await;
    server.sandbox.delay_next_acquire(Duration::from_secs(4));

    // The client gives up while the sandbox is still being provisioned.
    let sent = common::as_member(
        server
            .client
            .post(server.url(&format!("/api/agents/{agent_id}")))
            .timeout(Duration::from_millis(700))
            .json(&stream_body(&sid, "hi")),
        &alice.to_string(),
        "alice",
    )
    .send()
    .await;
    assert!(sent.is_err(), "the request timed out client-side");

    let mut gone = false;
    for _ in 0..40 {
        if nasiko_server::sessions::live::LiveTurn::registered() == 0 {
            gone = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        gone,
        "the registry entry must not outlive the dropped request"
    );
    server.cleanup().await;
}

/// Asks for a permission when prompted, then waits; on `session/cancel` it answers the prompt as
/// cancelled and records it.
fn permission_acp(dir: &std::path::Path) -> String {
    let dir = dir.display();
    format!(
        r#"
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -E 's/^\{{"jsonrpc":"2\.0","id":("[^"]*"|[0-9]+),.*/\1/')
  case "$line" in
    *'"method":"initialize"'*)
      echo "{{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":{{\"protocolVersion\":1,\"agentCapabilities\":{{}}}}}}" ;;
    *'"method":"session/new"'*)
      echo "{{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":{{\"sessionId\":\"sh-session\"}}}}" ;;
    *'"method":"session/prompt"'*)
      echo "$id" > {dir}/prompt_id
      echo '{{"jsonrpc":"2.0","id":"perm-1","method":"session/request_permission","params":{{"sessionId":"sh-session","toolCall":{{"toolCallId":"call-1","title":"Edit /workspace/a.txt"}},"options":[{{"optionId":"once","name":"Allow once","kind":"allow_once"}},{{"optionId":"no","name":"Reject","kind":"reject_once"}}]}}}}' ;;
    *'"method":"session/cancel"'*)
      echo cancel >> {dir}/seen
      pid=$(cat {dir}/prompt_id)
      echo "{{\"jsonrpc\":\"2.0\",\"id\":$pid,\"result\":{{\"stopReason\":\"cancelled\"}}}}" ;;
  esac
done
"#
    )
}

#[tokio::test]
#[serial]
async fn cancelling_during_a_permission_wait_closes_the_request_and_ends_canceled() {
    let World {
        server,
        agent_id,
        alice,
        ..
    } = world().await;
    sqlx::query("UPDATE agents SET hitl_enabled = true WHERE id = $1")
        .bind(agent_id)
        .execute(&server.db)
        .await
        .unwrap();
    let dir = std::env::temp_dir().join(format!("harness-e2e-{}", Uuid::new_v4().simple()));
    std::fs::create_dir_all(&dir).unwrap();
    let (_key, sid) = session_with_script(&server, &permission_acp(&dir)).await;

    let mut stream = turn_over_proxy(&server, agent_id, (alice, "alice"), &sid).await;
    let (task_id, _) = read_until(&mut stream, "TASK_STATE_INPUT_REQUIRED").await;

    // The turn is blocked on a human: tasks/get says so, and names the approval.
    let mut state = String::new();
    for _ in 0..40 {
        state = get_state(&server, agent_id, (alice, "alice"), &task_id).await;
        if state == "TASK_STATE_INPUT_REQUIRED" {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(state, "TASK_STATE_INPUT_REQUIRED");

    let cancelled = dispatch_rpc(
        &server,
        agent_id,
        (alice, "alice"),
        "tasks/cancel",
        &task_id,
    )
    .await;
    assert_eq!(
        cancelled["result"]["status"]["state"], "TASK_STATE_CANCELED",
        "{cancelled}"
    );
    let mut text = String::new();
    while let Some(chunk) = tokio::time::timeout(Duration::from_secs(15), stream.chunk())
        .await
        .expect("the stream ends")
        .unwrap()
    {
        text.push_str(&String::from_utf8_lossy(&chunk));
    }
    assert!(text.contains("TASK_STATE_CANCELED"), "{text}");
    let status: String = sqlx::query_scalar(
        "SELECT status FROM hitl_requests WHERE agent_id = $1 ORDER BY created_at DESC LIMIT 1",
    )
    .bind(agent_id)
    .fetch_one(&server.db)
    .await
    .unwrap();
    assert_eq!(
        status, "canceled",
        "nobody is left asked to approve a cancelled task"
    );
    server.cleanup().await;
}

/// Answers the handshake and streams a chunk, then ignores everything (including `session/cancel`).
/// Records each start in `<dir>/boots`.
fn deaf_acp(dir: &std::path::Path) -> String {
    let dir = dir.display();
    format!(
        r#"
echo "$$" >> {dir}/boots
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -E 's/^\{{"jsonrpc":"2\.0","id":("[^"]*"|[0-9]+),.*/\1/')
  case "$line" in
    *'"method":"initialize"'*)
      echo "{{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":{{\"protocolVersion\":1,\"agentCapabilities\":{{}}}}}}" ;;
    *'"method":"session/new"'*)
      echo "{{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":{{\"sessionId\":\"sh-session\"}}}}" ;;
    *'"method":"session/prompt"'*)
      echo '{{"jsonrpc":"2.0","method":"session/update","params":{{"sessionId":"sh-session","update":{{"sessionUpdate":"agent_message_chunk","content":{{"type":"text","text":"deaf"}}}}}}}}' ;;
  esac
done
"#
    )
}

#[tokio::test]
#[serial]
async fn a_cancel_the_agent_ignores_ends_the_turn_and_restarts_the_shims_harness() {
    let World {
        server,
        agent_id,
        alice,
        ..
    } = world().await;
    let dir = std::env::temp_dir().join(format!("harness-e2e-{}", Uuid::new_v4().simple()));
    std::fs::create_dir_all(&dir).unwrap();
    let (_key, sid) = session_with_script(&server, &deaf_acp(&dir)).await;

    let mut stream = turn_over_proxy(&server, agent_id, (alice, "alice"), &sid).await;
    let (task_id, _) = read_until(&mut stream, "deaf").await;
    let cancelled = proxy_rpc(
        &server,
        agent_id,
        (alice, "alice"),
        "tasks/cancel",
        &task_id,
    )
    .await
    .json::<Value>()
    .await
    .unwrap();
    assert_eq!(
        cancelled["result"]["status"]["state"], "TASK_STATE_CANCELED",
        "{cancelled}"
    );

    // The driver gave the agent its grace period, ended the turn anyway, and had the shim start
    // the harness again, so the next turn does not attach to one that is still stuck.
    let mut boots = 0;
    for _ in 0..60 {
        boots = std::fs::read_to_string(dir.join("boots"))
            .map(|b| b.lines().count())
            .unwrap_or(0);
        if boots >= 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    assert_eq!(boots, 2, "the harness child was restarted once");
    server.cleanup().await;
}

/// A session whose turn `elsewhere` runs on another replica (a live lease held by someone else).
async fn session_with_foreign_turn(
    server: &common::TestServer,
    agent_id: Uuid,
    alice: Uuid,
) -> String {
    let sid = Uuid::new_v4().to_string();
    nasiko_server::sessions::SessionStore::new(server.db.clone())
        .find_or_create_for_harness(agent_id, alice, &sid)
        .await
        .unwrap()
        .expect("own session");
    sqlx::query(
        "UPDATE chat_sessions SET active_turn_id = 'elsewhere', turn_owner = 'other-replica', \
         lease_expires_at = now() + interval '5 minutes' WHERE session_id = $1",
    )
    .bind(&sid)
    .execute(&server.db)
    .await
    .unwrap();
    sid
}

async fn end_foreign_turn(server: &common::TestServer, sid: &str) {
    sqlx::query(
        "UPDATE chat_sessions SET active_turn_id = NULL, turn_owner = NULL, lease_expires_at = NULL, \
         last_turn_id = 'elsewhere', last_turn_state = 'completed' WHERE session_id = $1",
    )
    .bind(sid)
    .execute(&server.db)
    .await
    .unwrap();
}

async fn drain(resub: &mut reqwest::Response, within: Duration) -> String {
    let mut rest = String::new();
    while let Some(chunk) = tokio::time::timeout(within, resub.chunk())
        .await
        .expect("the stream ends")
        .unwrap()
    {
        rest.push_str(&String::from_utf8_lossy(&chunk));
    }
    rest
}

#[tokio::test]
#[serial]
async fn a_task_running_on_another_replica_is_followed_by_polling_the_row() {
    let World {
        server,
        agent_id,
        alice,
        ..
    } = world().await;
    let sid = session_with_foreign_turn(&server, agent_id, alice).await;

    let mut resub = proxy_rpc(
        &server,
        agent_id,
        (alice, "alice"),
        "tasks/resubscribe",
        "elsewhere",
    )
    .await;
    assert_eq!(resub.status(), 200);
    let first = resub.chunk().await.unwrap().expect("a snapshot");
    let first = String::from_utf8_lossy(&first).to_string();
    assert!(
        first.contains("TASK_STATE_WORKING") && first.contains(r#""id":"elsewhere""#),
        "{first}"
    );

    // The other replica finishes the turn: the stream reports it and ends.
    end_foreign_turn(&server, &sid).await;
    let rest = drain(&mut resub, Duration::from_secs(10)).await;
    assert!(rest.contains("TASK_STATE_COMPLETED"), "{rest}");
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_task_running_on_another_replica_streams_its_frames_through_redis() {
    use nasiko_server::sessions::live::FramePublisher;
    use nasiko_types::a2a::{
        agent_message, completed, status_event, text_part, to_sse_data, working_with_message,
    };
    let World {
        server,
        agent_id,
        alice,
        ..
    } = world().await;
    let sid = session_with_foreign_turn(&server, agent_id, alice).await;
    let mut resub = proxy_rpc(
        &server,
        agent_id,
        (alice, "alice"),
        "tasks/resubscribe",
        "elsewhere",
    )
    .await;
    let first = resub.chunk().await.unwrap().expect("a snapshot");
    assert!(String::from_utf8_lossy(&first).contains(r#""id":"elsewhere""#));

    // The snapshot is out, so the subscriber is live: the owning replica's pump publishes.
    let publisher = FramePublisher::spawn(server.state.redis.client(), "elsewhere");
    publisher
        .publish(to_sse_data(&status_event(working_with_message(
            "elsewhere",
            &sid,
            agent_message(&sid, "elsewhere", text_part("hello from afar")),
        ))))
        .await;
    let (_, seen) = read_until(&mut resub, "hello from afar").await;
    assert!(seen.contains("hello from afar"), "{seen}");

    // The terminal frame ends the stream well before the 5 s row poll would, with the row still live.
    publisher
        .publish(to_sse_data(&status_event(completed("elsewhere", &sid))))
        .await;
    let rest = drain(&mut resub, Duration::from_secs(3)).await;
    assert!(rest.contains("TASK_STATE_COMPLETED"), "{rest}");
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn without_redis_a_foreign_task_still_gets_its_snapshot_and_ends_with_the_row() {
    let World {
        server,
        agent_id,
        alice,
        ..
    } = world_with(|c| c.redis_url = "redis://127.0.0.1:1".into()).await;
    let sid = session_with_foreign_turn(&server, agent_id, alice).await;
    let mut resub = proxy_rpc(
        &server,
        agent_id,
        (alice, "alice"),
        "tasks/resubscribe",
        "elsewhere",
    )
    .await;
    assert_eq!(resub.status(), 200);
    let first = resub.chunk().await.unwrap().expect("a snapshot");
    assert!(String::from_utf8_lossy(&first).contains("TASK_STATE_WORKING"));
    end_foreign_turn(&server, &sid).await;
    let rest = drain(&mut resub, Duration::from_secs(10)).await;
    assert!(rest.contains("TASK_STATE_COMPLETED"), "{rest}");
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn polling_a_task_opens_no_flow_and_records_no_participant() {
    let World {
        server,
        agent_id,
        alice,
        ..
    } = world().await;
    let (_key, sid) = session_with_shim(&server).await;
    let mut resp = turn_over_proxy(&server, agent_id, (alice, "alice"), &sid).await;
    let (task_id, _) = read_until(&mut resp, "hello from shim").await;
    while resp.chunk().await.unwrap().is_some() {}
    settled_session(&server.db, &sid).await;

    let counts = |db: PgPool| async move {
        let flows: i64 = sqlx::query_scalar("SELECT count(*) FROM flows")
            .fetch_one(&db)
            .await
            .unwrap();
        let participants: i64 = sqlx::query_scalar("SELECT count(*) FROM flow_participants")
            .fetch_one(&db)
            .await
            .unwrap();
        (flows, participants)
    };
    let before = counts(server.db.clone()).await;
    for _ in 0..3 {
        assert_eq!(
            get_state(&server, agent_id, (alice, "alice"), &task_id).await,
            "TASK_STATE_COMPLETED"
        );
    }
    assert_eq!(
        counts(server.db.clone()).await,
        before,
        "a poll leaves no flow behind"
    );
    server.cleanup().await;
}

/// A session admission that refuses every turn, as a distribution at its agent limit does.
struct AtTheCap;

#[async_trait::async_trait]
impl SessionAdmission for AtTheCap {
    async fn refuse(&self, _user_id: Uuid, _harness_id: &str) -> Option<SessionRefusal> {
        Some(SessionRefusal {
            status: axum::http::StatusCode::PAYMENT_REQUIRED,
            body: json!({ "error": "You've reached 2 cloud agents", "code": "plan_agent_limit_reached" }),
        })
    }
}

/// The session rows under `context_id`, and the flows `user` owns.
async fn written_for(db: &PgPool, user: Uuid, context_id: &str) -> (i64, i64) {
    let sessions = sqlx::query_scalar("SELECT count(*) FROM chat_sessions WHERE session_id = $1")
        .bind(context_id)
        .fetch_one(db)
        .await
        .unwrap();
    let flows = sqlx::query_scalar("SELECT count(*) FROM flows WHERE user_id = $1")
        .bind(user)
        .fetch_one(db)
        .await
        .unwrap();
    (sessions, flows)
}

fn assert_refused(body: &Value) {
    assert_eq!(body["jsonrpc"], "2.0", "{body}");
    assert_eq!(body["error"]["code"], SESSION_REFUSED, "{body}");
    assert_eq!(body["error"]["message"], "You've reached 2 cloud agents");
    assert_eq!(body["error"]["data"]["code"], "plan_agent_limit_reached");
}

/// A turn the session admission refuses is answered with a JSON-RPC error carrying the refusal,
/// over the proxy (on a 200, like its other harness errors) and over dispatch (with the refusal's
/// status), and writes nothing first: no session row for its contextId, no flow.
#[tokio::test]
#[serial]
async fn a_refused_turn_writes_nothing_and_answers_a_json_rpc_error_over_both_front_doors() {
    let World {
        server,
        agent_id,
        alice,
        ..
    } = world().await;
    server.state.session_admission.install(Arc::new(AtTheCap));

    let context_id = Uuid::new_v4().to_string();
    let resp = turn_over_proxy(&server, agent_id, (alice, "alice"), &context_id).await;
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["id"], "1");
    assert_refused(&body);
    assert_eq!(written_for(&server.db, alice, &context_id).await, (0, 0));

    let context_id = Uuid::new_v4().to_string();
    let mut dispatch = stream_body(&context_id, "say hello");
    dispatch["params"]["message"]["role"] = json!("ROLE_USER");
    dispatch["params"]["metadata"] = json!({ "agent_id": agent_id.to_string() });
    let resp = common::as_member(
        server
            .client
            .post(server.url("/api/orchestrator/a2a"))
            .json(&dispatch),
        &alice.to_string(),
        "alice",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(resp.status(), 402);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["id"], "1");
    assert_refused(&body);
    assert_eq!(written_for(&server.db, alice, &context_id).await, (0, 0));
    assert_eq!(
        server.sandbox.acquire_count(&SandboxKey::Session {
            session_id: context_id.parse().unwrap()
        }),
        0
    );
    server.cleanup().await;
}

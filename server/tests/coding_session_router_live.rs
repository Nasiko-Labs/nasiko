//! Session-token auth through the mounted LLM router (NAS-839).
//!
//! This is the proof that the coding-agent *identity* problem is solved. A coding
//! agent can't be billed via the `traceparent` rule the HTTP agents use, because
//! it never forwards one. Instead each turn carries a session-bound JWT whose
//! claims name {agent_id, user_id, session_id, flow_id}; the router authenticates
//! off those claims and bills the *session's user* with no traceparent and no
//! flow-participant lookup.
//!
//! `real_reply` makes a REAL OpenAI call (so you see an actual completion come
//! back through the routed path) and asserts the spend lands on the session user.
//! It is `#[ignore]` and needs a real key:
//!
//!   OPENAI_API_KEY=sk-... cargo test -p nasiko-server \
//!       --test coding_session_router_live -- --ignored --nocapture --test-threads=1
//!
//! `revoked` needs no upstream — the revocation check refuses the call before any
//! provider is reached — so it runs in CI as a regression guard for the
//! `revoked_session_tokens` producer (`coding::revocation::revoke_session_token`,
//! which the turn path calls at turn end) and its reader. `sweep` guards the
//! companion purge.
//!
//! Every test is `#[serial]`: `GatewayConfig::from_env()` is read once when the
//! app boots, so `AGENT_JWT_SECRET` (and the real key) must be in the environment
//! before `TestServer::start()`.

mod common;

use common::TestServer;
use nasiko_session_token::{SessionTokenInput, mint_session_token};
use serial_test::serial;
use uuid::Uuid;

const JWT_SECRET: &str = "coding-session-live-secret";

async fn seed_user(server: &TestServer, name: &str) -> Uuid {
    sqlx::query_scalar("INSERT INTO users (username, email) VALUES ($1, $2) RETURNING id")
        .bind(name)
        .bind(format!("{name}@test.local"))
        .fetch_one(&server.db)
        .await
        .expect("seed user")
}

async fn seed_agent(server: &TestServer, owner_id: Uuid, name: &str) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO agents (name, owner_id, image, status, workload_kind) \
         VALUES ($1, $2, 'opencode:0.1.0', 'running', 'acp_coding_agent') RETURNING id",
    )
    .bind(name)
    .bind(owner_id)
    .fetch_one(&server.db)
    .await
    .expect("seed agent")
}

/// Pull the `jti` claim out of a minted token (signature unchecked — we only
/// need the id the producer records).
fn jti_of(token: &str) -> String {
    use jsonwebtoken::{DecodingKey, Validation};
    let mut v = Validation::new(jsonwebtoken::Algorithm::HS256);
    v.insecure_disable_signature_validation();
    v.validate_aud = false;
    v.validate_exp = false;
    let data =
        jsonwebtoken::decode::<serde_json::Value>(token, &DecodingKey::from_secret(b"ignored"), &v)
            .expect("decode claims");
    data.claims["jti"].as_str().expect("jti claim").to_owned()
}

/// Mint a session token exactly as the coding-agent turn does.
fn session_token(agent: Uuid, user: Uuid, session_id: &str, flow_id: &str) -> String {
    mint_session_token(
        &SessionTokenInput {
            agent_id: agent.to_string(),
            user_id: user.to_string(),
            session_id: session_id.to_owned(),
            flow_id: flow_id.to_owned(),
        },
        JWT_SECRET,
        3600,
        jsonwebtoken::Algorithm::HS256,
    )
    .expect("mint session token")
    .token
}

fn chat_body() -> serde_json::Value {
    serde_json::json!({
        "model": "gpt-4o-mini",
        "messages": [{
            "role": "user",
            "content": "Reply with only the single digit 4 and nothing else."
        }]
    })
}

/// The usage row is written fire-and-forget, so poll rather than sleep once.
async fn wait_for_usage_row(
    server: &TestServer,
    agent_id: Uuid,
) -> Option<(Option<Uuid>, Option<String>, i32)> {
    for _ in 0..50 {
        let row: Option<(Option<Uuid>, Option<String>, i32)> = sqlx::query_as(
            "SELECT user_id, session_id, \
                    (input_tokens + output_tokens + cache_read_tokens + cache_creation_tokens)::int \
               FROM trace_usage WHERE agent_id = $1 AND priced_by = 'router' \
              ORDER BY materialized_at DESC LIMIT 1",
        )
        .bind(agent_id)
        .fetch_optional(&server.db)
        .await
        .expect("query trace_usage");
        if row.is_some() {
            return row;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    None
}

/// A real reply through the routed path, billed to the session user (NAS-839).
#[tokio::test]
#[serial]
#[ignore = "makes a real OpenAI call; requires OPENAI_API_KEY"]
async fn session_token_gets_real_reply_and_bills_the_session_user() {
    let key = std::env::var("OPENAI_API_KEY")
        .ok()
        .filter(|k| !k.trim().is_empty())
        .expect("set OPENAI_API_KEY to a real key to run this test");
    // SAFETY: serialized by #[serial]; set before the app reads its config.
    unsafe {
        std::env::set_var("AGENT_JWT_SECRET", JWT_SECRET);
        std::env::set_var("PLATFORM_OPENAI_API_KEY", &key);
        // Leave OPENAI_BASE_URL pointing at real OpenAI, not a leftover stub.
        std::env::remove_var("OPENAI_BASE_URL");
    }

    let server = TestServer::start().await;

    // One person owns the agent and is the one chatting — the session token
    // carries their id as the billing subject.
    let user = seed_user(&server, "coding-session-user").await;
    let agent = seed_agent(&server, user, "coding-session-agent").await;

    let flow_id = format!("flow-{}", Uuid::new_v4());
    let token = session_token(agent, user, "coding-session-1", &flow_id);

    let res = server
        .client
        .post(server.url("/v1/chat/completions"))
        .bearer_auth(&token)
        .json(&chat_body())
        .send()
        .await
        .expect("llm request");
    assert_eq!(
        res.status(),
        200,
        "a session-authenticated call must be served"
    );

    let body: serde_json::Value = res.json().await.expect("parse completion json");
    let content = body["choices"][0]["message"]["content"]
        .as_str()
        .unwrap_or_default();
    assert!(
        !content.trim().is_empty(),
        "expected a real model reply, got: {body}"
    );
    assert!(
        content.contains('4'),
        "expected the model's answer to contain 4, got: {content:?}"
    );

    // The spend must follow the session's user — the whole point of the token.
    let (billed_user, session_id, total_tokens) = wait_for_usage_row(&server, agent)
        .await
        .expect("a usage row must be written for a served call");
    assert_eq!(
        billed_user,
        Some(user),
        "spend must be billed to the session token's user"
    );
    assert_eq!(
        session_id.as_deref(),
        Some(flow_id.as_str()),
        "session_id must carry the token's flow_id so per-conversation usage aggregates"
    );
    assert!(total_tokens > 0, "token counts must come from the response");

    server.cleanup().await;
}

/// A revoked session token is refused before any provider is reached. Proves the
/// `revoked_session_tokens` reader works end to end (no upstream needed).
#[tokio::test]
#[serial]
async fn revoked_session_token_is_refused() {
    // SAFETY: serialized by #[serial]; set before the app reads its config.
    unsafe {
        std::env::set_var("AGENT_JWT_SECRET", JWT_SECRET);
    }

    let server = TestServer::start().await;
    let user = seed_user(&server, "coding-revoked-user").await;
    let agent = seed_agent(&server, user, "coding-revoked-agent").await;

    let flow_id = format!("flow-{}", Uuid::new_v4());
    let token = session_token(agent, user, "coding-revoked-1", &flow_id);

    // Revoke it through the real producer, exactly as the turn path does at
    // turn end.
    let jti = jti_of(&token);
    nasiko_server::coding::revocation::revoke_session_token(
        &server.db,
        &jti,
        chrono::Utc::now() + chrono::Duration::hours(1),
    )
    .await
    .expect("revoke token");

    let res = server
        .client
        .post(server.url("/v1/chat/completions"))
        .bearer_auth(&token)
        .json(&chat_body())
        .send()
        .await
        .expect("llm request");
    assert_eq!(
        res.status(),
        403,
        "a revoked session token must be refused before any upstream call"
    );

    server.cleanup().await;
}

/// The sweeper drops revocation rows whose token has expired (dead weight) and
/// keeps the ones that could still verify.
#[tokio::test]
#[serial]
async fn sweep_purges_only_expired_revocations() {
    let server = TestServer::start().await;

    // One already-expired revocation, one still live.
    nasiko_server::coding::revocation::revoke_session_token(
        &server.db,
        "jti-expired",
        chrono::Utc::now() - chrono::Duration::hours(1),
    )
    .await
    .expect("record expired");
    nasiko_server::coding::revocation::revoke_session_token(
        &server.db,
        "jti-live",
        chrono::Utc::now() + chrono::Duration::hours(1),
    )
    .await
    .expect("record live");

    let purged = nasiko_server::coding::revocation::sweep_expired(&server.db)
        .await
        .expect("sweep");
    assert!(purged >= 1, "the expired row must be purged");

    let remaining: Vec<String> =
        sqlx::query_scalar("SELECT jti FROM revoked_session_tokens WHERE jti LIKE 'jti-%'")
            .fetch_all(&server.db)
            .await
            .expect("list remaining");
    assert!(
        remaining.contains(&"jti-live".to_owned())
            && !remaining.contains(&"jti-expired".to_owned()),
        "only the live revocation should survive the sweep; got {remaining:?}"
    );

    server.cleanup().await;
}

/// A stub OpenAI upstream, so the routed path needs no real key. Keep the guard alive.
async fn stub_upstream() -> mockito::ServerGuard {
    let mut upstream = mockito::Server::new_async().await;
    upstream
        .mock("POST", "/v1/chat/completions")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            r#"{"id":"chatcmpl-1","object":"chat.completion","created":1,
                "model":"gpt-4o-mini",
                "choices":[{"index":0,"message":{"role":"assistant","content":"hi"},
                            "finish_reason":"stop"}],
                "usage":{"prompt_tokens":11,"completion_tokens":7,"total_tokens":18}}"#,
        )
        .expect_at_least(0)
        .create_async()
        .await;
    // SAFETY: serialized by #[serial]; set before the app reads its config.
    unsafe {
        std::env::set_var("OPENAI_BASE_URL", upstream.url());
        std::env::set_var("AGENT_JWT_SECRET", JWT_SECRET);
        std::env::set_var("PLATFORM_OPENAI_API_KEY", "sk-platform-test");
    }
    upstream
}

async fn seed_flow(
    server: &TestServer,
    flow_id: &str,
    user: Uuid,
    session: &str,
    participant: Uuid,
) {
    sqlx::query("INSERT INTO flows (flow_id, user_id, session_id) VALUES ($1, $2, $3)")
        .bind(flow_id)
        .bind(user)
        .bind(session)
        .execute(&server.db)
        .await
        .expect("seed flow");
    sqlx::query("INSERT INTO flow_participants (flow_id, agent_id) VALUES ($1, $2)")
        .bind(flow_id)
        .bind(participant)
        .execute(&server.db)
        .await
        .expect("seed participant");
}

/// Send one chat call with the session token (no `traceparent`, as a harness CLI does) and
/// return the `session_id` and attribution source the usage row was written under.
async fn usage_session_for(
    server: &TestServer,
    agent: Uuid,
    token: &str,
) -> (Option<String>, String) {
    let res = server
        .client
        .post(server.url("/v1/chat/completions"))
        .bearer_auth(token)
        .json(&chat_body())
        .send()
        .await
        .expect("llm request");
    assert_eq!(res.status(), 200, "a session-authenticated call is served");
    let session = wait_for_usage_row(server, agent)
        .await
        .expect("usage row")
        .1;
    let source: Option<String> = sqlx::query_scalar(
        "SELECT metadata->>'attribution' FROM token_usage \
          WHERE agent_id = $1 ORDER BY created_at DESC LIMIT 1",
    )
    .bind(agent)
    .fetch_one(&server.db)
    .await
    .expect("usage attribution");
    (session, source.unwrap_or_default())
}

/// A later turn: the token still claims the first turn's flow A, but the session has a newer
/// flow B with the agent as participant, so usage lands on B without any `traceparent`.
#[tokio::test]
#[serial]
async fn later_turn_usage_follows_the_sessions_newest_flow() {
    let _upstream = stub_upstream().await;
    let server = TestServer::start().await;
    let user = seed_user(&server, "coding-turn2-user").await;
    let agent = seed_agent(&server, user, "coding-turn2-agent").await;

    let flow_a = format!("flow-a-{}", Uuid::new_v4().simple());
    let flow_b = format!("flow-b-{}", Uuid::new_v4().simple());
    seed_flow(&server, &flow_a, user, "ses-turns", agent).await;
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    seed_flow(&server, &flow_b, user, "ses-turns", agent).await;
    let token = session_token(agent, user, "ses-turns", &flow_a);

    let (session, source) = usage_session_for(&server, agent, &token).await;
    assert_eq!(session.as_deref(), Some(flow_b.as_str()));
    assert_eq!(source, "session");

    server.cleanup().await;
}

/// A flow in a different session is never adopted: the claim stands.
#[tokio::test]
#[serial]
async fn another_sessions_flow_is_not_adopted() {
    let _upstream = stub_upstream().await;
    let server = TestServer::start().await;
    let user = seed_user(&server, "coding-foreign-user").await;
    let agent = seed_agent(&server, user, "coding-foreign-agent").await;

    let flow_a = format!("flow-a-{}", Uuid::new_v4().simple());
    let foreign = format!("flow-x-{}", Uuid::new_v4().simple());
    seed_flow(&server, &flow_a, user, "ses-mine", agent).await;
    seed_flow(&server, &foreign, user, "ses-other", agent).await;
    let token = session_token(agent, user, "ses-mine", &flow_a);
    // Flip the order so the foreign session's flow is the newest row overall.
    sqlx::query("UPDATE flows SET created_at = now() + interval '1 second' WHERE flow_id = $1")
        .bind(&foreign)
        .execute(&server.db)
        .await
        .expect("reorder");

    let (session, source) = usage_session_for(&server, agent, &token).await;
    assert_eq!(session.as_deref(), Some(flow_a.as_str()));
    assert_eq!(
        source, "session",
        "the claim flow is itself the session's newest flow"
    );

    server.cleanup().await;
}

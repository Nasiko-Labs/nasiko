//! Integration tests for chat session and message list endpoints.
//!
//! Covers:
//!   - GET /chat/sessions: cursor round-trip (first page, next page, stable ordering)
//!   - GET /chat/sessions/{id}/messages: cursor round-trip (first load, before, after)
//!   - Ownership: user B cannot read user A's messages (404)
//!   - CursorPage envelope: has_more, next_cursor, prev_cursor present and correct
//!
//! Requires infra (Postgres :5432, Redis, S3):
//!   cargo test -p nasiko-server --test chat_sessions -- --test-threads=1

mod common;

use serde_json::{Value, json};
use serial_test::serial;

// ─── helpers ────────────────────────────────────────────────────────────────

async fn init_admin(server: &common::TestServer) -> Value {
    server
        .client
        .post(server.url("/api/auth/initialize-admin"))
        .json(&json!({"username": "admin", "email": "admin@test.local"}))
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap()
}

async fn create_user(server: &common::TestServer, admin_id: &str, username: &str) -> Value {
    common::as_superuser(
        server.client.post(server.url("/api/users")),
        admin_id,
        "admin",
    )
    .json(&json!({"username": username, "email": format!("{username}@test.local")}))
    .send()
    .await
    .unwrap()
    .json::<Value>()
    .await
    .unwrap()
}

async fn create_session(server: &common::TestServer, uid: &str, title: &str) -> Value {
    common::as_superuser(
        server.client.post(server.url("/api/chat/sessions")),
        uid,
        "admin",
    )
    .json(&json!({"title": title}))
    .send()
    .await
    .unwrap()
    .json::<Value>()
    .await
    .unwrap()
}

async fn send_message(server: &common::TestServer, uid: &str, sid: &str, content: &str) -> Value {
    common::as_superuser(
        server
            .client
            .post(server.url(&format!("/api/chat/sessions/{sid}/messages"))),
        uid,
        "admin",
    )
    .json(&json!({"role": "user", "content": content}))
    .send()
    .await
    .unwrap()
    .json::<Value>()
    .await
    .unwrap()
}

async fn list_sessions(server: &common::TestServer, uid: &str, query: &str) -> Value {
    common::as_superuser(
        server
            .client
            .get(server.url(&format!("/api/chat/sessions{query}"))),
        uid,
        "admin",
    )
    .send()
    .await
    .unwrap()
    .json::<Value>()
    .await
    .unwrap()
}

async fn list_messages(
    server: &common::TestServer,
    uid: &str,
    sid: &str,
    query: &str,
) -> reqwest::Response {
    common::as_superuser(
        server
            .client
            .get(server.url(&format!("/api/chat/sessions/{sid}/messages{query}"))),
        uid,
        "admin",
    )
    .send()
    .await
    .unwrap()
}

// ─── Session cursor pagination ───────────────────────────────────────────────

#[tokio::test]
#[serial]
async fn list_sessions_cursor_round_trip() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    // Create 3 sessions — they'll be ordered by updated_at DESC.
    create_session(&server, uid, "session-a").await;
    create_session(&server, uid, "session-b").await;
    create_session(&server, uid, "session-c").await;

    // First page: limit=2. Expect 2 rows, has_more=true, next_cursor set.
    let page1 = list_sessions(&server, uid, "?limit=2").await;
    let data1 = page1["data"].as_array().unwrap();
    assert_eq!(data1.len(), 2, "first page should have 2 sessions");
    assert!(page1["has_more"].as_bool().unwrap());
    assert!(
        page1["next_cursor"].is_string(),
        "next_cursor must be present"
    );

    let next_cursor = page1["next_cursor"].as_str().unwrap();
    let page1_ids: Vec<&str> = data1
        .iter()
        .map(|s| s["session_id"].as_str().unwrap())
        .collect();

    // Second page via cursor. Expect 1 row, has_more=false.
    let page2 = list_sessions(&server, uid, &format!("?limit=2&cursor={next_cursor}")).await;
    let data2 = page2["data"].as_array().unwrap();
    assert_eq!(
        data2.len(),
        1,
        "second page should have the remaining session"
    );
    assert!(!page2["has_more"].as_bool().unwrap());

    // No overlap between pages.
    let page2_ids: Vec<&str> = data2
        .iter()
        .map(|s| s["session_id"].as_str().unwrap())
        .collect();
    for id in &page2_ids {
        assert!(!page1_ids.contains(id), "pages must not overlap");
    }

    // All 3 distinct sessions accounted for.
    let mut all_ids = page1_ids.clone();
    all_ids.extend(page2_ids);
    all_ids.dedup();
    assert_eq!(
        all_ids.len(),
        3,
        "all 3 sessions should appear across both pages"
    );

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn list_sessions_scoped_to_owner() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    let other = create_user(&server, uid, "other-sess").await;
    let other_id = other["id"].as_str().unwrap();

    create_session(&server, uid, "admin-session").await;

    // other user creates their own session (non-superuser)
    common::as_member(
        server.client.post(server.url("/api/chat/sessions")),
        other_id,
        "other-sess",
    )
    .json(&json!({"title": "other-session"}))
    .send()
    .await
    .unwrap();

    // Admin sees only their own session, not other's.
    let page = list_sessions(&server, uid, "").await;
    let data = page["data"].as_array().unwrap();
    assert_eq!(data.len(), 1, "admin should only see their own session");
    assert_eq!(data[0]["title"].as_str().unwrap(), "admin-session");

    server.cleanup().await;
}

// ─── Message cursor pagination ───────────────────────────────────────────────

#[tokio::test]
#[serial]
async fn list_messages_cursor_round_trip() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    let session = create_session(&server, uid, "msg-cursor-test").await;
    let sid = session["session_id"].as_str().unwrap();

    send_message(&server, uid, sid, "msg-1").await;
    send_message(&server, uid, sid, "msg-2").await;
    send_message(&server, uid, sid, "msg-3").await;

    // First load (no anchor): should return the 2 most recent in ASC order, has_more=true.
    let res = list_messages(&server, uid, sid, "?limit=2").await;
    assert_eq!(res.status(), 200);
    let page1: Value = res.json().await.unwrap();
    let data1 = page1["data"].as_array().unwrap();
    assert_eq!(data1.len(), 2);
    assert!(page1["has_more"].as_bool().unwrap());
    assert!(page1["next_cursor"].is_string());
    // Data is in ASC order — earlier message comes first.
    assert!(
        data1[0]["timestamp"].as_str() <= data1[1]["timestamp"].as_str(),
        "messages must be in ascending order"
    );

    // Load older messages via prev_cursor from the first result.
    // There's only 1 older message (msg-1 is the oldest, page1 has msg-2 and msg-3... wait
    // actually no anchor DESC: returns most recent 2 = msg-3, msg-2 reversed to ASC = msg-2, msg-3).
    // prev_cursor on page1 points before msg-2, so loading older → msg-1.
    let prev_cursor = page1["prev_cursor"].as_str().unwrap();
    let res2 = list_messages(
        &server,
        uid,
        sid,
        &format!("?limit=2&prev_cursor={prev_cursor}"),
    )
    .await;
    assert_eq!(res2.status(), 200);
    let page2: Value = res2.json().await.unwrap();
    let data2 = page2["data"].as_array().unwrap();
    assert_eq!(data2.len(), 1, "one older message before the first page");
    assert_eq!(data2[0]["content"].as_str().unwrap(), "msg-1");

    server.cleanup().await;
}

// ─── list_sessions: no dead prev_cursor (fix #3) ────────────────────────────

#[tokio::test]
#[serial]
async fn list_sessions_response_has_no_prev_cursor() {
    // Regression: `list_sessions` has no backward-paging input at all
    // (`ListSessionsParams` only has a forward `cursor`), so emitting a
    // `prev_cursor` implied a capability the API doesn't actually have. The
    // field must now always be null for this endpoint.
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    create_session(&server, uid, "solo-session").await;

    let page = list_sessions(&server, uid, "").await;
    assert!(
        page["prev_cursor"].is_null(),
        "list_sessions must not emit a usable prev_cursor: {page}"
    );

    server.cleanup().await;
}

// ─── create_session: agent_id validation + agent_url SSRF (fix #2) ─────────

#[tokio::test]
#[serial]
async fn create_session_rejects_nonexistent_agent_id() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    let fake_agent_id = uuid::Uuid::new_v4().to_string();
    let res = common::as_superuser(
        server.client.post(server.url("/api/chat/sessions")),
        uid,
        "admin",
    )
    .json(&json!({"agent_id": fake_agent_id, "title": "t"}))
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 400, "non-existent agent_id must be rejected");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn create_session_ignores_client_supplied_agent_url() {
    // Regression for SEC fix #2 (stored-SSRF): `agent_url` must never be taken
    // from client input — the canonical URL is always resolved server-side
    // from the agent's own row, never from the request body.
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    let agent: Value =
        common::as_superuser(server.client.post(server.url("/api/agents")), uid, "admin")
            .json(&json!({"name": "ssrf-target-agent", "version": "1.0.0"}))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
    let agent_id = agent["id"].as_str().unwrap();

    let session: Value = common::as_superuser(
        server.client.post(server.url("/api/chat/sessions")),
        uid,
        "admin",
    )
    .json(&json!({
        "agent_id": agent_id,
        "agent_url": "http://169.254.169.254/latest/meta-data/",
        "title": "ssrf-attempt"
    }))
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();

    assert_eq!(session["agent_id"], agent_id);
    assert_ne!(
        session["agent_url"], "http://169.254.169.254/latest/meta-data/",
        "client-supplied agent_url must never be persisted verbatim"
    );

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn create_session_rejects_inaccessible_agent() {
    // A non-superuser with no grant on someone else's private agent must not
    // be able to pin a chat session to it.
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let admin_id = admin["user_id"].as_str().unwrap();

    let agent: Value = common::as_superuser(
        server.client.post(server.url("/api/agents")),
        admin_id,
        "admin",
    )
    .json(&json!({"name": "private-agent-for-session-test", "version": "1.0.0"}))
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    let agent_id = agent["id"].as_str().unwrap();

    let other = create_user(&server, admin_id, "session-other").await;
    let other_id = other["id"].as_str().unwrap();

    let res = common::as_member(
        server.client.post(server.url("/api/chat/sessions")),
        other_id,
        "session-other",
    )
    .json(&json!({"agent_id": agent_id, "title": "nope"}))
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 403, "inaccessible agent_id must be rejected");

    server.cleanup().await;
}

// ─── Ownership ───────────────────────────────────────────────────────────────

#[tokio::test]
#[serial]
async fn list_messages_returns_404_for_other_users_session() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    // Admin creates a session and posts a message.
    let session = create_session(&server, uid, "private-session").await;
    let sid = session["session_id"].as_str().unwrap();
    send_message(&server, uid, sid, "secret").await;

    // Create a second user.
    let other = create_user(&server, uid, "eavesdropper").await;
    let other_id = other["id"].as_str().unwrap();

    // Other user tries to read admin's messages — must get 404, not the messages.
    let res = common::as_member(
        server
            .client
            .get(server.url(&format!("/api/chat/sessions/{sid}/messages"))),
        other_id,
        "eavesdropper",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(
        res.status(),
        404,
        "other user must not read another user's messages"
    );

    server.cleanup().await;
}

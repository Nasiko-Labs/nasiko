//! `/api/orchestrator/a2a` must not read or write a chat session that belongs to another user.
mod common;

use serde_json::json;
use serial_test::serial;

async fn seed_user(server: &common::TestServer, username: &str) -> uuid::Uuid {
    sqlx::query_scalar(
        "INSERT INTO users (username, email, is_superuser, is_active) \
         VALUES ($1, $2, false, true) RETURNING id",
    )
    .bind(username)
    .bind(format!("{username}@local.test"))
    .fetch_one(&server.db)
    .await
    .expect("seed user")
}

async fn seed_session(server: &common::TestServer, session_id: &str, owner: uuid::Uuid) {
    sqlx::query(
        "INSERT INTO chat_sessions (session_id, user_id, agent_url, title, session_type) \
         VALUES ($1, $2, '/api/orchestrator/a2a', 'private chat', 'orchestrator')",
    )
    .bind(session_id)
    .bind(owner)
    .execute(&server.db)
    .await
    .expect("seed session");
    sqlx::query(
        "INSERT INTO chat_messages (session_id, role, content) VALUES ($1, 'user', 'canary-A')",
    )
    .bind(session_id)
    .execute(&server.db)
    .await
    .expect("seed message");
}

fn send(context_id: &str, text: &str) -> serde_json::Value {
    json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "message/stream",
        "params": {
            "message": {
                "messageId": uuid::Uuid::new_v4().to_string(),
                "role": "ROLE_USER",
                "contextId": context_id,
                "parts": [{ "text": text }]
            }
        }
    })
}

async fn messages_in(server: &common::TestServer, session_id: &str) -> Vec<String> {
    sqlx::query_scalar("SELECT content FROM chat_messages WHERE session_id = $1 ORDER BY timestamp")
        .bind(session_id)
        .fetch_all(&server.db)
        .await
        .unwrap()
}

#[tokio::test]
#[serial]
async fn another_users_session_is_refused_and_left_untouched() {
    let server = common::TestServer::start().await;
    let alice = seed_user(&server, "dummy-alice").await;
    let bob = seed_user(&server, "dummy-bob").await;
    seed_session(&server, "sess-alice-1", alice).await;

    let resp = common::as_member(
        server.client.post(server.url("/api/orchestrator/a2a")),
        &bob.to_string(),
        "dummy-bob",
    )
    .json(&send("sess-alice-1", "canary-B"))
    .send()
    .await
    .unwrap();

    assert_eq!(resp.status(), reqwest::StatusCode::FORBIDDEN);
    assert_eq!(
        messages_in(&server, "sess-alice-1").await,
        vec!["canary-A".to_string()]
    );

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn the_owner_can_still_continue_their_own_session() {
    let server = common::TestServer::start().await;
    let alice = seed_user(&server, "dummy-alice2").await;
    seed_session(&server, "sess-alice-2", alice).await;

    let resp = common::as_member(
        server.client.post(server.url("/api/orchestrator/a2a")),
        &alice.to_string(),
        "dummy-alice2",
    )
    .json(&send("sess-alice-2", "canary-A2"))
    .send()
    .await
    .unwrap();

    assert_ne!(resp.status(), reqwest::StatusCode::FORBIDDEN);
    let _ = resp.text().await;
    assert!(
        messages_in(&server, "sess-alice-2")
            .await
            .contains(&"canary-A2".to_string()),
        "the owner's own message was not recorded"
    );

    server.cleanup().await;
}

//! Integration tests for skill-tag discovery (`GET /api/agents/by-skill`)
//! and the agent_skills projection kept in sync by catalog create/update.
//!
//! Uses the TestServer harness (isolated DB per test, gateway-header simulation).
//! Pure DB/HTTP — no Docker. Requires infra (Postgres :5432, Redis, S3).
//!   `cargo test -p nasiko-server --test agent_skills -- --test-threads=1`

mod common;

use serde_json::{Value, json};
use serial_test::serial;

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

/// POST /api/agents as a superuser; returns the created agent JSON.
async fn create_agent(server: &common::TestServer, uid: &str, body: Value) -> Value {
    let res = common::as_superuser(server.client.post(server.url("/api/agents")), uid, "admin")
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 201, "create agent should succeed");
    res.json::<Value>().await.unwrap()
}

/// GET by-skill with the given identity; returns (status, names).
async fn by_skill(
    server: &common::TestServer,
    uid: &str,
    is_super: bool,
    query: &str,
) -> (u16, Vec<String>) {
    let rb = server
        .client
        .get(server.url(&format!("/api/agents/by-skill?{query}")));
    let res = if is_super {
        common::as_superuser(rb, uid, "u")
    } else {
        common::as_member(rb, uid, "u")
    }
    .send()
    .await
    .unwrap();
    let status = res.status().as_u16();
    if status != 200 {
        return (status, vec![]);
    }
    let arr: Value = res.json().await.unwrap();
    let names = arr
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|a| a["name"].as_str().map(String::from))
        .collect();
    (status, names)
}

fn skill(id: &str, tags: &[&str]) -> Value {
    json!({"id": id, "name": format!("{id}-n"), "description": "d", "tags": tags})
}

#[tokio::test]
#[serial]
async fn by_skill_finds_and_excludes() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    create_agent(
        &server,
        uid,
        json!({"name": "nlp-agent", "skills": [skill("s1", &["nlp", "text"])]}),
    )
    .await;

    let (st, names) = by_skill(&server, uid, true, "tag=nlp").await;
    assert_eq!(st, 200);
    assert!(
        names.contains(&"nlp-agent".to_string()),
        "tag match: {names:?}"
    );

    let (_, none) = by_skill(&server, uid, true, "tag=does-not-exist").await;
    assert!(none.is_empty(), "non-matching tag returns empty");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn update_resyncs_skill_tags() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    let agent = create_agent(
        &server,
        uid,
        json!({"name": "mut-agent", "skills": [skill("s1", &["nlp"])]}),
    )
    .await;
    let id = agent["id"].as_str().unwrap();

    // Replace skills: drop "nlp", add "vision".
    let res = common::as_superuser(
        server.client.put(server.url(&format!("/api/agents/{id}"))),
        uid,
        "admin",
    )
    .json(&json!({"skills": [skill("s1", &["vision"])]}))
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 200);

    let (_, nlp) = by_skill(&server, uid, true, "tag=nlp").await;
    assert!(
        !nlp.contains(&"mut-agent".to_string()),
        "old tag dropped after update"
    );
    let (_, vision) = by_skill(&server, uid, true, "tag=vision").await;
    assert!(
        vision.contains(&"mut-agent".to_string()),
        "new tag present after update"
    );

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn no_skills_returns_empty() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    create_agent(&server, uid, json!({"name": "bare-agent"})).await;
    let (_, names) = by_skill(&server, uid, true, "tag=anything").await;
    assert!(names.is_empty());

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn empty_tag_is_bad_request() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    let (st, _) = by_skill(&server, uid, true, "tag=").await;
    assert_eq!(st, 400, "empty tag → 400");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn limit_is_honored() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    create_agent(
        &server,
        uid,
        json!({"name": "a1", "skills": [skill("s", &["shared"])]}),
    )
    .await;
    create_agent(
        &server,
        uid,
        json!({"name": "a2", "skills": [skill("s", &["shared"])]}),
    )
    .await;

    let (_, both) = by_skill(&server, uid, true, "tag=shared").await;
    assert_eq!(both.len(), 2, "both agents share the tag");

    let (_, one) = by_skill(&server, uid, true, "tag=shared&limit=1").await;
    assert_eq!(one.len(), 1, "limit honored");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn by_skill_is_owner_scoped() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let admin_id = admin["user_id"].as_str().unwrap();

    // Admin owns the agent.
    create_agent(
        &server,
        admin_id,
        json!({"name": "owned-agent", "skills": [skill("s1", &["secret"])]}),
    )
    .await;

    // A separate member must NOT see it via by-skill.
    let alice = create_user(&server, admin_id, "alice").await;
    let alice_id = alice["id"].as_str().unwrap();
    let (st, names) = by_skill(&server, alice_id, false, "tag=secret").await;
    assert_eq!(st, 200);
    assert!(
        !names.contains(&"owned-agent".to_string()),
        "IDOR guard: member sees only own agents"
    );

    // Superuser does see it.
    let (_, admin_view) = by_skill(&server, admin_id, true, "tag=secret").await;
    assert!(admin_view.contains(&"owned-agent".to_string()));

    server.cleanup().await;
}

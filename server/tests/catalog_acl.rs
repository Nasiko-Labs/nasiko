//! Integration tests for catalog read-endpoint ACL enforcement and skill-tag dedup.
//!
//! Covers:
//!   - GET /api/agents/{id}       — non-owner gets 403, superuser gets through
//!   - GET /api/agents/{id}/versions — non-owner gets 403
//!   - GET /api/search/agents     — non-owner only sees their own agents
//!   - POST /api/agents           — skill tags are merged into agent.tags on create
//!   - PUT  /api/agents/{id}      — skill tags are merged into agent.tags on update
//!
//! Requires infra (Postgres :5432, Redis, S3):
//!   cargo test -p nasiko-server --test catalog_acl -- --test-threads=1

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
    server
        .client
        .post(server.url("/api/users"))
        .header("x-user-id", admin_id)
        .header("x-username", "admin")
        .header("x-is-superuser", "true")
        .header("x-user-role", "admin")
        .json(&json!({"username": username, "email": format!("{username}@test.local")}))
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap()
}

async fn create_agent(server: &common::TestServer, uid: &str, body: Value) -> Value {
    let res = server
        .client
        .post(server.url("/api/agents"))
        .header("x-user-id", uid)
        .header("x-username", "admin")
        .header("x-is-superuser", "true")
        .header("x-user-role", "admin")
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 201, "create agent should succeed");
    res.json::<Value>().await.unwrap()
}

async fn get_agent(server: &common::TestServer, uid: &str, is_super: bool, id: &str) -> reqwest::Response {
    server
        .client
        .get(server.url(&format!("/api/agents/{id}")))
        .header("x-user-id", uid)
        .header("x-username", "u")
        .header("x-is-superuser", if is_super { "true" } else { "false" })
        .header("x-user-role", if is_super { "admin" } else { "member" })
        .send()
        .await
        .unwrap()
}

async fn list_versions(server: &common::TestServer, uid: &str, is_super: bool, agent_id: &str) -> reqwest::Response {
    server
        .client
        .get(server.url(&format!("/api/agents/{agent_id}/versions")))
        .header("x-user-id", uid)
        .header("x-username", "u")
        .header("x-is-superuser", if is_super { "true" } else { "false" })
        .header("x-user-role", if is_super { "admin" } else { "member" })
        .send()
        .await
        .unwrap()
}

async fn search(server: &common::TestServer, uid: &str, is_super: bool, q: &str) -> Vec<Value> {
    let res = server
        .client
        .get(server.url(&format!("/api/search/agents?q={q}")))
        .header("x-user-id", uid)
        .header("x-username", "u")
        .header("x-is-superuser", if is_super { "true" } else { "false" })
        .header("x-user-role", if is_super { "admin" } else { "member" })
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    res.json::<Vec<Value>>().await.unwrap()
}

async fn update_agent(server: &common::TestServer, uid: &str, agent_id: &str, body: Value) -> Value {
    let res = server
        .client
        .put(server.url(&format!("/api/agents/{agent_id}")))
        .header("x-user-id", uid)
        .header("x-username", "admin")
        .header("x-is-superuser", "true")
        .header("x-user-role", "admin")
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200, "update agent should succeed");
    res.json::<Value>().await.unwrap()
}

fn skill(id: &str, tags: &[&str]) -> Value {
    json!({"id": id, "name": format!("{id}-name"), "description": "desc", "tags": tags})
}

// ─── get_one ACL ─────────────────────────────────────────────────────────────

#[tokio::test]
#[serial]
async fn get_one_returns_403_for_non_owner() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    let agent = create_agent(&server, uid, json!({"name": "acl-get-agent", "version": "1.0.0"})).await;
    let agent_id = agent["id"].as_str().unwrap();

    let other = create_user(&server, uid, "acl-get-other").await;
    let other_id = other["id"].as_str().unwrap();

    let res = get_agent(&server, other_id, false, agent_id).await;
    assert_eq!(res.status(), 403, "non-owner must get 403 on get_one");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn get_one_superuser_sees_any_agent() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    let agent = create_agent(&server, uid, json!({"name": "acl-super-agent", "version": "1.0.0"})).await;
    let agent_id = agent["id"].as_str().unwrap();

    // A second superuser can access any agent.
    let other = create_user(&server, uid, "acl-super-other").await;
    let other_id = other["id"].as_str().unwrap();

    let res = get_agent(&server, other_id, true, agent_id).await;
    assert_eq!(res.status(), 200, "superuser must be able to get any agent");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn get_one_by_name_returns_403_for_non_owner() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    create_agent(&server, uid, json!({"name": "acl-name-agent", "version": "1.0.0"})).await;

    let other = create_user(&server, uid, "acl-name-other").await;
    let other_id = other["id"].as_str().unwrap();

    let res = get_agent(&server, other_id, false, "acl-name-agent").await;
    assert_eq!(res.status(), 403, "non-owner must get 403 when looking up agent by name");

    server.cleanup().await;
}

// ─── list_versions ACL ───────────────────────────────────────────────────────

#[tokio::test]
#[serial]
async fn list_versions_returns_403_for_non_owner() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    let agent = create_agent(&server, uid, json!({"name": "acl-versions-agent", "version": "1.0.0"})).await;
    let agent_id = agent["id"].as_str().unwrap();

    let other = create_user(&server, uid, "acl-versions-other").await;
    let other_id = other["id"].as_str().unwrap();

    let res = list_versions(&server, other_id, false, agent_id).await;
    assert_eq!(res.status(), 403, "non-owner must get 403 on list_versions");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn list_versions_superuser_sees_any_agent() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    let agent = create_agent(&server, uid, json!({"name": "acl-versions-super", "version": "1.0.0"})).await;
    let agent_id = agent["id"].as_str().unwrap();

    let other = create_user(&server, uid, "acl-vers-super-other").await;
    let other_id = other["id"].as_str().unwrap();

    let res = list_versions(&server, other_id, true, agent_id).await;
    assert_eq!(res.status(), 200, "superuser must be able to list versions for any agent");

    server.cleanup().await;
}

// ─── search owner scoping ────────────────────────────────────────────────────

#[tokio::test]
#[serial]
async fn search_is_owner_scoped() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    let other = create_user(&server, uid, "srch-other").await;
    let other_id = other["id"].as_str().unwrap();

    // Admin owns one agent; other user owns a second one (created via admin elevation).
    create_agent(&server, uid, json!({"name": "srch-admin-agent", "version": "1.0.0"})).await;

    let other_agent = server
        .client
        .post(server.url("/api/agents"))
        .header("x-user-id", other_id)
        .header("x-username", "srch-other")
        .header("x-is-superuser", "false")
        .header("x-user-role", "member")
        .json(&json!({"name": "srch-other-agent", "version": "1.0.0"}))
        .send()
        .await
        .unwrap();
    assert_eq!(other_agent.status(), 201);

    // Non-superuser search: only their own agent.
    let results = search(&server, other_id, false, "srch").await;
    let names: Vec<&str> = results.iter().filter_map(|a| a["name"].as_str()).collect();
    assert!(names.contains(&"srch-other-agent"), "user must see their own agent");
    assert!(!names.contains(&"srch-admin-agent"), "user must not see other's agent");

    // Superuser search: both agents.
    let all = search(&server, uid, true, "srch").await;
    let all_names: Vec<&str> = all.iter().filter_map(|a| a["name"].as_str()).collect();
    assert!(all_names.contains(&"srch-admin-agent"));
    assert!(all_names.contains(&"srch-other-agent"));

    server.cleanup().await;
}

// ─── skill-tag dedup ─────────────────────────────────────────────────────────

#[tokio::test]
#[serial]
async fn create_with_skills_merges_skill_tags() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    let agent = create_agent(&server, uid, json!({
        "name": "tag-merge-create",
        "version": "1.0.0",
        "tags": ["explicit-tag"],
        "skills": [
            skill("s1", &["nlp", "streaming"]),
            skill("s2", &["nlp", "vision"]),   // "nlp" appears in both skills
        ]
    }))
    .await;

    let tags: Vec<&str> = agent["tags"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|t| t.as_str())
        .collect();

    assert!(tags.contains(&"explicit-tag"), "explicit tag must be preserved");
    assert!(tags.contains(&"nlp"),          "skill tag 'nlp' must be included");
    assert!(tags.contains(&"streaming"),    "skill tag 'streaming' must be included");
    assert!(tags.contains(&"vision"),       "skill tag 'vision' must be included");

    let nlp_count = tags.iter().filter(|&&t| t == "nlp").count();
    assert_eq!(nlp_count, 1, "'nlp' must appear exactly once after dedup");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn update_with_skills_merges_skill_tags() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    let agent = create_agent(&server, uid, json!({
        "name": "tag-merge-update",
        "version": "1.0.0",
        "tags": ["pre-existing"],
    }))
    .await;
    let agent_id = agent["id"].as_str().unwrap();

    let updated = update_agent(&server, uid, agent_id, json!({
        "tags": ["pre-existing", "added"],
        "skills": [skill("upd-s1", &["added", "from-skill"])],
    }))
    .await;

    let tags: Vec<&str> = updated["tags"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|t| t.as_str())
        .collect();

    assert!(tags.contains(&"pre-existing"), "pre-existing tag must be preserved");
    assert!(tags.contains(&"added"),        "'added' from both explicit tags and skill must appear once");
    assert!(tags.contains(&"from-skill"),   "skill-only tag must be included");

    let added_count = tags.iter().filter(|&&t| t == "added").count();
    assert_eq!(added_count, 1, "'added' must appear exactly once after dedup");

    server.cleanup().await;
}

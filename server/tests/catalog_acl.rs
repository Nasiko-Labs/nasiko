//! Integration tests for catalog read-endpoint ACL enforcement and skill-tag dedup.
//!
//! Covers:
//!   - GET /api/agents/{id}       — non-owner gets 403, superuser gets through
//!   - GET /api/agents/{id}/versions — non-owner gets 403
//!   - GET /api/search/agents     — non-owner only sees their own agents
//!   - GET /api/agents, /api/agents/by-skill, /api/search/agents — a public or
//!     user-granted agent (not owned by the caller) is discoverable via listing,
//!     not just fetchable directly by id (CAT-3 regression coverage)
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

async fn create_agent(server: &common::TestServer, uid: &str, body: Value) -> Value {
    let res = common::as_superuser(
        server.client.post(server.url("/api/agents")),
        uid,
        "admin",
    )
    .json(&body)
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 201, "create agent should succeed");
    res.json::<Value>().await.unwrap()
}

async fn get_agent(server: &common::TestServer, uid: &str, is_super: bool, id: &str) -> reqwest::Response {
    let rb = server.client.get(server.url(&format!("/api/agents/{id}")));
    if is_super {
        common::as_superuser(rb, uid, "u")
    } else {
        common::as_member(rb, uid, "u")
    }
    .send()
    .await
    .unwrap()
}

async fn list_versions(server: &common::TestServer, uid: &str, is_super: bool, agent_id: &str) -> reqwest::Response {
    let rb = server.client.get(server.url(&format!("/api/agents/{agent_id}/versions")));
    if is_super {
        common::as_superuser(rb, uid, "u")
    } else {
        common::as_member(rb, uid, "u")
    }
    .send()
    .await
    .unwrap()
}

async fn search(server: &common::TestServer, uid: &str, is_super: bool, q: &str) -> Vec<Value> {
    let rb = server.client.get(server.url(&format!("/api/search/agents?q={q}")));
    let res = if is_super {
        common::as_superuser(rb, uid, "u")
    } else {
        common::as_member(rb, uid, "u")
    }
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 200);
    // Agent search returns a {agents, total, max_score} envelope (Python parity).
    let body: Value = res.json().await.unwrap();
    body["agents"].as_array().cloned().unwrap_or_default()
}

async fn list_agents(server: &common::TestServer, uid: &str, is_super: bool) -> Vec<Value> {
    let rb = server.client.get(server.url("/api/agents"));
    let res = if is_super {
        common::as_superuser(rb, uid, "u")
    } else {
        common::as_member(rb, uid, "u")
    }
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 200);
    res.json::<Vec<Value>>().await.unwrap()
}

async fn by_skill(server: &common::TestServer, uid: &str, is_super: bool, tag: &str) -> Vec<Value> {
    let rb = server
        .client
        .get(server.url(&format!("/api/agents/by-skill?tag={tag}")));
    let res = if is_super {
        common::as_superuser(rb, uid, "u")
    } else {
        common::as_member(rb, uid, "u")
    }
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 200);
    res.json::<Vec<Value>>().await.unwrap()
}

/// Insert a direct user-grant row so `grantee_id` can access `agent_id` (CAT-3 tests).
async fn grant_agent_to_user(server: &common::TestServer, agent_id: &str, grantee_id: &str) {
    sqlx::query(
        "INSERT INTO agent_grants (agent_id, grant_type, grantee_id) VALUES ($1, 'user', $2)",
    )
    .bind(uuid::Uuid::parse_str(agent_id).unwrap())
    .bind(grantee_id)
    .execute(&server.db)
    .await
    .unwrap();
}

async fn update_agent(server: &common::TestServer, uid: &str, agent_id: &str, body: Value) -> Value {
    let res = common::as_superuser(
        server.client.put(server.url(&format!("/api/agents/{agent_id}"))),
        uid,
        "admin",
    )
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

    let other_agent = common::as_member(
        server.client.post(server.url("/api/agents")),
        other_id,
        "srch-other",
    )
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

// ─── CAT-3: listing endpoints must surface public/granted agents ────────────
// `get_one` already allows a non-owner to fetch a public or user-granted agent
// directly by id (see `public_agent_non_owner_can_read_but_not_mutate` above).
// `list`, `by_skill`, and `search` must apply the same owner ∪ public ∪
// user-grant predicate, not a bare owner_id scope — otherwise such an agent is
// fetchable by id but never discoverable by browsing/searching.

#[tokio::test]
#[serial]
async fn list_includes_public_agent_for_non_owner() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    let pub_agent = create_agent(&server, uid, json!({"name": "cat3-list-pub", "version": "1.0.0"})).await;
    let pub_id = pub_agent["id"].as_str().unwrap();
    sqlx::query("UPDATE agents SET is_public = true WHERE id = $1")
        .bind(uuid::Uuid::parse_str(pub_id).unwrap())
        .execute(&server.db)
        .await
        .unwrap();

    let priv_agent = create_agent(&server, uid, json!({"name": "cat3-list-priv", "version": "1.0.0"})).await;
    let priv_id = priv_agent["id"].as_str().unwrap();

    let bob = create_user(&server, uid, "cat3-list-bob").await;
    let bob_id = bob["id"].as_str().unwrap();

    let seen = list_agents(&server, bob_id, false).await;
    let ids: Vec<&str> = seen.iter().filter_map(|a| a["id"].as_str()).collect();

    assert!(ids.contains(&pub_id), "non-owner must see a public agent in the list");
    assert!(!ids.contains(&priv_id), "non-owner must not see a private, non-granted agent in the list");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn by_skill_includes_user_granted_agent_for_non_owner() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    let granted = create_agent(&server, uid, json!({
        "name": "cat3-skill-granted",
        "version": "1.0.0",
        "skills": [skill("cat3-s1", &["cat3-skill-tag"])],
    }))
    .await;
    let granted_id = granted["id"].as_str().unwrap();

    let ungranted = create_agent(&server, uid, json!({
        "name": "cat3-skill-ungranted",
        "version": "1.0.0",
        "skills": [skill("cat3-s2", &["cat3-skill-tag"])],
    }))
    .await;
    let ungranted_id = ungranted["id"].as_str().unwrap();

    let bob = create_user(&server, uid, "cat3-skill-bob").await;
    let bob_id = bob["id"].as_str().unwrap();
    grant_agent_to_user(&server, granted_id, bob_id).await;

    let seen = by_skill(&server, bob_id, false, "cat3-skill-tag").await;
    let ids: Vec<&str> = seen.iter().filter_map(|a| a["id"].as_str()).collect();

    assert!(ids.contains(&granted_id), "non-owner must see a user-granted agent via by-skill");
    assert!(!ids.contains(&ungranted_id), "non-owner must not see a non-granted agent via by-skill");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn search_includes_public_agent_for_non_owner() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    let pub_agent = create_agent(&server, uid, json!({"name": "cat3-search-pub", "version": "1.0.0"})).await;
    let pub_id = pub_agent["id"].as_str().unwrap();
    sqlx::query("UPDATE agents SET is_public = true WHERE id = $1")
        .bind(uuid::Uuid::parse_str(pub_id).unwrap())
        .execute(&server.db)
        .await
        .unwrap();

    create_agent(&server, uid, json!({"name": "cat3-search-priv", "version": "1.0.0"})).await;

    let bob = create_user(&server, uid, "cat3-search-bob").await;
    let bob_id = bob["id"].as_str().unwrap();

    let results = search(&server, bob_id, false, "cat3-search").await;
    let names: Vec<&str> = results.iter().filter_map(|a| a["name"].as_str()).collect();

    assert!(names.contains(&"cat3-search-pub"), "non-owner must see a public agent via search");
    assert!(!names.contains(&"cat3-search-priv"), "non-owner must not see a private, non-granted agent via search");

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

// ─── read vs manage split (R3 correction / RUN-9) ────────────────────────────
// A public (or invoke-granted) agent is READABLE by a non-owner, but must NOT be
// mutable/destroyable by them — mutations are owner-or-superuser only.
#[tokio::test]
#[serial]
async fn public_agent_non_owner_can_read_but_not_mutate() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let owner_id = admin["user_id"].as_str().unwrap();

    let agent = create_agent(&server, owner_id, json!({"name": "pub-split-agent", "version": "1.0.0"})).await;
    let agent_id = agent["id"].as_str().unwrap();

    // Make it public so view-access (can_access_agent) is true for anyone.
    sqlx::query("UPDATE agents SET is_public = true WHERE id = $1")
        .bind(uuid::Uuid::parse_str(agent_id).unwrap())
        .execute(&server.db)
        .await
        .unwrap();

    let bob = create_user(&server, owner_id, "bobpub").await;
    let bob_id = bob["id"].as_str().unwrap();

    // READ: allowed for a non-owner because the agent is public.
    let read = get_agent(&server, bob_id, false, agent_id).await;
    assert_eq!(read.status(), 200, "non-owner should be able to read a public agent");

    // DELETE: forbidden — destroy is owner-or-superuser (RUN-9 IDOR guard).
    let del = common::as_member(
        server.client.delete(server.url(&format!("/api/agents/{agent_id}"))),
        bob_id,
        "bobpub",
    )
        .send()
        .await
        .unwrap();
    assert_eq!(del.status(), 403, "non-owner must NOT delete a public agent");

    // UPDATE: likewise forbidden — an invoke/public grant is not a manage grant.
    let upd = common::as_member(
        server.client.put(server.url(&format!("/api/agents/{agent_id}"))),
        bob_id,
        "bobpub",
    )
        .json(&json!({"description": "hijacked"}))
        .send()
        .await
        .unwrap();
    assert_eq!(upd.status(), 403, "non-owner must NOT update a public agent");

    server.cleanup().await;
}

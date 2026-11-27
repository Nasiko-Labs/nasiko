//! Harness agents are never shared. The OSS share routes refuse to make one public, grant it to a
//! user or hand it to another owner (409 `agent_not_shareable`); a user's own may be made private,
//! the platform's may not. A grant row on a user's harness agent written straight to the database
//! reaches nobody: not the access check, the catalog listing, the user registry, the
//! accessible-agents view or the A2A registry. The platform's harness catalog rows stay visible to
//! everyone, and the seed keeps them public.
//!
//! Requires infra (Postgres :5432, Redis, S3); skips when Postgres is unreachable:
//!   cargo test -p nasiko-server --test harness_sharing -- --test-threads=1

mod common;

use reqwest::StatusCode;
use serde_json::{Value, json};
use serial_test::serial;
use uuid::Uuid;

async fn insert_user(server: &common::TestServer, name: &str) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO users (username, email, is_superuser) VALUES ($1, $2, false) RETURNING id",
    )
    .bind(name)
    .bind(format!("{name}@harness-sharing.local"))
    .fetch_one(&server.db)
    .await
    .unwrap()
}

/// A user's own harness agent: not public and named `harness-<id>`, as the per-user rows are
/// created.
async fn insert_users_harness_agent(server: &common::TestServer, owner: Uuid) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO agents (name, owner_id, image, status, is_public, workload_kind, harness_id) \
         VALUES ('harness-opencode', $1, 'nasiko/harness-opencode:test', 'parked', false, \
                 'harness', 'opencode') RETURNING id",
    )
    .bind(owner)
    .fetch_one(&server.db)
    .await
    .unwrap()
}

async fn insert_framework_agent(server: &common::TestServer, owner: Uuid) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO agents (name, owner_id, image, status) \
         VALUES ('plain-agent', $1, 'img:1', 'running') RETURNING id",
    )
    .bind(owner)
    .fetch_one(&server.db)
    .await
    .unwrap()
}

/// The platform's harness catalog rows, seeded under the first superuser: that superuser's id and
/// the rows' ids.
async fn seed_platform_harnesses(server: &common::TestServer) -> (Uuid, Vec<Uuid>) {
    let admin: Value = server
        .client
        .post(server.url("/api/auth/initialize-admin"))
        .json(&json!({"username": "admin", "email": "admin@harness-sharing.local"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let admin = Uuid::parse_str(admin["user_id"].as_str().unwrap()).unwrap();
    nasiko_server::seed::seed_harness_agents(&server.state).await;
    let platform =
        sqlx::query_scalar("SELECT id FROM agents WHERE workload_kind = 'harness' AND is_public")
            .fetch_all(&server.db)
            .await
            .unwrap();
    (admin, platform)
}

fn ids_at(body: &Value, pointer: &str, key: &str) -> Vec<String> {
    body.pointer(pointer)
        .and_then(Value::as_array)
        .unwrap()
        .iter()
        .filter_map(|a| a[key].as_str().map(str::to_owned))
        .collect()
}

/// Ids `GET /api/registry/user/agents` gives `who`.
async fn registry_listed(server: &common::TestServer, who: Uuid) -> Vec<String> {
    let body: Value = as_member(
        server.client.get(server.url("/api/registry/user/agents")),
        who,
    )
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    ids_at(&body, "/data", "agent_id")
}

/// Ids `GET /api/users/{who}/accessible-agents` gives, asked by the superuser `admin` (the route is
/// for user managers).
async fn accessible(server: &common::TestServer, admin: Uuid, who: Uuid) -> Vec<String> {
    let body: Value = common::as_superuser(
        server
            .client
            .get(server.url(&format!("/api/users/{who}/accessible-agents"))),
        &admin.to_string(),
        "admin",
    )
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    ids_at(&body, "/data", "id")
}

async fn grant_directly(server: &common::TestServer, agent: Uuid, kind: &str, grantee: &str) {
    sqlx::query(
        "INSERT INTO agent_grants (agent_id, grant_type, grantee_id) VALUES ($1, $2::grant_type, $3)",
    )
    .bind(agent)
    .bind(kind)
    .bind(grantee)
    .execute(&server.db)
    .await
    .unwrap();
}

fn as_member(rb: reqwest::RequestBuilder, who: Uuid) -> reqwest::RequestBuilder {
    common::as_member(rb, &who.to_string(), "u")
}

async fn assert_refused(response: reqwest::Response) {
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["code"], "agent_not_shareable");
    assert!(body["error"].as_str().unwrap().contains("cannot be shared"));
}

/// Ids in the catalog listing `who` gets.
async fn listed(server: &common::TestServer, who: Uuid) -> Vec<String> {
    let response = as_member(server.client.get(server.url("/api/agents")), who)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let agents: Vec<Value> = response.json().await.unwrap();
    agents
        .iter()
        .filter_map(|a| a["id"].as_str().map(str::to_owned))
        .collect()
}

/// Ids the A2A registry gives `who`'s session.
async fn discovered(server: &common::TestServer, who: Uuid) -> Vec<String> {
    let token = common::sign_token(&who.to_string(), "u", false, "member");
    let body: Value = server
        .client
        .post(server.url("/a2a/v1"))
        .bearer_auth(token)
        .json(&json!({"jsonrpc": "2.0", "id": 1, "method": "message/send", "params": {}}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    body["result"]["artifacts"][0]["parts"][0]["data"]["agents"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|a| a["id"].as_str().map(str::to_owned))
        .collect()
}

#[tokio::test]
#[serial]
async fn the_share_routes_refuse_a_harness_agent_and_change_nothing() {
    let server = common::TestServer::start().await;
    let alice = insert_user(&server, "harness-alice").await;
    let bob = insert_user(&server, "harness-bob").await;
    let harness = insert_users_harness_agent(&server, alice).await;
    let plain = insert_framework_agent(&server, alice).await;

    let public = as_member(
        server
            .client
            .post(server.url(&format!("/api/agents/{harness}/grants/public"))),
        alice,
    )
    .send()
    .await
    .unwrap();
    assert_refused(public).await;
    let to_bob = as_member(
        server
            .client
            .post(server.url(&format!("/api/agents/{harness}/grants/users"))),
        alice,
    )
    .json(&json!({ "user_id": bob }))
    .send()
    .await
    .unwrap();
    assert_refused(to_bob).await;
    let handed_to_bob = as_member(
        server
            .client
            .put(server.url(&format!("/api/agents/{harness}/owner"))),
        alice,
    )
    .json(&json!({ "new_owner_id": bob }))
    .send()
    .await
    .unwrap();
    assert_refused(handed_to_bob).await;
    // Making a user's own harness agent private is allowed: it never should be public.
    let private = as_member(
        server
            .client
            .delete(server.url(&format!("/api/agents/{harness}/grants/public"))),
        alice,
    )
    .send()
    .await
    .unwrap();
    assert_eq!(private.status(), StatusCode::NO_CONTENT);

    let (is_public, owner, grants): (bool, Uuid, i64) = sqlx::query_as(
        "SELECT is_public, owner_id, (SELECT count(*) FROM agent_grants WHERE agent_id = $1) \
         FROM agents WHERE id = $1",
    )
    .bind(harness)
    .fetch_one(&server.db)
    .await
    .unwrap();
    assert!(!is_public);
    assert_eq!(owner, alice);
    assert_eq!(grants, 0);

    // A framework agent is shared as before.
    let plain_public = as_member(
        server
            .client
            .post(server.url(&format!("/api/agents/{plain}/grants/public"))),
        alice,
    )
    .send()
    .await
    .unwrap();
    assert_eq!(plain_public.status(), StatusCode::NO_CONTENT);
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_grant_on_a_users_harness_agent_reaches_nobody_else() {
    let server = common::TestServer::start().await;
    let (admin, platform) = seed_platform_harnesses(&server).await;
    assert!(
        !platform.is_empty(),
        "the platform harness rows are seeded public"
    );
    let alice = insert_user(&server, "harness-alice").await;
    let bob = insert_user(&server, "harness-bob").await;
    let harness = insert_users_harness_agent(&server, alice).await;
    grant_directly(&server, harness, "user", &bob.to_string()).await;
    grant_directly(&server, harness, "public", "*").await;

    let opened = as_member(
        server
            .client
            .get(server.url(&format!("/api/agents/{harness}"))),
        bob,
    )
    .send()
    .await
    .unwrap();
    assert_eq!(opened.status(), StatusCode::FORBIDDEN);

    let harness = harness.to_string();
    let bob_sees = [
        listed(&server, bob).await,
        discovered(&server, bob).await,
        registry_listed(&server, bob).await,
        accessible(&server, admin, bob).await,
    ];
    for (view, ids) in ["catalog", "a2a registry", "user registry", "accessible"]
        .iter()
        .zip(&bob_sees)
    {
        assert!(
            !ids.contains(&harness),
            "{view} shows bob alice's harness agent"
        );
        for id in &platform {
            assert!(
                ids.contains(&id.to_string()),
                "{view} still shows the platform harness rows"
            );
        }
    }

    // Its owner still sees it.
    assert!(listed(&server, alice).await.contains(&harness));
    assert!(discovered(&server, alice).await.contains(&harness));
    assert!(registry_listed(&server, alice).await.contains(&harness));
    assert!(accessible(&server, admin, alice).await.contains(&harness));
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_platform_harness_agent_may_not_be_made_private() {
    let server = common::TestServer::start().await;
    let (admin, platform) = seed_platform_harnesses(&server).await;
    let opencode = *platform
        .first()
        .expect("the platform harness rows are seeded");

    let private = common::as_superuser(
        server
            .client
            .delete(server.url(&format!("/api/agents/{opencode}/grants/public"))),
        &admin.to_string(),
        "admin",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(private.status(), StatusCode::CONFLICT);
    let body: Value = private.json().await.unwrap();
    assert_eq!(body["code"], "agent_not_shareable");

    let is_public: bool = sqlx::query_scalar("SELECT is_public FROM agents WHERE id = $1")
        .bind(opencode)
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert!(is_public);
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn the_seed_keeps_the_platform_harness_rows_public() {
    let server = common::TestServer::start().await;
    let (_, platform) = seed_platform_harnesses(&server).await;
    sqlx::query("UPDATE agents SET is_public = false WHERE id = ANY($1)")
        .bind(&platform)
        .execute(&server.db)
        .await
        .unwrap();

    nasiko_server::seed::seed_harness_agents(&server.state).await;

    let private: i64 =
        sqlx::query_scalar("SELECT count(*) FROM agents WHERE id = ANY($1) AND NOT is_public")
            .bind(&platform)
            .fetch_one(&server.db)
            .await
            .unwrap();
    assert_eq!(private, 0);
    server.cleanup().await;
}

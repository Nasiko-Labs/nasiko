//! `GET/PUT/DELETE /api/agents/{id}/my-llm-config`: a user's own config for an agent someone else owns,
//! through the production router. Access is the agent's (404 otherwise), an owner is refused (400), and only
//! the caller's own live config can be chosen.
//!
//! Requires infra (Postgres, Redis, S3):
//!   cargo test -p nasiko-server --test agent_my_llm_config -- --test-threads=1

mod common;

use reqwest::{Method, StatusCode};
use serde_json::{Value, json};
use serial_test::serial;
use uuid::Uuid;

const OWNER_REFUSAL: &str =
    "you own this agent: set its model with PATCH /api/agents/{id}/llm-config";

type Choice = (Uuid, Uuid, Uuid);

async fn seed_user(server: &common::TestServer, name: &str) -> Uuid {
    sqlx::query_scalar("INSERT INTO users (username, email) VALUES ($1, $2) RETURNING id")
        .bind(name)
        .bind(format!("{name}@my-llm-config.local"))
        .fetch_one(&server.db)
        .await
        .unwrap()
}

async fn seed_agent(server: &common::TestServer, owner: Uuid, name: &str, is_public: bool) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO agents (name, owner_id, image, status, is_public) \
         VALUES ($1, $2, 'img:1', 'running', $3) RETURNING id",
    )
    .bind(name)
    .bind(owner)
    .bind(is_public)
    .fetch_one(&server.db)
    .await
    .unwrap()
}

async fn seed_config(server: &common::TestServer, owner: Uuid, name: &str) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO llm_configs (created_by, name, provider, model) \
         VALUES ($1, $2, 'openai', 'gpt-4o-mini') RETURNING id",
    )
    .bind(owner)
    .bind(name)
    .fetch_one(&server.db)
    .await
    .unwrap()
}

/// A row written straight to the table, as an earlier state of the world could have left it.
async fn seed_choice(server: &common::TestServer, (user, agent, config): Choice) {
    sqlx::query(
        "INSERT INTO agent_user_llm_configs (user_id, agent_id, llm_config_id) VALUES ($1, $2, $3)",
    )
    .bind(user)
    .bind(agent)
    .bind(config)
    .execute(&server.db)
    .await
    .unwrap();
}

async fn choices(server: &common::TestServer) -> Vec<Choice> {
    sqlx::query_as(
        "SELECT user_id, agent_id, llm_config_id FROM agent_user_llm_configs ORDER BY user_id, agent_id",
    )
    .fetch_all(&server.db)
    .await
    .unwrap()
}

async fn updated_at(
    server: &common::TestServer,
    user: Uuid,
    agent: Uuid,
) -> chrono::DateTime<chrono::Utc> {
    sqlx::query_scalar(
        "SELECT updated_at FROM agent_user_llm_configs WHERE user_id = $1 AND agent_id = $2",
    )
    .bind(user)
    .bind(agent)
    .fetch_one(&server.db)
    .await
    .unwrap()
}

async fn call(
    server: &common::TestServer,
    method: Method,
    who: Uuid,
    agent: Uuid,
    config: Option<Uuid>,
) -> (StatusCode, String) {
    let url = server.url(&format!("/api/agents/{agent}/my-llm-config"));
    let mut request = common::as_member(server.client.request(method, url), &who.to_string(), "u");
    if let Some(config) = config {
        request = request.json(&json!({ "llm_config_id": config }));
    }
    let response = request.send().await.unwrap();
    (response.status(), response.text().await.unwrap())
}

async fn get(server: &common::TestServer, who: Uuid, agent: Uuid) -> (StatusCode, String) {
    call(server, Method::GET, who, agent, None).await
}

async fn put(
    server: &common::TestServer,
    who: Uuid,
    agent: Uuid,
    config: Uuid,
) -> (StatusCode, String) {
    call(server, Method::PUT, who, agent, Some(config)).await
}

async fn delete(server: &common::TestServer, who: Uuid, agent: Uuid) -> (StatusCode, String) {
    call(server, Method::DELETE, who, agent, None).await
}

/// The envelope's `data` of a 200 response.
fn data((status, body): &(StatusCode, String)) -> Value {
    assert_eq!(*status, StatusCode::OK, "body: {body}");
    serde_json::from_str::<Value>(body).unwrap()["data"].clone()
}

fn assert_refused((status, body): (StatusCode, String), want: StatusCode, text: &str) {
    assert_eq!(status, want, "body: {body}");
    assert_eq!(body, text);
}

fn unset(agent: Uuid) -> Value {
    json!({ "agent_id": agent, "llm_config_id": null, "llm_config": null })
}

#[tokio::test]
#[serial]
async fn put_then_get_returns_the_choice_and_a_second_put_replaces_it() {
    let server = common::TestServer::start().await;
    let owner = seed_user(&server, "mlc-owner").await;
    let bob = seed_user(&server, "mlc-bob").await;
    let agent = seed_agent(&server, owner, "shared", true).await;
    let (first, second) = (
        seed_config(&server, bob, "first").await,
        seed_config(&server, bob, "second").await,
    );

    assert_eq!(data(&get(&server, bob, agent).await), unset(agent));
    assert!(choices(&server).await.is_empty());

    let put_first = data(&put(&server, bob, agent, first).await);
    assert_eq!(put_first["llm_config_id"], json!(first));
    assert_eq!(put_first["llm_config"]["id"], json!(first));
    assert_eq!(put_first["llm_config"]["name"], "first");
    assert_eq!(put_first["llm_config"]["provider"], "openai");
    assert_eq!(data(&get(&server, bob, agent).await), put_first);
    assert_eq!(choices(&server).await, vec![(bob, agent, first)]);
    let first_at = updated_at(&server, bob, agent).await;

    let put_second = data(&put(&server, bob, agent, second).await);
    assert_eq!(put_second["llm_config"]["name"], "second");
    assert_eq!(data(&get(&server, bob, agent).await), put_second);
    assert_eq!(choices(&server).await, vec![(bob, agent, second)]);
    assert!(updated_at(&server, bob, agent).await > first_at);

    // The same choice again is a plain success, still one row.
    data(&put(&server, bob, agent, second).await);
    assert_eq!(choices(&server).await, vec![(bob, agent, second)]);
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn the_three_methods_are_in_the_openapi_spec() {
    let server = common::TestServer::start().await;
    let spec: Value = server
        .client
        .get(server.url("/api/openapi.json"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let path = &spec["paths"]["/api/agents/{id}/my-llm-config"];
    for method in ["get", "put", "delete"] {
        assert!(path[method].is_object(), "{method} missing from {path}");
    }
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn another_users_config_is_404_and_writes_nothing() {
    let server = common::TestServer::start().await;
    let owner = seed_user(&server, "mlc-owner").await;
    let bob = seed_user(&server, "mlc-bob").await;
    let agent = seed_agent(&server, owner, "shared", true).await;
    let mine = seed_config(&server, bob, "mine").await;
    let owners = seed_config(&server, owner, "owners").await;
    data(&put(&server, bob, agent, mine).await);
    let before = choices(&server).await;
    assert_eq!(before, vec![(bob, agent, mine)]);

    // Live, but the owner's: its key would not decrypt for bob.
    assert_refused(
        put(&server, bob, agent, owners).await,
        StatusCode::NOT_FOUND,
        "llm config not found",
    );
    assert_refused(
        put(&server, bob, agent, Uuid::new_v4()).await,
        StatusCode::NOT_FOUND,
        "llm config not found",
    );
    assert_eq!(choices(&server).await, before);
    assert_eq!(
        data(&get(&server, bob, agent).await)["llm_config_id"],
        json!(mine)
    );
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_soft_deleted_config_cannot_be_chosen() {
    let server = common::TestServer::start().await;
    let owner = seed_user(&server, "mlc-owner").await;
    let bob = seed_user(&server, "mlc-bob").await;
    let agent = seed_agent(&server, owner, "shared", true).await;
    let gone = seed_config(&server, bob, "gone").await;
    sqlx::query("UPDATE llm_configs SET deleted_at = now() WHERE id = $1")
        .bind(gone)
        .execute(&server.db)
        .await
        .unwrap();

    assert_refused(
        put(&server, bob, agent, gone).await,
        StatusCode::NOT_FOUND,
        "llm config not found",
    );
    assert!(choices(&server).await.is_empty());
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_soft_deleted_choice_reads_as_unset() {
    let server = common::TestServer::start().await;
    let owner = seed_user(&server, "mlc-owner").await;
    let bob = seed_user(&server, "mlc-bob").await;
    let agent = seed_agent(&server, owner, "shared", true).await;
    let chosen = seed_config(&server, bob, "chosen").await;
    data(&put(&server, bob, agent, chosen).await);
    assert_eq!(
        data(&get(&server, bob, agent).await)["llm_config_id"],
        json!(chosen)
    );

    sqlx::query("UPDATE llm_configs SET deleted_at = now() WHERE id = $1")
        .bind(chosen)
        .execute(&server.db)
        .await
        .unwrap();

    // The row is still there; the read ignores it.
    assert_eq!(choices(&server).await, vec![(bob, agent, chosen)]);
    assert_eq!(data(&get(&server, bob, agent).await), unset(agent));
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_choice_pointing_at_someone_elses_config_reads_as_unset() {
    let server = common::TestServer::start().await;
    let owner = seed_user(&server, "mlc-owner").await;
    let bob = seed_user(&server, "mlc-bob").await;
    let agent = seed_agent(&server, owner, "shared", true).await;
    let owners = seed_config(&server, owner, "owners").await;
    seed_choice(&server, (bob, agent, owners)).await;

    assert_eq!(data(&get(&server, bob, agent).await), unset(agent));
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn no_access_is_404_on_all_three_and_a_deleted_agent_too() {
    let server = common::TestServer::start().await;
    let owner = seed_user(&server, "mlc-owner").await;
    let bob = seed_user(&server, "mlc-bob").await;
    let root = seed_user(&server, "mlc-root").await;
    let private = seed_agent(&server, owner, "private", false).await;
    let deleted = seed_agent(&server, owner, "deleted", true).await;
    sqlx::query("UPDATE agents SET deleted_at = now() WHERE id = $1")
        .bind(deleted)
        .execute(&server.db)
        .await
        .unwrap();
    let (mine, other) = (
        seed_config(&server, bob, "mine").await,
        seed_config(&server, bob, "other").await,
    );
    // A leftover from when bob could use the agent: refused calls must neither read, replace nor remove it.
    seed_choice(&server, (bob, private, mine)).await;
    let before = choices(&server).await;

    for agent in [private, deleted, Uuid::new_v4()] {
        assert_refused(
            get(&server, bob, agent).await,
            StatusCode::NOT_FOUND,
            "agent not found",
        );
        assert_refused(
            put(&server, bob, agent, other).await,
            StatusCode::NOT_FOUND,
            "agent not found",
        );
        assert_refused(
            delete(&server, bob, agent).await,
            StatusCode::NOT_FOUND,
            "agent not found",
        );
    }
    // A superuser passes the access check for any id, so a deleted agent must be refused on its own.
    let root_config = seed_config(&server, root, "root").await;
    let as_root = |method: Method, config: Option<Uuid>| {
        let url = server.url(&format!("/api/agents/{deleted}/my-llm-config"));
        let request = common::as_superuser(
            server.client.request(method, url),
            &root.to_string(),
            "root",
        );
        match config {
            Some(config) => request.json(&json!({ "llm_config_id": config })),
            None => request,
        }
    };
    for request in [
        as_root(Method::GET, None),
        as_root(Method::PUT, Some(root_config)),
        as_root(Method::DELETE, None),
    ] {
        let response = request.send().await.unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_eq!(response.text().await.unwrap(), "agent not found");
    }
    assert_eq!(choices(&server).await, before);

    // Positive control: the same calls succeed once bob is granted the agent.
    sqlx::query("INSERT INTO agent_grants (agent_id, grant_type, grantee_id) VALUES ($1, 'user'::grant_type, $2)")
        .bind(private)
        .bind(bob.to_string())
        .execute(&server.db)
        .await
        .unwrap();
    assert_eq!(
        data(&put(&server, bob, private, other).await)["llm_config_id"],
        json!(other)
    );
    assert_eq!(choices(&server).await, vec![(bob, private, other)]);

    let anonymous = server
        .client
        .get(server.url(&format!("/api/agents/{private}/my-llm-config")))
        .send()
        .await
        .unwrap();
    assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED);
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn the_owner_is_refused_on_all_three_and_nothing_is_written() {
    let server = common::TestServer::start().await;
    let owner = seed_user(&server, "mlc-owner").await;
    let agent = seed_agent(&server, owner, "mine", true).await;
    let (first, second) = (
        seed_config(&server, owner, "first").await,
        seed_config(&server, owner, "second").await,
    );
    // A leftover from before the agent was theirs: refused calls must neither read, replace nor remove it.
    seed_choice(&server, (owner, agent, first)).await;
    let before = choices(&server).await;

    assert_refused(
        get(&server, owner, agent).await,
        StatusCode::BAD_REQUEST,
        OWNER_REFUSAL,
    );
    assert_refused(
        put(&server, owner, agent, second).await,
        StatusCode::BAD_REQUEST,
        OWNER_REFUSAL,
    );
    assert_refused(
        delete(&server, owner, agent).await,
        StatusCode::BAD_REQUEST,
        OWNER_REFUSAL,
    );
    assert_eq!(choices(&server).await, before);
    assert_eq!(before, vec![(owner, agent, first)]);
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn delete_removes_only_the_callers_choice_and_deleting_again_is_ok() {
    let server = common::TestServer::start().await;
    let owner = seed_user(&server, "mlc-owner").await;
    let (bob, dave) = (
        seed_user(&server, "mlc-bob").await,
        seed_user(&server, "mlc-dave").await,
    );
    let (agent, elsewhere) = (
        seed_agent(&server, owner, "shared", true).await,
        seed_agent(&server, owner, "other", true).await,
    );
    let (bobs, daves) = (
        seed_config(&server, bob, "bobs").await,
        seed_config(&server, dave, "daves").await,
    );
    data(&put(&server, bob, agent, bobs).await);
    data(&put(&server, bob, elsewhere, bobs).await);
    data(&put(&server, dave, agent, daves).await);

    assert_eq!(data(&delete(&server, bob, agent).await), Value::Null);
    assert_eq!(data(&get(&server, bob, agent).await), unset(agent));
    let mut want = vec![(bob, elsewhere, bobs), (dave, agent, daves)];
    want.sort();
    assert_eq!(choices(&server).await, want);

    assert_eq!(data(&delete(&server, bob, agent).await), Value::Null);
    assert_eq!(choices(&server).await, want);
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn concurrent_puts_converge_on_one_row() {
    let server = common::TestServer::start().await;
    let owner = seed_user(&server, "mlc-owner").await;
    let bob = seed_user(&server, "mlc-bob").await;
    let agent = seed_agent(&server, owner, "shared", true).await;
    let mut configs = Vec::new();
    for n in 0..8 {
        configs.push(seed_config(&server, bob, &format!("config-{n}")).await);
    }

    let results = futures::future::join_all(
        configs
            .iter()
            .map(|config| put(&server, bob, agent, *config)),
    )
    .await;

    for result in &results {
        data(result);
    }
    let rows = choices(&server).await;
    assert_eq!(rows.len(), 1);
    assert!(configs.contains(&rows[0].2));
    assert_eq!(
        data(&get(&server, bob, agent).await)["llm_config_id"],
        json!(rows[0].2)
    );
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_database_failure_is_a_500_not_an_unset_answer() {
    let server = common::TestServer::start().await;
    let owner = seed_user(&server, "mlc-owner").await;
    let bob = seed_user(&server, "mlc-bob").await;
    let agent = seed_agent(&server, owner, "shared", true).await;
    let mine = seed_config(&server, bob, "mine").await;
    sqlx::query("DROP TABLE agent_user_llm_configs")
        .execute(&server.db)
        .await
        .unwrap();

    assert_refused(
        get(&server, bob, agent).await,
        StatusCode::INTERNAL_SERVER_ERROR,
        "failed to read my llm config",
    );
    assert_refused(
        put(&server, bob, agent, mine).await,
        StatusCode::INTERNAL_SERVER_ERROR,
        "failed to set my llm config",
    );
    assert_refused(
        delete(&server, bob, agent).await,
        StatusCode::INTERNAL_SERVER_ERROR,
        "failed to clear my llm config",
    );

    // The access lookup fails before anything is read or written.
    sqlx::query("ALTER TABLE agents RENAME TO agents_moved")
        .execute(&server.db)
        .await
        .unwrap();
    assert_refused(
        get(&server, bob, agent).await,
        StatusCode::INTERNAL_SERVER_ERROR,
        "failed to check access for llm config",
    );
    server.cleanup().await;
}

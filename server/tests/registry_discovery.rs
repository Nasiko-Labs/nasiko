//! NAS-1129: `POST /a2a/v1` answered anyone, with every running agent of
//! every owner and its runtime-internal address. It now lists public agents to
//! a caller with no credentials, and the agents a caller may call to a caller
//! with a session; every address is the agent proxy path.
//!
//!   cargo test -p nasiko-server --test registry_discovery -- --test-threads=1
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
    .bind(format!("{name}@discovery-test.local"))
    .fetch_one(&server.db)
    .await
    .unwrap()
}

/// A running agent whose stored address is an internal one, as a real deploy
/// leaves it.
async fn insert_agent(
    server: &common::TestServer,
    name: &str,
    owner: Uuid,
    is_public: bool,
    internal_address: &str,
) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO agents (name, owner_id, version, image, status, url, is_public) \
         VALUES ($1, $2, '1.0.0', 'img:1', 'running', $3, $4) RETURNING id",
    )
    .bind(name)
    .bind(owner)
    .bind(internal_address)
    .bind(is_public)
    .fetch_one(&server.db)
    .await
    .unwrap()
}

/// `(status, raw body)` for a discovery call, with an optional bearer token.
async fn discover(server: &common::TestServer, token: Option<&str>) -> (StatusCode, String) {
    let mut request = server.client.post(server.url("/a2a/v1")).json(&json!({
        "jsonrpc": "2.0", "id": 1, "method": "message/send", "params": {},
    }));
    if let Some(token) = token {
        request = request.bearer_auth(token);
    }
    let response = request.send().await.unwrap();
    (response.status(), response.text().await.unwrap())
}

fn names(body: &str) -> Vec<String> {
    let value: Value = serde_json::from_str(body).unwrap();
    value["result"]["artifacts"][0]["parts"][0]["data"]["agents"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["name"].as_str().unwrap().to_owned())
        .collect()
}

#[tokio::test]
#[serial]
async fn discovery_lists_what_each_caller_may_call_and_never_an_internal_address() {
    let server = common::TestServer::start().await;
    let alice = insert_user(&server, "dummy-alice").await;
    let bob = insert_user(&server, "dummy-bob").await;
    let private = insert_agent(
        &server,
        "dummy-private",
        alice,
        false,
        "http://10.0.9.9:8000",
    )
    .await;
    let public = insert_agent(&server, "dummy-public", bob, true, "http://10.0.9.10:8000").await;

    // No credentials: the public catalog only.
    let (status, body) = discover(&server, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(names(&body), ["dummy-public"]);
    assert!(
        !body.contains("10.0.9"),
        "an internal address leaked: {body}"
    );
    let entry: Value =
        serde_json::from_str::<Value>(&body).unwrap()["result"]["artifacts"][0]["parts"][0]["data"]
            ["agents"][0]
            .clone();
    assert_eq!(entry["url"], format!("/api/agents/{public}"));
    assert_eq!(entry["endpoint"], format!("/api/agents/{public}"));

    // The owner of the private agent sees it, plus the public one.
    let alice_token = common::sign_token(&alice.to_string(), "dummy-alice", false, "member");
    let (_, body) = discover(&server, Some(&alice_token)).await;
    assert_eq!(names(&body), ["dummy-private", "dummy-public"]);
    assert!(
        !body.contains("10.0.9"),
        "an internal address leaked: {body}"
    );
    assert!(body.contains(&format!("/api/agents/{private}")));

    // Another member does not.
    let bob_token = common::sign_token(&bob.to_string(), "dummy-bob", false, "member");
    let (_, body) = discover(&server, Some(&bob_token)).await;
    assert_eq!(names(&body), ["dummy-public"]);

    // A user grant is what the proxy honours, so discovery honours it too.
    sqlx::query(
        "INSERT INTO agent_grants (agent_id, grant_type, grantee_id) VALUES ($1, 'user', $2)",
    )
    .bind(private)
    .bind(bob.to_string())
    .execute(&server.db)
    .await
    .unwrap();
    let (_, body) = discover(&server, Some(&bob_token)).await;
    assert_eq!(names(&body), ["dummy-private", "dummy-public"]);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_superuser_sees_every_running_agent() {
    let server = common::TestServer::start().await;
    let alice = insert_user(&server, "dummy-alice").await;
    let admin = insert_user(&server, "dummy-admin").await;
    sqlx::query("UPDATE users SET is_superuser = true WHERE id = $1")
        .bind(admin)
        .execute(&server.db)
        .await
        .unwrap();
    insert_agent(
        &server,
        "dummy-private",
        alice,
        false,
        "http://10.0.9.9:8000",
    )
    .await;

    let token = common::sign_token(&admin.to_string(), "dummy-admin", true, "admin");
    let (status, body) = discover(&server, Some(&token)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(names(&body), ["dummy-private"]);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn an_invalid_token_is_refused_not_downgraded_to_the_public_view() {
    let server = common::TestServer::start().await;

    let (status, _) = discover(&server, Some("not-a-token")).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    server.cleanup().await;
}

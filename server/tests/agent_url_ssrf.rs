//! NAS-1113: an agent registered by URL is reached through the agent proxy
//! (and A2A dispatch, and MAF) at whatever `agents.url` says, and the reply is
//! relayed back. Any signed-in user could register one, so the proxy became a
//! way to reach addresses only the server can: cloud metadata, the database,
//! admin ports, other agents' containers.
//!
//! The address is now checked where a user sets it: an agent the platform
//! deploys takes its address from the deploy, and an agent registered by URL
//! must use a public address unless `AGENT_ALLOW_PRIVATE_URLS` is set.
//!
//!   cargo test -p nasiko-server --test agent_url_ssrf -- --test-threads=1
mod common;

use std::sync::{Arc, Mutex};

use axum::{Router, routing::any};
use reqwest::StatusCode;
use serde_json::{Value, json};
use serial_test::serial;
use uuid::Uuid;

/// A stand-in for an internal service: answers `probe-ok` and counts requests.
async fn internal_service() -> (String, Arc<Mutex<u32>>) {
    let hits: Arc<Mutex<u32>> = Arc::default();
    let count = hits.clone();
    let app = Router::new().fallback(any(move || {
        let count = count.clone();
        async move {
            *count.lock().unwrap() += 1;
            "probe-ok"
        }
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{addr}"), hits)
}

async fn guarded_server() -> common::TestServer {
    common::TestServer::start_with(|config| config.agent_allow_private_urls = false).await
}

async fn insert_member(server: &common::TestServer) -> String {
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO users (username, email, is_superuser) \
         VALUES ('dummy-member', 'dummy-member@test.local', false) RETURNING id",
    )
    .fetch_one(&server.db)
    .await
    .unwrap();
    id.to_string()
}

async fn create(server: &common::TestServer, member: &str, body: Value) -> reqwest::Response {
    common::as_member(
        server.client.post(server.url("/api/agents")),
        member,
        "dummy-member",
    )
    .json(&body)
    .send()
    .await
    .unwrap()
}

async fn update(
    server: &common::TestServer,
    member: &str,
    id: &str,
    body: Value,
) -> reqwest::Response {
    common::as_member(
        server.client.put(server.url(&format!("/api/agents/{id}"))),
        member,
        "dummy-member",
    )
    .json(&body)
    .send()
    .await
    .unwrap()
}

async fn stored_url(server: &common::TestServer, id: &str) -> Option<String> {
    sqlx::query_scalar("SELECT url FROM agents WHERE id = $1::uuid")
        .bind(id)
        .fetch_one(&server.db)
        .await
        .unwrap()
}

#[tokio::test]
#[serial]
async fn a_member_cannot_register_an_agent_at_a_private_address() {
    let server = guarded_server().await;
    let member = insert_member(&server).await;
    let (loopback, hits) = internal_service().await;

    for url in [
        loopback.as_str(),
        "http://localhost:8080/",
        "http://169.254.169.254/latest/meta-data/",
        "http://10.0.0.5:5432/",
        "file:///etc/passwd",
    ] {
        let created = create(
            &server,
            &member,
            json!({"name": "dummy-external", "url": url}),
        )
        .await;
        assert_eq!(created.status(), StatusCode::BAD_REQUEST, "{url}");
    }
    assert_eq!(*hits.lock().unwrap(), 0);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_member_cannot_move_an_agent_to_a_private_address_and_reach_it() {
    let server = guarded_server().await;
    let member = insert_member(&server).await;
    let (loopback, hits) = internal_service().await;

    // An IP literal: public, and resolvable without DNS.
    let created = create(
        &server,
        &member,
        json!({"name": "dummy-external", "url": "http://93.184.216.34/"}),
    )
    .await;
    assert_eq!(created.status(), StatusCode::CREATED);
    let id = created.json::<Value>().await.unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();

    let moved = update(
        &server,
        &member,
        &id,
        json!({"url": loopback, "status": "running"}),
    )
    .await;
    assert_eq!(moved.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        stored_url(&server, &id).await.as_deref(),
        Some("http://93.184.216.34/")
    );

    // Nothing the member can call now reaches the internal service.
    let running = update(&server, &member, &id, json!({"status": "running"})).await;
    assert!(running.status().is_success());
    let _ = common::as_member(
        server
            .client
            .get(server.url(&format!("/api/agents/{id}/.well-known/agent.json")))
            .timeout(std::time::Duration::from_secs(5)),
        &member,
        "dummy-member",
    )
    .send()
    .await;
    assert_eq!(*hits.lock().unwrap(), 0);

    server.cleanup().await;
}

/// `nasiko deploy` sends its card's `url` (`http://localhost:8000/` from the
/// scaffold) when it registers a new agent. The deploy sets the real address,
/// so the requested one is dropped rather than refused.
#[tokio::test]
#[serial]
async fn a_platform_deployed_agent_takes_no_address_from_the_caller() {
    let server = guarded_server().await;
    let member = insert_member(&server).await;
    let member_uuid = Uuid::parse_str(&member).unwrap();
    let image = format!("u-{}/dummy-deployed:1.0.0", member_uuid.as_simple());

    let created = create(
        &server,
        &member,
        json!({
            "name": "dummy-deployed",
            "url": "http://localhost:8000/",
            "image": image,
        }),
    )
    .await;
    assert_eq!(created.status(), StatusCode::CREATED);
    let id = created.json::<Value>().await.unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(stored_url(&server, &id).await, None);

    let moved = update(
        &server,
        &member,
        &id,
        json!({"url": "http://169.254.169.254/"}),
    )
    .await;
    assert!(moved.status().is_success());
    assert_eq!(stored_url(&server, &id).await, None);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn an_operator_can_allow_agents_on_private_addresses() {
    let server = common::TestServer::start().await; // the harness turns the opt-in on
    let member = insert_member(&server).await;
    let (loopback, hits) = internal_service().await;

    let created = create(
        &server,
        &member,
        json!({"name": "dummy-onprem", "url": loopback}),
    )
    .await;
    assert_eq!(created.status(), StatusCode::CREATED);
    let id = created.json::<Value>().await.unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(
        update(&server, &member, &id, json!({"status": "running"}))
            .await
            .status()
            .is_success()
    );
    let reply = common::as_member(
        server
            .client
            .get(server.url(&format!("/api/agents/{id}/.well-known/agent.json"))),
        &member,
        "dummy-member",
    )
    .send()
    .await
    .unwrap()
    .text()
    .await
    .unwrap();
    assert_eq!(reply, "probe-ok");
    assert_eq!(*hits.lock().unwrap(), 1);

    server.cleanup().await;
}

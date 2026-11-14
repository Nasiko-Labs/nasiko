//! `GET/PUT /api/settings` report the models the server runs, from its environment, and a
//! `router_model` sent by an older client changes nothing.
//!
//!   cargo test -p nasiko-server --test settings_models -- --test-threads=1

mod common;

use serde_json::{Value, json};
use serial_test::serial;

async fn init_admin(server: &common::TestServer) -> String {
    let body: Value = server
        .client
        .post(server.url("/api/auth/initialize-admin"))
        .json(&json!({"username": "admin", "email": "admin@test.local"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    body["user_id"].as_str().unwrap().to_string()
}

async fn get(server: &common::TestServer, admin: &str) -> Value {
    common::as_superuser(
        server.client.get(server.url("/api/settings")),
        admin,
        "admin",
    )
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap()
}

#[tokio::test]
#[serial]
async fn settings_report_the_models_from_the_environment() {
    let server = common::TestServer::start_with(|c| {
        c.openai_model = "orchestrator-env-model".into();
        c.router_model = "router-env-model".into();
    })
    .await;
    let admin = init_admin(&server).await;

    let before = get(&server, &admin).await;
    assert_eq!(before["orchestrator_model"], "orchestrator-env-model");
    assert_eq!(before["router_model"], "router-env-model");

    let res = common::as_superuser(
        server.client.put(server.url("/api/settings")),
        &admin,
        "admin",
    )
    .json(&json!({
        "router_model": "ignored-model",
        "default_provider": "openai",
        "max_flow_depth": 5,
        "max_flow_fan_out": 20,
        "max_flow_tokens": 100000,
        "flow_timeout_secs": 120,
    }))
    .send()
    .await
    .unwrap();
    assert_eq!(res.status().as_u16(), 200);
    // The page caches this answer, so it carries the models as GET does.
    let saved: Value = res.json().await.unwrap();
    assert_eq!(saved["orchestrator_model"], "orchestrator-env-model");
    assert_eq!(saved["router_model"], "router-env-model");

    assert_eq!(
        get(&server, &admin).await["router_model"],
        "router-env-model"
    );
}

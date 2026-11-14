//! `GET/PUT /api/settings` report the models, default provider and flow limits the server runs,
//! from its environment, and the same keys sent by an older client change nothing.
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

#[tokio::test]
#[serial]
async fn settings_report_the_provider_and_flow_limits_that_run() {
    let server = common::TestServer::start_with(|c| {
        c.flow_max_depth = 7;
        c.flow_max_fan_out = 33;
        c.flow_max_tokens = 123_456;
        c.flow_timeout_secs = 600;
    })
    .await;
    let admin = init_admin(&server).await;
    let provider = nasiko_llm_router::GatewayConfig::from_env().default_provider;

    let check = |body: &Value| {
        assert_eq!(body["default_provider"], provider.as_str());
        assert_eq!(body["max_flow_depth"], 7);
        assert_eq!(body["max_flow_fan_out"], 33);
        assert_eq!(body["max_flow_tokens"], 123_456);
        assert_eq!(body["flow_timeout_secs"], 600);
    };
    check(&get(&server, &admin).await);

    // An older page sends the values it used to edit: they are ignored, and the stored columns
    // that remain still save.
    let res = common::as_superuser(
        server.client.put(server.url("/api/settings")),
        &admin,
        "admin",
    )
    .json(&json!({
        "default_provider": "not-a-provider",
        "max_flow_depth": 1,
        "max_flow_fan_out": 1,
        "max_flow_tokens": 1,
        "flow_timeout_secs": 120,
        "catalog_tabs": "devops",
    }))
    .send()
    .await
    .unwrap();
    assert_eq!(res.status().as_u16(), 200);
    let saved: Value = res.json().await.unwrap();
    check(&saved);
    assert_eq!(saved["catalog_tabs"], "devops");

    let after = get(&server, &admin).await;
    check(&after);
    assert_eq!(after["catalog_tabs"], "devops");
}

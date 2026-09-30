//! Integration tests for the per-agent payload-compression toggle.
//!
//! Covers the round trip the Settings switch depends on:
//!   - PUT  /api/agents/{id} {"compress_enabled": true}  — persists
//!   - GET  /api/agents/{id}                             — **reports it back**
//!   - PUT  with the field omitted                       — leaves it alone
//!
//! The middle one is the regression this file exists for. `GET /api/agents/{id}` does not
//! serialize the `Agent` model — it builds `AgentDetailResponse`, a hand-written projection.
//! A field added to the model and to the update path but not to that projection persists
//! correctly and reads back as `false`, so the switch shows off however the column reads,
//! and toggling it "works" right up until you reload the page. Nothing else catches that:
//! the write path, the column, and the router all behave.
//!
//! Requires infra (Postgres :5432, Redis, S3):
//!   cargo test -p nasiko-server --test agent_compress_toggle -- --test-threads=1

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

async fn create_agent(server: &common::TestServer, uid: &str, name: &str) -> Value {
    let res = common::as_superuser(server.client.post(server.url("/api/agents")), uid, "admin")
        .json(&json!({"name": name, "version": "1.0.0"}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 201);
    res.json::<Value>().await.unwrap()
}

async fn get_agent(server: &common::TestServer, uid: &str, id: &str) -> Value {
    let res = common::as_superuser(
        server.client.get(server.url(&format!("/api/agents/{id}"))),
        uid,
        "admin",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 200);
    let body: Value = res.json().await.unwrap();
    body["data"].clone()
}

async fn put_agent(server: &common::TestServer, uid: &str, id: &str, body: Value) {
    let res = common::as_superuser(
        server.client.put(server.url(&format!("/api/agents/{id}"))),
        uid,
        "admin",
    )
    .json(&body)
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 200, "update should succeed");
}

#[tokio::test]
#[serial]
async fn compress_toggle_survives_a_reload() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap().to_string();
    let agent = create_agent(&server, &uid, &format!("compress-{}", uuid::Uuid::new_v4())).await;
    let id = agent["id"].as_str().unwrap().to_string();

    // Off by default, and the field must actually be present — a missing key reads as
    // `false` on the client and is indistinguishable from "off" until someone toggles it.
    let fetched = get_agent(&server, &uid, &id).await;
    assert!(
        fetched.get("compress_enabled").is_some(),
        "GET /api/agents/{{id}} omits compress_enabled; the Settings switch cannot render its \
         own state. Present keys: {:?}",
        fetched.as_object().map(|o| o.keys().collect::<Vec<_>>())
    );
    assert_eq!(fetched["compress_enabled"], json!(false));

    put_agent(&server, &uid, &id, json!({"compress_enabled": true})).await;

    // The actual regression: the value must come back, not just land in the column.
    let fetched = get_agent(&server, &uid, &id).await;
    assert_eq!(
        fetched["compress_enabled"],
        json!(true),
        "toggle persisted but did not read back — the switch will show off after a reload"
    );

    put_agent(&server, &uid, &id, json!({"compress_enabled": false})).await;
    assert_eq!(
        get_agent(&server, &uid, &id).await["compress_enabled"],
        json!(false)
    );
}

#[tokio::test]
#[serial]
async fn an_unrelated_update_does_not_reset_the_toggle() {
    // The Settings tab saves display name and description through the same endpoint. Without
    // COALESCE on the column, renaming an agent would silently switch compression back off.
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap().to_string();
    let agent = create_agent(&server, &uid, &format!("compress-{}", uuid::Uuid::new_v4())).await;
    let id = agent["id"].as_str().unwrap().to_string();

    put_agent(&server, &uid, &id, json!({"compress_enabled": true})).await;
    put_agent(&server, &uid, &id, json!({"display_name": "Renamed"})).await;

    let fetched = get_agent(&server, &uid, &id).await;
    assert_eq!(fetched["display_name"], json!("Renamed"));
    assert_eq!(
        fetched["compress_enabled"],
        json!(true),
        "an unrelated field update reset the compression toggle"
    );
}

#[tokio::test]
#[serial]
async fn the_toggle_is_scoped_to_one_agent() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap().to_string();
    let a = create_agent(
        &server,
        &uid,
        &format!("compress-a-{}", uuid::Uuid::new_v4()),
    )
    .await;
    let b = create_agent(
        &server,
        &uid,
        &format!("compress-b-{}", uuid::Uuid::new_v4()),
    )
    .await;
    let (a_id, b_id) = (
        a["id"].as_str().unwrap().to_string(),
        b["id"].as_str().unwrap().to_string(),
    );

    put_agent(&server, &uid, &a_id, json!({"compress_enabled": true})).await;

    assert_eq!(
        get_agent(&server, &uid, &a_id).await["compress_enabled"],
        json!(true)
    );
    assert_eq!(
        get_agent(&server, &uid, &b_id).await["compress_enabled"],
        json!(false),
        "enabling compression on one agent leaked to another"
    );
}

// ─── metadata / feature flags ───────────────────────────────────────────────
//
// Same projection trap as the compress toggle above, third occurrence. The Settings
// "Features" switches (prompt comments, and anything added beside it) render from
// `metadata.features`, and the UI builds its PUT body by spreading the value it read back.
// Omit `metadata` from `AgentDetailResponse` and both halves break at once: the switch shows
// off whatever the column says, and each save replaces the whole column with the single
// feature being toggled.

#[tokio::test]
#[serial]
async fn get_agent_reports_metadata_back() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();
    let agent = create_agent(&server, uid, "metadata-roundtrip").await;
    let id = agent["id"].as_str().unwrap();

    put_agent(
        &server,
        uid,
        id,
        json!({"metadata": {"features": {"prompt_comments": "enabled"}}}),
    )
    .await;

    let fetched = get_agent(&server, uid, id).await;
    assert!(
        fetched.get("metadata").is_some(),
        "GET omits metadata; every Features switch renders off however the column reads: {fetched}"
    );
    assert_eq!(
        fetched["metadata"]["features"]["prompt_comments"],
        json!("enabled")
    );

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_feature_toggle_does_not_wipe_the_rest_of_metadata() {
    // The UI spreads what GET returned, so a projection that drops metadata silently turns
    // every toggle into "replace the column". Pin the round trip that makes spreading safe.
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();
    let agent = create_agent(&server, uid, "metadata-preserve").await;
    let id = agent["id"].as_str().unwrap();

    put_agent(
        &server,
        uid,
        id,
        json!({"metadata": {"keep_me": "yes", "features": {"prompt_comments": "disabled"}}}),
    )
    .await;

    // What the UI does: read, spread, flip one feature, write back.
    let before = get_agent(&server, uid, id).await;
    let mut metadata = before["metadata"].clone();
    metadata["features"]["prompt_comments"] = json!("enabled");
    put_agent(&server, uid, id, json!({ "metadata": metadata })).await;

    let after = get_agent(&server, uid, id).await;
    assert_eq!(
        after["metadata"]["features"]["prompt_comments"],
        json!("enabled")
    );
    assert_eq!(
        after["metadata"]["keep_me"],
        json!("yes"),
        "toggling one feature dropped the rest of metadata: {after}"
    );

    server.cleanup().await;
}

//! Integration tests for the per-agent compact-tool-definitions toggle.
//!
//! Same round trip the Settings switch depends on as `agent_compress_toggle.rs`:
//!   - PUT  /api/agents/{id} {"compact_tools_enabled": true} — persists
//!   - GET  /api/agents/{id}                                 — reports it back, plus whether the
//!     operator enabled the feature at all
//!   - PUT  with the field omitted                           — leaves it alone
//!   - PUT  by a granted non-owner                           — 403 (a grant is not management)
//!
//! Requires infra (Postgres :5432, Redis, S3):
//!   cargo test -p nasiko-server --test agent_compact_tools_toggle -- --test-threads=1

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

// ─── round trip ──────────────────────────────────────────────────────────────

#[tokio::test]
#[serial]
async fn compact_tools_toggle_survives_a_reload() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap().to_string();
    let agent = create_agent(&server, &uid, &format!("compact-{}", uuid::Uuid::new_v4())).await;
    let id = agent["id"].as_str().unwrap().to_string();

    let fetched = get_agent(&server, &uid, &id).await;
    assert!(
        fetched.get("compact_tools_enabled").is_some(),
        "GET /api/agents/{{id}} omits compact_tools_enabled; the Settings switch cannot render \
         its own state. Present keys: {:?}",
        fetched.as_object().map(|o| o.keys().collect::<Vec<_>>())
    );
    assert_eq!(fetched["compact_tools_enabled"], json!(false));

    put_agent(&server, &uid, &id, json!({"compact_tools_enabled": true})).await;
    assert_eq!(
        get_agent(&server, &uid, &id).await["compact_tools_enabled"],
        json!(true),
        "toggle persisted but did not read back"
    );

    put_agent(&server, &uid, &id, json!({"compact_tools_enabled": false})).await;
    assert_eq!(
        get_agent(&server, &uid, &id).await["compact_tools_enabled"],
        json!(false)
    );
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn an_unrelated_update_does_not_reset_the_toggle() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap().to_string();
    let agent = create_agent(&server, &uid, &format!("compact-{}", uuid::Uuid::new_v4())).await;
    let id = agent["id"].as_str().unwrap().to_string();

    put_agent(&server, &uid, &id, json!({"compact_tools_enabled": true})).await;
    put_agent(&server, &uid, &id, json!({"display_name": "Renamed"})).await;
    // The sibling switch is independent: turning compression on does not touch this one.
    put_agent(&server, &uid, &id, json!({"compress_enabled": true})).await;

    let fetched = get_agent(&server, &uid, &id).await;
    assert_eq!(fetched["display_name"], json!("Renamed"));
    assert_eq!(fetched["compact_tools_enabled"], json!(true));
    assert_eq!(fetched["compress_enabled"], json!(true));
    server.cleanup().await;
}

// ─── operator gate ───────────────────────────────────────────────────────────

#[tokio::test]
#[serial]
async fn availability_follows_the_server_configuration_and_the_stored_value_is_served_either_way() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap().to_string();
    let agent = create_agent(&server, &uid, &format!("compact-{}", uuid::Uuid::new_v4())).await;
    let id = agent["id"].as_str().unwrap().to_string();
    put_agent(&server, &uid, &id, json!({"compact_tools_enabled": true})).await;
    let fetched = get_agent(&server, &uid, &id).await;
    assert_eq!(fetched["compact_tools_available"], json!(false));
    assert_eq!(
        fetched["compact_tools_enabled"],
        json!(true),
        "the stored choice is served even while the operator gate is off"
    );
    server.cleanup().await;

    let server = common::TestServer::start_with(|c| c.compact_tools_enabled = true).await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap().to_string();
    let agent = create_agent(&server, &uid, &format!("compact-{}", uuid::Uuid::new_v4())).await;
    let id = agent["id"].as_str().unwrap().to_string();
    let fetched = get_agent(&server, &uid, &id).await;
    assert_eq!(fetched["compact_tools_available"], json!(true));
    assert_eq!(fetched["compact_tools_enabled"], json!(false));
    server.cleanup().await;
}

// ─── authorization ───────────────────────────────────────────────────────────

#[tokio::test]
#[serial]
async fn a_user_grant_does_not_let_a_non_owner_change_the_toggle() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let admin_id = admin["user_id"].as_str().unwrap().to_string();
    let agent = create_agent(
        &server,
        &admin_id,
        &format!("compact-{}", uuid::Uuid::new_v4()),
    )
    .await;
    let id = agent["id"].as_str().unwrap().to_string();
    let member = create_user(
        &server,
        &admin_id,
        &format!("m{}", uuid::Uuid::new_v4().simple()),
    )
    .await;
    let member_id = member["user_id"]
        .as_str()
        .or_else(|| member["id"].as_str())
        .unwrap()
        .to_string();
    sqlx::query(
        "INSERT INTO agent_grants (agent_id, grant_type, grantee_id) VALUES ($1, 'user', $2)",
    )
    .bind(uuid::Uuid::parse_str(&id).unwrap())
    .bind(&member_id)
    .execute(&server.db)
    .await
    .unwrap();

    // The grant lets the member read the agent, field included.
    let res = common::as_member(
        server.client.get(server.url(&format!("/api/agents/{id}"))),
        &member_id,
        "member",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 200);
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["data"]["compact_tools_enabled"], json!(false));

    // It does not let them manage it.
    let res = common::as_member(
        server.client.put(server.url(&format!("/api/agents/{id}"))),
        &member_id,
        "member",
    )
    .json(&json!({"compact_tools_enabled": true}))
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 403);
    assert_eq!(
        get_agent(&server, &admin_id, &id).await["compact_tools_enabled"],
        json!(false)
    );
    server.cleanup().await;
}

// ─── flag parsing agreement ──────────────────────────────────────────────────

/// The server's `TOKEN_COMPACT_TOOLS` reading (for `compact_tools_available`) and the router's
/// (for actually applying the feature) must agree on every input, or the UI could call the
/// feature unavailable while the router is applying it, or the reverse.
#[test]
fn server_and_router_parse_the_flag_identically() {
    for value in [
        Some("true"),
        Some("1"),
        Some("false"),
        Some("0"),
        Some("yes"),
        Some("TRUE"),
        Some(""),
        None,
    ] {
        for default in [false, true] {
            assert_eq!(
                nasiko_config::flag_from(value, default),
                nasiko_llm_router::config::flag_from(value, default),
                "{value:?} / {default}"
            );
        }
    }
}

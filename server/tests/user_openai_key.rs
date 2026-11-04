//! The OpenAI key the server spends for a user: the one they saved from the UI, then the server's.
//!
//!   cargo test -p nasiko-server --test user_openai_key -- --test-threads=1

mod common;

use nasiko_orchestrator::user_key::{openai_key, saved_openai_key};
use nasiko_secrets::SecretsCrypto;
use serde_json::{Value, json};
use serial_test::serial;
use uuid::Uuid;

async fn init_admin(server: &common::TestServer) -> Uuid {
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
    body["user_id"].as_str().unwrap().parse().unwrap()
}

async fn save_secret(server: &common::TestServer, user: Uuid, name: &str, value: &str) {
    sqlx::query("INSERT INTO user_secrets (user_id, name, encrypted_value) VALUES ($1, $2, $3)")
        .bind(user)
        .bind(name)
        .bind(SecretsCrypto::for_user(user).encrypt(value))
        .execute(&server.db)
        .await
        .unwrap();
}

async fn save_config(
    server: &common::TestServer,
    user: Uuid,
    (name, provider): (&str, &str),
    secret: &str,
    is_default: bool,
) {
    sqlx::query(
        "INSERT INTO llm_configs (created_by, name, provider, model, api_key_secret_name, is_default) \
         VALUES ($1, $2, $3, 'm', $4, $5)",
    )
    .bind(user)
    .bind(name)
    .bind(provider)
    .bind(secret)
    .bind(is_default)
    .execute(&server.db)
    .await
    .unwrap();
}

#[tokio::test]
#[serial]
async fn a_user_with_no_saved_key_gets_the_servers() {
    let server = common::TestServer::start().await;
    let user = init_admin(&server).await;

    assert_eq!(saved_openai_key(&server.db, user).await, None);
    assert_eq!(openai_key(&server.db, user, "sk-server").await, "sk-server");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn the_saved_key_wins_over_the_servers() {
    let server = common::TestServer::start().await;
    let user = init_admin(&server).await;
    save_secret(&server, user, "OPENAI_API_KEY", "  sk-saved\n").await;
    save_config(
        &server,
        user,
        ("openai-setup", "openai"),
        "OPENAI_API_KEY",
        true,
    )
    .await;

    assert_eq!(
        saved_openai_key(&server.db, user).await.as_deref(),
        Some("sk-saved")
    );
    assert_eq!(openai_key(&server.db, user, "sk-server").await, "sk-saved");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn another_users_key_is_never_used() {
    let server = common::TestServer::start().await;
    let owner = init_admin(&server).await;
    save_secret(&server, owner, "OPENAI_API_KEY", "sk-owner").await;
    save_config(&server, owner, ("c", "openai"), "OPENAI_API_KEY", true).await;

    assert_eq!(saved_openai_key(&server.db, Uuid::new_v4()).await, None);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn only_a_live_openai_config_counts() {
    let server = common::TestServer::start().await;
    let user = init_admin(&server).await;
    save_secret(&server, user, "ANTHROPIC_API_KEY", "sk-ant").await;
    save_secret(&server, user, "OPENAI_API_KEY", "sk-deleted").await;
    save_config(
        &server,
        user,
        ("anthropic-setup", "anthropic"),
        "ANTHROPIC_API_KEY",
        true,
    )
    .await;
    save_config(&server, user, ("old", "openai"), "OPENAI_API_KEY", false).await;
    sqlx::query("UPDATE llm_configs SET deleted_at = now() WHERE name = 'old'")
        .execute(&server.db)
        .await
        .unwrap();

    assert_eq!(saved_openai_key(&server.db, user).await, None);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_non_default_openai_config_is_used_when_the_default_is_another_provider() {
    let server = common::TestServer::start().await;
    let user = init_admin(&server).await;
    save_secret(&server, user, "ANTHROPIC_API_KEY", "sk-ant").await;
    save_secret(&server, user, "OPENAI_API_KEY", "sk-openai").await;
    save_config(
        &server,
        user,
        ("anthropic-setup", "anthropic"),
        "ANTHROPIC_API_KEY",
        true,
    )
    .await;
    save_config(
        &server,
        user,
        ("openai-setup", "openai"),
        "OPENAI_API_KEY",
        false,
    )
    .await;

    assert_eq!(
        saved_openai_key(&server.db, user).await.as_deref(),
        Some("sk-openai")
    );

    server.cleanup().await;
}

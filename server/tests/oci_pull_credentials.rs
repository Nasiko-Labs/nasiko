//! Regression tests for the per-agent OCI pull credential path (Gap #52):
//! `nasiko deploy`'s image pulls on a real K8s cluster can't use the
//! registry's normal bearer-JWT auth, so `nasiko_oci::pull_credentials`
//! mints a separate HTTP Basic-auth credential scoped to exactly one agent.
//!
//! Requires infra (Postgres): `cargo test -p nasiko-server --test oci_pull_credentials -- --test-threads=1`

mod common;

use serial_test::serial;
use sqlx::PgPool;
use uuid::Uuid;

async fn insert_user(db: &PgPool, username: &str) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO users (username, email, is_superuser) VALUES ($1, $2, false) RETURNING id",
    )
    .bind(username)
    .bind(format!("{username}@oci-pull-test.com"))
    .fetch_one(db)
    .await
    .expect("insert test user")
}

async fn insert_agent(db: &PgPool, name: &str, owner_id: Uuid) -> Uuid {
    sqlx::query_scalar("INSERT INTO agents (name, owner_id, version, image) VALUES ($1, $2, '1.0.0', 'img:1') RETURNING id")
        .bind(name)
        .bind(owner_id)
        .fetch_one(db)
        .await
        .expect("insert test agent")
}

async fn insert_manifest(db: &PgPool, repository: &str) {
    sqlx::query(
        "INSERT INTO oci_manifests (digest, repository, reference, media_type, content, size_bytes)
         VALUES ($1, $2, 'latest', 'application/vnd.oci.image.manifest.v1+json', '{}', 2)",
    )
    .bind(format!("sha256:{}", Uuid::new_v4().simple()))
    .bind(repository)
    .execute(db)
    .await
    .expect("insert test manifest");
}

#[tokio::test]
#[serial]
async fn get_or_create_mints_once_then_returns_none() {
    let server = common::TestServer::start().await;
    let owner_id = insert_user(&server.db, "pull-cred-owner").await;
    let agent_id = insert_agent(
        &server.db,
        &format!("pull-cred-agent-{}", Uuid::new_v4()),
        owner_id,
    )
    .await;

    let first = nasiko_oci::pull_credentials::get_or_create(&server.db, agent_id)
        .await
        .unwrap();
    assert!(
        first.is_some(),
        "first mint must return the plaintext credential"
    );

    let second = nasiko_oci::pull_credentials::get_or_create(&server.db, agent_id)
        .await
        .unwrap();
    assert!(
        second.is_none(),
        "a live credential already exists — nothing new to seed"
    );

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn get_or_create_remints_after_revoke() {
    let server = common::TestServer::start().await;
    let owner_id = insert_user(&server.db, "pull-cred-owner2").await;
    let agent_id = insert_agent(
        &server.db,
        &format!("pull-cred-agent-{}", Uuid::new_v4()),
        owner_id,
    )
    .await;

    let first = nasiko_oci::pull_credentials::get_or_create(&server.db, agent_id)
        .await
        .unwrap()
        .unwrap();
    nasiko_oci::pull_credentials::revoke(&server.db, agent_id)
        .await
        .unwrap();

    let reminted = nasiko_oci::pull_credentials::get_or_create(&server.db, agent_id)
        .await
        .unwrap();
    assert!(
        reminted.is_some(),
        "a revoked credential must be treated as absent — re-mints"
    );
    assert_ne!(
        reminted.unwrap().token,
        first.token,
        "the new token must not reuse the revoked one"
    );

    // The old token must no longer verify.
    assert!(
        nasiko_oci::pull_credentials::verify(&server.db, &first.username, &first.token)
            .await
            .unwrap()
            .is_none()
    );

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn tags_list_allows_matching_pull_credential() {
    let server = common::TestServer::start().await;
    let owner_id = insert_user(&server.db, "pull-cred-owner3").await;
    let agent_name = format!("pull-cred-agent-{}", Uuid::new_v4());
    let agent_id = insert_agent(&server.db, &agent_name, owner_id).await;
    insert_manifest(&server.db, &format!("nasiko/{agent_name}")).await;
    let cred = nasiko_oci::pull_credentials::get_or_create(&server.db, agent_id)
        .await
        .unwrap()
        .unwrap();

    let resp = common::as_pull_credential(
        server
            .client
            .get(server.url(&format!("/v2/nasiko/{agent_name}/tags/list"))),
        &cred.username,
        &cred.token,
    )
    .send()
    .await
    .unwrap();

    assert_eq!(
        resp.status(),
        200,
        "a pull credential must be able to read its own agent's repo"
    );

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn tags_list_denies_pull_credential_for_a_different_repo() {
    let server = common::TestServer::start().await;
    let owner_id = insert_user(&server.db, "pull-cred-owner4").await;
    let agent_name = format!("pull-cred-agent-{}", Uuid::new_v4());
    let other_agent_name = format!("pull-cred-other-{}", Uuid::new_v4());
    let agent_id = insert_agent(&server.db, &agent_name, owner_id).await;
    insert_agent(&server.db, &other_agent_name, owner_id).await;
    insert_manifest(&server.db, &format!("nasiko/{other_agent_name}")).await;
    let cred = nasiko_oci::pull_credentials::get_or_create(&server.db, agent_id)
        .await
        .unwrap()
        .unwrap();

    let resp = common::as_pull_credential(
        server
            .client
            .get(server.url(&format!("/v2/nasiko/{other_agent_name}/tags/list"))),
        &cred.username,
        &cred.token,
    )
    .send()
    .await
    .unwrap();

    assert_eq!(
        resp.status(),
        403,
        "a pull credential must not be able to read a different agent's repo"
    );

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn tags_list_denies_wrong_password() {
    let server = common::TestServer::start().await;
    let owner_id = insert_user(&server.db, "pull-cred-owner5").await;
    let agent_name = format!("pull-cred-agent-{}", Uuid::new_v4());
    let agent_id = insert_agent(&server.db, &agent_name, owner_id).await;
    insert_manifest(&server.db, &format!("nasiko/{agent_name}")).await;
    let cred = nasiko_oci::pull_credentials::get_or_create(&server.db, agent_id)
        .await
        .unwrap()
        .unwrap();

    let resp = common::as_pull_credential(
        server
            .client
            .get(server.url(&format!("/v2/nasiko/{agent_name}/tags/list"))),
        &cred.username,
        "wrong-token",
    )
    .send()
    .await
    .unwrap();

    assert_eq!(resp.status(), 401);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn tags_list_denies_revoked_credential() {
    let server = common::TestServer::start().await;
    let owner_id = insert_user(&server.db, "pull-cred-owner6").await;
    let agent_name = format!("pull-cred-agent-{}", Uuid::new_v4());
    let agent_id = insert_agent(&server.db, &agent_name, owner_id).await;
    insert_manifest(&server.db, &format!("nasiko/{agent_name}")).await;
    let cred = nasiko_oci::pull_credentials::get_or_create(&server.db, agent_id)
        .await
        .unwrap()
        .unwrap();
    nasiko_oci::pull_credentials::revoke(&server.db, agent_id)
        .await
        .unwrap();

    let resp = common::as_pull_credential(
        server
            .client
            .get(server.url(&format!("/v2/nasiko/{agent_name}/tags/list"))),
        &cred.username,
        &cred.token,
    )
    .send()
    .await
    .unwrap();

    assert_eq!(resp.status(), 401, "a revoked credential must be rejected");

    server.cleanup().await;
}

/// A pull credential must never be usable on a mutating route, regardless of
/// what repo it's scoped to — `PUT manifest` handlers only accept
/// `CallerIdentity` (bearer-derived), which a Basic-auth request never gets.
#[tokio::test]
#[serial]
async fn pull_credential_cannot_push_a_manifest() {
    let server = common::TestServer::start().await;
    let owner_id = insert_user(&server.db, "pull-cred-owner7").await;
    let agent_name = format!("pull-cred-agent-{}", Uuid::new_v4());
    let agent_id = insert_agent(&server.db, &agent_name, owner_id).await;
    let cred = nasiko_oci::pull_credentials::get_or_create(&server.db, agent_id)
        .await
        .unwrap()
        .unwrap();

    let resp = common::as_pull_credential(
        server
            .client
            .put(server.url(&format!("/v2/nasiko/{agent_name}/manifests/latest")))
            .header("content-type", "application/vnd.oci.image.manifest.v1+json")
            .body("{}"),
        &cred.username,
        &cred.token,
    )
    .send()
    .await
    .unwrap();

    assert_eq!(
        resp.status(),
        401,
        "a pull credential must never be able to push a manifest"
    );

    server.cleanup().await;
}

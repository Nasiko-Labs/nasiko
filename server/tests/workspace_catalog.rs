//! Integration tests for the workspace catalog (`nasiko_server::workspace::catalog`).
//!
//! These exercise the data-access functions directly against a real Postgres —
//! the catalog has no HTTP surface of its own, and every invariant worth testing
//! here (history archival, tombstone clearing, quota accounting, the one-shot
//! upload capability, grant idempotency) lives in SQL rather than in Rust.
//!
//! Requires infra (Postgres :5432, Redis, S3):
//!   cargo test -p nasiko-server --test workspace_catalog -- --test-threads=1

mod common;

use chrono::{Duration, Utc};
use nasiko_server::workspace::catalog::{self, FileRow, NewPointer, Scope, UploadRow};
use serde_json::{Value, json};
use serial_test::serial;
use sqlx::PgPool;
use uuid::Uuid;

// ─── fixtures ────────────────────────────────────────────────────────────────

/// An admin user plus one agent they own — the `(agent, user)` pair every
/// workspace row is keyed on.
struct Fixture {
    agent_id: Uuid,
    user_id: Uuid,
}

async fn fixture(server: &common::TestServer) -> Fixture {
    let admin: Value = server
        .client
        .post(server.url("/api/auth/initialize-admin"))
        .json(&json!({"username": "admin", "email": "admin@test.local"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let user_id = admin["user_id"].as_str().unwrap().to_owned();

    let agent: Value = common::as_superuser(
        server.client.post(server.url("/api/agents")),
        &user_id,
        "admin",
    )
    .json(&json!({"name": "workspace-agent", "version": "1.0.0"}))
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();

    Fixture {
        agent_id: agent["id"].as_str().unwrap().parse().unwrap(),
        user_id: user_id.parse().unwrap(),
    }
}

/// Installs a pointer with the fields these tests do not vary held constant.
async fn install(
    db: &PgPool,
    fx: &Fixture,
    rel_path: &str,
    blob_key: &str,
    size_bytes: i64,
    state: &str,
) -> (FileRow, i64) {
    catalog::install_pointer(
        db,
        NewPointer {
            agent_id: fx.agent_id,
            user_id: fx.user_id,
            rel_path,
            blob_key,
            size_bytes,
            mime_type: "text/plain",
            digest: Some("sha256:aaaa"),
            origin: "agent",
            origin_agent_id: None,
            source_file_id: None,
            trace_id: "trace-aaaa",
            session_id: Some("session-1"),
            state,
            staged_until: None,
        },
    )
    .await
    .expect("install pointer")
}

async fn version_rows(db: &PgPool, file_id: Uuid) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM workspace_file_versions WHERE file_id=$1")
        .bind(file_id)
        .fetch_one(db)
        .await
        .unwrap()
}

// ─── pointers and history ────────────────────────────────────────────────────

#[tokio::test]
#[serial]
async fn second_write_archives_the_first_pointer() {
    let server = common::TestServer::start().await;
    let fx = fixture(&server).await;

    let (first, v1) = install(&server.db, &fx, "docs/a.txt", "blob-1", 100, "kept").await;
    assert_eq!(v1, 1, "a fresh path is version 1");

    let (second, v2) = install(&server.db, &fx, "docs/a.txt", "blob-2", 250, "kept").await;
    assert_eq!(v2, 2, "the overwrite is version 2");
    assert_eq!(second.id, first.id, "the path keeps its file identity");
    assert_eq!(second.blob_key, "blob-2");

    assert_eq!(version_rows(&server.db, first.id).await, 1);
    let archived: (String, i64) =
        sqlx::query_as("SELECT blob_key, size_bytes FROM workspace_file_versions WHERE file_id=$1")
            .bind(first.id)
            .fetch_one(&server.db)
            .await
            .unwrap();
    assert_eq!(
        archived,
        ("blob-1".to_owned(), 100),
        "history holds the superseded pointer"
    );

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn rewriting_a_tombstoned_path_clears_it_without_archiving() {
    let server = common::TestServer::start().await;
    let fx = fixture(&server).await;

    let (first, _) = install(&server.db, &fx, "a.txt", "blob-1", 100, "kept").await;
    catalog::tombstone(&server.db, first.id).await.unwrap();

    // A tombstone hides the row from path lookups but not from id lookups.
    assert!(
        catalog::get_by_path(&server.db, fx.agent_id, fx.user_id, "a.txt")
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        catalog::get_by_id(&server.db, first.id)
            .await
            .unwrap()
            .is_some()
    );

    let (revived, version) = install(&server.db, &fx, "a.txt", "blob-2", 70, "kept").await;
    assert_eq!(revived.id, first.id);
    assert_eq!(revived.deleted_at, None, "the tombstone is cleared");
    assert_eq!(version, 1, "the deleted pointer was not archived");
    assert_eq!(version_rows(&server.db, first.id).await, 0);

    server.cleanup().await;
}

// ─── listing ─────────────────────────────────────────────────────────────────

#[tokio::test]
#[serial]
async fn list_separates_the_inbox_from_kept_files_and_honours_a_prefix() {
    let server = common::TestServer::start().await;
    let fx = fixture(&server).await;

    install(&server.db, &fx, "docs/a.txt", "blob-a", 10, "kept").await;
    install(&server.db, &fx, "notes/b.txt", "blob-b", 10, "kept").await;
    install(&server.db, &fx, "inbox/c.txt", "blob-c", 10, "staged").await;

    let kept = catalog::list(&server.db, fx.agent_id, fx.user_id, None, Scope::All, 50)
        .await
        .unwrap();
    let kept_paths: Vec<&str> = kept.iter().map(|f| f.rel_path.as_str()).collect();
    assert_eq!(
        kept.len(),
        2,
        "staged rows stay out of the workspace proper"
    );
    assert!(kept_paths.contains(&"docs/a.txt") && kept_paths.contains(&"notes/b.txt"));

    let staged = catalog::list(&server.db, fx.agent_id, fx.user_id, None, Scope::Inbox, 50)
        .await
        .unwrap();
    assert_eq!(staged.len(), 1);
    assert_eq!(staged[0].rel_path, "inbox/c.txt");

    let under_docs = catalog::list(
        &server.db,
        fx.agent_id,
        fx.user_id,
        Some("docs/"),
        Scope::All,
        50,
    )
    .await
    .unwrap();
    assert_eq!(under_docs.len(), 1);
    assert_eq!(under_docs[0].rel_path, "docs/a.txt");

    server.cleanup().await;
}

// ─── quota accounting ────────────────────────────────────────────────────────

#[tokio::test]
#[serial]
async fn bytes_used_counts_current_pointers_and_history() {
    let server = common::TestServer::start().await;
    let fx = fixture(&server).await;

    install(&server.db, &fx, "a.txt", "blob-1", 100, "kept").await;
    assert_eq!(
        catalog::bytes_used(&server.db, fx.agent_id, fx.user_id)
            .await
            .unwrap(),
        100
    );

    install(&server.db, &fx, "a.txt", "blob-2", 250, "kept").await;
    assert_eq!(
        catalog::bytes_used(&server.db, fx.agent_id, fx.user_id)
            .await
            .unwrap(),
        350,
        "the superseded blob still occupies the store"
    );

    server.cleanup().await;
}

// ─── settings ────────────────────────────────────────────────────────────────

#[tokio::test]
#[serial]
async fn settings_upserts_defaults_and_is_stable() {
    let server = common::TestServer::start().await;
    let fx = fixture(&server).await;

    let first = catalog::settings(&server.db, fx.agent_id).await.unwrap();
    assert_eq!(first.agent_id, fx.agent_id);
    assert!(first.enabled);
    assert_eq!(first.quota_bytes, 5_368_709_120);
    assert_eq!(first.file_max_bytes, 1_073_741_824);
    assert_eq!(first.inline_max_bytes, 6_291_456);
    assert_eq!(first.history_retention_days, 30);
    assert!(!first.auto_keep_uploads);
    assert!(!first.result_links);

    let second = catalog::settings(&server.db, fx.agent_id).await.unwrap();
    assert_eq!(second.quota_bytes, first.quota_bytes);
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM agent_workspace_settings")
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert_eq!(rows, 1, "the second call read the row, it did not add one");

    server.cleanup().await;
}

// ─── uploads ─────────────────────────────────────────────────────────────────

#[tokio::test]
#[serial]
async fn upload_capability_is_one_shot_and_finishing_stamps_the_row() {
    let server = common::TestServer::start().await;
    let fx = fixture(&server).await;

    let upload_id = Uuid::new_v4();
    let staged = UploadRow {
        upload_id,
        agent_id: fx.agent_id,
        user_id: fx.user_id,
        rel_path: "uploads/report.pdf".into(),
        declared_size: Some(2048),
        mime_type: Some("application/pdf".into()),
        blob_key: catalog::blob_key(fx.agent_id, fx.user_id, upload_id),
        mode: "relay".into(),
        capability_hash: Some("cap-hash-1".into()),
        state: "pending".into(),
        received_size: None,
        received_digest: None,
        trace_id: "trace-aaaa".into(),
        session_id: Some("session-1".into()),
        expires_at: Utc::now() + Duration::hours(1),
    };
    catalog::create_upload(&server.db, &staged).await.unwrap();

    let found = catalog::get_upload_by_capability(&server.db, "cap-hash-1")
        .await
        .unwrap()
        .expect("a pending, unexpired capability resolves");
    assert_eq!(found.upload_id, upload_id);
    assert_eq!(found.state, "pending");

    catalog::mark_received(&server.db, upload_id, 2048, "sha256:bbbb")
        .await
        .unwrap();
    assert!(
        catalog::get_upload_by_capability(&server.db, "cap-hash-1")
            .await
            .unwrap()
            .is_none(),
        "a received upload no longer answers its capability"
    );

    let received = catalog::get_upload(&server.db, upload_id)
        .await
        .unwrap()
        .expect("the row is still readable by id");
    assert_eq!(received.state, "received");
    assert_eq!(received.received_size, Some(2048));
    assert_eq!(received.received_digest.as_deref(), Some("sha256:bbbb"));

    catalog::mark_upload_state(&server.db, upload_id, "finished")
        .await
        .unwrap();
    let finished: (String, bool) = sqlx::query_as(
        "SELECT state, finished_at IS NOT NULL FROM workspace_uploads WHERE upload_id=$1",
    )
    .bind(upload_id)
    .fetch_one(&server.db)
    .await
    .unwrap();
    assert_eq!(finished, ("finished".to_owned(), true));

    server.cleanup().await;
}

// ─── grants ──────────────────────────────────────────────────────────────────

#[tokio::test]
#[serial]
async fn grants_are_idempotent_and_only_the_grantee_sees_the_file() {
    let server = common::TestServer::start().await;
    let fx = fixture(&server).await;

    let other: Value = common::as_superuser(
        server.client.post(server.url("/api/users")),
        &fx.user_id.to_string(),
        "admin",
    )
    .json(&json!({"username": "grantee", "email": "grantee@test.local"}))
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    let grantee_id: Uuid = other["id"].as_str().unwrap().parse().unwrap();

    let (file, _) = install(&server.db, &fx, "docs/shared.txt", "blob-1", 100, "kept").await;

    let grant = catalog::add_grant(
        &server.db,
        file.id,
        "user",
        &grantee_id.to_string(),
        fx.user_id,
        Some(fx.agent_id),
    )
    .await
    .unwrap();
    let regrant = catalog::add_grant(
        &server.db,
        file.id,
        "user",
        &grantee_id.to_string(),
        fx.user_id,
        Some(fx.agent_id),
    )
    .await
    .unwrap();
    assert_eq!(
        regrant.id, grant.id,
        "re-granting updates, it does not duplicate"
    );
    assert_eq!(
        catalog::grants_for_file(&server.db, file.id)
            .await
            .unwrap()
            .len(),
        1
    );

    let for_grantee = catalog::shared_with_user(&server.db, grantee_id, 50)
        .await
        .unwrap();
    assert_eq!(for_grantee.len(), 1);
    assert_eq!(for_grantee[0].id, file.id);
    assert!(
        catalog::shared_with_user(&server.db, fx.user_id, 50)
            .await
            .unwrap()
            .is_empty(),
        "an owner's own files are not 'shared with' them"
    );

    assert_eq!(
        catalog::remove_grant(&server.db, grant.id).await.unwrap(),
        1
    );
    assert_eq!(
        catalog::remove_grant(&server.db, grant.id).await.unwrap(),
        0
    );

    server.cleanup().await;
}

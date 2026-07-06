//! Integration tests for chat file attachments (P4).
//!
//! Covers: upload, attach-to-message, list, download redirect, delete, IDOR guard,
//!         session-delete cascade, size limit, file count limit.
//!
//! Requires: `docker compose --profile infra up -d` (Postgres + MinIO).
//! Run: `cargo test -p nasiko-server --test chat_files -- --test-threads=1`

mod common;

use serde_json::{Value, json};
use serial_test::serial;

fn no_redirect_client() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap()
}

// ─── Helpers ────────────────────────────────────────────────────────────────

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

async fn create_session(server: &common::TestServer, user_id: &str, is_super: bool) -> Value {
    let rb = server.client.post(server.url("/api/chat/sessions"));
    if is_super {
        common::as_superuser(rb, user_id, "u")
    } else {
        common::as_member(rb, user_id, "u")
    }
    .json(&json!({"title": "test session"}))
    .send()
    .await
    .unwrap()
    .json::<Value>()
    .await
    .unwrap()
}

/// Upload a single small text file; returns the file record.
async fn upload_file(
    server: &common::TestServer,
    user_id: &str,
    is_super: bool,
    session_id: &str,
    content: &'static str,
    filename: &str,
) -> (u16, Value) {
    let part = reqwest::multipart::Part::bytes(content.as_bytes().to_vec())
        .file_name(filename.to_string())
        .mime_str("text/plain")
        .unwrap();
    let form = reqwest::multipart::Form::new().part("file", part);

    let rb = server.client.post(server.url(&format!("/api/chat/sessions/{session_id}/files")));
    let res = if is_super {
        common::as_superuser(rb, user_id, "u")
    } else {
        common::as_member(rb, user_id, "u")
    }
    .multipart(form)
    .send()
    .await
    .unwrap();

    let status = res.status().as_u16();
    let body: Value = res.json().await.unwrap_or(Value::Null);
    (status, body)
}

async fn send_message_with_files(
    server: &common::TestServer,
    user_id: &str,
    is_super: bool,
    session_id: &str,
    file_ids: Vec<&str>,
) -> (u16, Value) {
    let rb = server.client.post(server.url(&format!("/api/chat/sessions/{session_id}/messages")));
    let res = if is_super {
        common::as_superuser(rb, user_id, "u")
    } else {
        common::as_member(rb, user_id, "u")
    }
    .json(&json!({"role": "user", "content": "see attached", "file_ids": file_ids}))
    .send()
    .await
    .unwrap();
    let status = res.status().as_u16();
    let body: Value = res.json().await.unwrap_or(Value::Null);
    (status, body)
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[tokio::test]
#[serial]
async fn upload_file_returns_metadata() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    let session = create_session(&server, uid, true).await;
    let sid = session["session_id"].as_str().unwrap();

    let (status, body) = upload_file(&server, uid, true, sid, "hello", "hello.txt").await;
    assert_eq!(status, 201, "upload should succeed: {body}");

    let files = body.as_array().unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0]["filename"], "hello.txt");
    assert_eq!(files[0]["size_bytes"], 5);
    assert!(files[0]["id"].as_str().is_some(), "file id present");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn send_message_attaches_files() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    let session = create_session(&server, uid, true).await;
    let sid = session["session_id"].as_str().unwrap();

    let (_, files) = upload_file(&server, uid, true, sid, "data", "data.txt").await;
    let file_id = files[0]["id"].as_str().unwrap();

    let (status, msg) = send_message_with_files(&server, uid, true, sid, vec![file_id]).await;
    assert_eq!(status, 201, "send should succeed: {msg}");
    assert_eq!(msg["has_file_parts"], true, "has_file_parts should be set");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn list_files_for_message() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    let session = create_session(&server, uid, true).await;
    let sid = session["session_id"].as_str().unwrap();

    let (_, files) = upload_file(&server, uid, true, sid, "content", "note.txt").await;
    let file_id = files[0]["id"].as_str().unwrap();

    let (_, msg) = send_message_with_files(&server, uid, true, sid, vec![file_id]).await;
    let msg_id = msg["id"].as_str().unwrap();

    let res = common::as_superuser(
        server.client.get(server.url(&format!("/api/chat/sessions/{sid}/messages/{msg_id}/files"))),
        uid,
        "admin",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 200);
    let listed: Value = res.json().await.unwrap();
    let arr = listed.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["id"], file_id);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn download_returns_redirect() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    let session = create_session(&server, uid, true).await;
    let sid = session["session_id"].as_str().unwrap();

    let (_, files) = upload_file(&server, uid, true, sid, "abc", "abc.txt").await;
    let file_id = files[0]["id"].as_str().unwrap();

    let res = common::as_superuser(
        no_redirect_client().get(server.url(&format!("/api/chat/files/{file_id}/download"))),
        uid,
        "admin",
    )
    .send()
    .await
    .unwrap();

    assert_eq!(res.status(), 307, "expect redirect to presigned URL");
    let location = res.headers().get("location").unwrap().to_str().unwrap();
    assert!(!location.is_empty(), "Location header should be set");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn cannot_attach_already_attached_file() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    let session = create_session(&server, uid, true).await;
    let sid = session["session_id"].as_str().unwrap();

    let (_, files) = upload_file(&server, uid, true, sid, "x", "x.txt").await;
    let file_id = files[0]["id"].as_str().unwrap();

    // First attach succeeds.
    let (s1, _) = send_message_with_files(&server, uid, true, sid, vec![file_id]).await;
    assert_eq!(s1, 201);

    // Second attach of the same file_id must fail.
    let (s2, _) = send_message_with_files(&server, uid, true, sid, vec![file_id]).await;
    assert_eq!(s2, 400, "cannot attach an already-attached file");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn cannot_access_other_users_file() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let admin_id = admin["user_id"].as_str().unwrap();

    // Admin uploads a file.
    let session = create_session(&server, admin_id, true).await;
    let sid = session["session_id"].as_str().unwrap();
    let (_, files) = upload_file(&server, admin_id, true, sid, "secret", "secret.txt").await;
    let file_id = files[0]["id"].as_str().unwrap();

    // Another user tries to download it.
    let alice = create_user(&server, admin_id, "alice-files").await;
    let alice_id = alice["id"].as_str().unwrap();

    let res = common::as_member(
        no_redirect_client().get(server.url(&format!("/api/chat/files/{file_id}/download"))),
        alice_id,
        "alice-files",
    )
    .send()
    .await
    .unwrap();

    assert_eq!(res.status(), 404, "IDOR: other user must not access file");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn delete_removes_file() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    let session = create_session(&server, uid, true).await;
    let sid = session["session_id"].as_str().unwrap();

    let (_, files) = upload_file(&server, uid, true, sid, "bye", "bye.txt").await;
    let file_id = files[0]["id"].as_str().unwrap();

    let del = common::as_superuser(
        server.client.delete(server.url(&format!("/api/chat/files/{file_id}"))),
        uid,
        "admin",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(del.status(), 204);

    // Subsequent download should 404.
    let res = common::as_superuser(
        no_redirect_client().get(server.url(&format!("/api/chat/files/{file_id}/download"))),
        uid,
        "admin",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 404);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn session_delete_cleans_up_files() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    let session = create_session(&server, uid, true).await;
    let sid = session["session_id"].as_str().unwrap();

    let (_, files) = upload_file(&server, uid, true, sid, "orphan", "orphan.txt").await;
    let file_id = files[0]["id"].as_str().unwrap();

    // Delete the session.
    let del = common::as_superuser(
        server.client.delete(server.url(&format!("/api/chat/sessions/{sid}"))),
        uid,
        "admin",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(del.status(), 204);

    // File should be gone from DB.
    let res = common::as_superuser(
        no_redirect_client().get(server.url(&format!("/api/chat/files/{file_id}/download"))),
        uid,
        "admin",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 404, "file gone after session delete");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn too_many_files_rejected() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    let session = create_session(&server, uid, true).await;
    let sid = session["session_id"].as_str().unwrap();

    // Build a form with 11 files (over the limit of 10).
    let mut form = reqwest::multipart::Form::new();
    for i in 0..=10u8 {
        let part = reqwest::multipart::Part::bytes(vec![i])
            .file_name(format!("f{i}.bin"))
            .mime_str("application/octet-stream")
            .unwrap();
        form = form.part(format!("file{i}"), part);
    }

    let res = common::as_superuser(
        server.client.post(server.url(&format!("/api/chat/sessions/{sid}/files"))),
        uid,
        "admin",
    )
    .multipart(form)
    .send()
    .await
    .unwrap();

    assert_eq!(res.status(), 400, "11 files should be rejected");

    server.cleanup().await;
}

// ─── Additional coverage ─────────────────────────────────────────────────────

#[tokio::test]
#[serial]
async fn multiple_files_attached_to_one_message() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    let session = create_session(&server, uid, true).await;
    let sid = session["session_id"].as_str().unwrap();

    let (_, f1) = upload_file(&server, uid, true, sid, "aaa", "a.txt").await;
    let (_, f2) = upload_file(&server, uid, true, sid, "bbb", "b.txt").await;
    let id1 = f1[0]["id"].as_str().unwrap();
    let id2 = f2[0]["id"].as_str().unwrap();

    let (status, msg) = send_message_with_files(&server, uid, true, sid, vec![id1, id2]).await;
    assert_eq!(status, 201, "multi-file attach: {msg}");
    assert_eq!(msg["has_file_parts"], true);

    let msg_id = msg["id"].as_str().unwrap();
    let res = common::as_superuser(
        server.client.get(server.url(&format!("/api/chat/sessions/{sid}/messages/{msg_id}/files"))),
        uid,
        "admin",
    )
    .send()
    .await
    .unwrap();
    let listed: Value = res.json().await.unwrap();
    assert_eq!(listed.as_array().unwrap().len(), 2, "both files listed");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn cannot_delete_other_users_file() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let admin_id = admin["user_id"].as_str().unwrap();

    let session = create_session(&server, admin_id, true).await;
    let sid = session["session_id"].as_str().unwrap();
    let (_, files) = upload_file(&server, admin_id, true, sid, "mine", "mine.txt").await;
    let file_id = files[0]["id"].as_str().unwrap();

    let alice = create_user(&server, admin_id, "alice-del").await;
    let alice_id = alice["id"].as_str().unwrap();

    let res = common::as_member(
        server.client.delete(server.url(&format!("/api/chat/files/{file_id}"))),
        alice_id,
        "alice-del",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 404, "IDOR: other user must not delete file");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn cannot_list_files_for_other_users_session() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let admin_id = admin["user_id"].as_str().unwrap();

    let session = create_session(&server, admin_id, true).await;
    let sid = session["session_id"].as_str().unwrap();
    let (_, files) = upload_file(&server, admin_id, true, sid, "x", "x.txt").await;
    let (_, msg) = send_message_with_files(
        &server, admin_id, true, sid, vec![files[0]["id"].as_str().unwrap()],
    )
    .await;
    let msg_id = msg["id"].as_str().unwrap();

    let alice = create_user(&server, admin_id, "alice-list").await;
    let alice_id = alice["id"].as_str().unwrap();

    let res = common::as_member(
        server.client.get(server.url(&format!("/api/chat/sessions/{sid}/messages/{msg_id}/files"))),
        alice_id,
        "alice-list",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 404, "IDOR: other user must not list message files");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn send_message_with_nonexistent_file_id_rejected() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    let session = create_session(&server, uid, true).await;
    let sid = session["session_id"].as_str().unwrap();

    let fake_id = "00000000-0000-0000-0000-000000000001";
    let (status, _) = send_message_with_files(&server, uid, true, sid, vec![fake_id]).await;
    assert_eq!(status, 400, "non-existent file_id must be rejected");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn send_message_with_cross_session_file_id_rejected() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    let session_a = create_session(&server, uid, true).await;
    let sid_a = session_a["session_id"].as_str().unwrap();
    let (_, files) = upload_file(&server, uid, true, sid_a, "x", "x.txt").await;
    let file_id = files[0]["id"].as_str().unwrap();

    // Attempt to attach the file via a different session.
    let session_b = create_session(&server, uid, true).await;
    let sid_b = session_b["session_id"].as_str().unwrap();
    let (status, _) = send_message_with_files(&server, uid, true, sid_b, vec![file_id]).await;
    assert_eq!(status, 400, "file_id from another session must be rejected");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn empty_file_ids_sends_message_without_files() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    let session = create_session(&server, uid, true).await;
    let sid = session["session_id"].as_str().unwrap();

    let (status, msg) = send_message_with_files(&server, uid, true, sid, vec![]).await;
    assert_eq!(status, 201);
    assert_eq!(msg["has_file_parts"], false, "empty file_ids → has_file_parts false");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn delete_last_file_clears_has_file_parts() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    let session = create_session(&server, uid, true).await;
    let sid = session["session_id"].as_str().unwrap();

    let (_, files) = upload_file(&server, uid, true, sid, "z", "z.txt").await;
    let file_id = files[0]["id"].as_str().unwrap();
    let (_, msg) = send_message_with_files(&server, uid, true, sid, vec![file_id]).await;
    let msg_id = msg["id"].as_str().unwrap();
    assert_eq!(msg["has_file_parts"], true);

    common::as_superuser(
        server.client.delete(server.url(&format!("/api/chat/files/{file_id}"))),
        uid,
        "admin",
    )
    .send()
    .await
    .unwrap();

    let msgs: Value = common::as_superuser(
        server.client.get(server.url(&format!("/api/chat/sessions/{sid}/messages"))),
        uid,
        "admin",
    )
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    let updated = msgs["data"].as_array().unwrap().iter().find(|m| m["id"] == msg_id).unwrap();
    assert_eq!(updated["has_file_parts"], false, "has_file_parts cleared after last file deleted");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn delete_file_keeps_has_file_parts_when_inline_parts_present() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    let session = create_session(&server, uid, true).await;
    let sid = session["session_id"].as_str().unwrap();

    let (_, files) = upload_file(&server, uid, true, sid, "s3data", "s3.txt").await;
    let file_id = files[0]["id"].as_str().unwrap();

    // Send a message with both an S3-backed file AND inline file_parts.
    let msg: Value = common::as_superuser(
        server.client.post(server.url(&format!("/api/chat/sessions/{sid}/messages"))),
        uid,
        "admin",
    )
    .json(&json!({
        "role": "user",
        "content": "mixed",
        "file_parts": [{"type": "image_url", "url": "https://example.com/img.png"}],
        "file_ids": [file_id]
    }))
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    let msg_id = msg["id"].as_str().unwrap();
    assert_eq!(msg["has_file_parts"], true);

    // Delete only the S3 file — inline file_parts still present.
    common::as_superuser(
        server.client.delete(server.url(&format!("/api/chat/files/{file_id}"))),
        uid,
        "admin",
    )
    .send()
    .await
    .unwrap();

    let msgs: Value = common::as_superuser(
        server.client.get(server.url(&format!("/api/chat/sessions/{sid}/messages"))),
        uid,
        "admin",
    )
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    let updated = msgs["data"].as_array().unwrap().iter().find(|m| m["id"] == msg_id).unwrap();
    assert_eq!(
        updated["has_file_parts"], true,
        "has_file_parts must stay true when inline file_parts still present"
    );

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn unattached_file_can_be_downloaded_and_deleted() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    let session = create_session(&server, uid, true).await;
    let sid = session["session_id"].as_str().unwrap();

    // Upload but do NOT attach to any message.
    let (_, files) = upload_file(&server, uid, true, sid, "pending", "pending.txt").await;
    let file_id = files[0]["id"].as_str().unwrap();

    let dl = common::as_superuser(
        no_redirect_client().get(server.url(&format!("/api/chat/files/{file_id}/download"))),
        uid,
        "admin",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(dl.status(), 307, "unattached file should be downloadable");

    let del = common::as_superuser(
        server.client.delete(server.url(&format!("/api/chat/files/{file_id}"))),
        uid,
        "admin",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(del.status(), 204, "unattached file should be deletable");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn upload_to_another_users_session_rejected() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let admin_id = admin["user_id"].as_str().unwrap();

    let session = create_session(&server, admin_id, true).await;
    let sid = session["session_id"].as_str().unwrap();

    let alice = create_user(&server, admin_id, "alice-upload-idor").await;
    let alice_id = alice["id"].as_str().unwrap();

    let (status, _) = upload_file(&server, alice_id, false, sid, "evil", "evil.txt").await;
    assert_eq!(status, 404, "upload to another user's session must be rejected");

    server.cleanup().await;
}

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
use uuid::Uuid;

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
    let body = if is_super {
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
    .unwrap();
    // The handler wraps the session in a `{ data, status_code, message }`
    // envelope (session_response); return the inner session object.
    body["data"].clone()
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

    let rb = server
        .client
        .post(server.url(&format!("/api/chat/sessions/{session_id}/files")));
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
    let rb = server
        .client
        .post(server.url(&format!("/api/chat/sessions/{session_id}/messages")));
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

struct StorageDown;

#[async_trait::async_trait]
impl nasiko_runtime::BlobStore for StorageDown {
    async fn put_blob(
        &self,
        _: &str,
        _: bytes::Bytes,
    ) -> Result<i64, nasiko_runtime::BlobStoreError> {
        Err(down())
    }
    async fn get_blob(&self, _: &str) -> Result<bytes::Bytes, nasiko_runtime::BlobStoreError> {
        Err(down())
    }
    async fn delete_blob(&self, _: &str) -> Result<(), nasiko_runtime::BlobStoreError> {
        Err(down())
    }
    async fn blob_exists(&self, _: &str) -> bool {
        false
    }
    async fn blob_size(&self, _: &str) -> Result<i64, nasiko_runtime::BlobStoreError> {
        Err(down())
    }
    async fn presigned_get_url(
        &self,
        _: &str,
        _: u64,
    ) -> Result<String, nasiko_runtime::BlobStoreError> {
        Err(down())
    }
    async fn ensure_bucket(&self, _: bool) -> anyhow::Result<()> {
        Ok(())
    }
}

fn down() -> nasiko_runtime::BlobStoreError {
    nasiko_runtime::BlobStoreError::Backend("connection refused".into())
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[tokio::test]
#[serial]
async fn an_upload_while_storage_is_down_is_a_503_that_says_so_and_stores_nothing() {
    let server = common::TestServer::start_with_storage(std::sync::Arc::new(StorageDown)).await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();
    let session = create_session(&server, uid, true).await;
    let sid = session["session_id"].as_str().unwrap();
    let form = reqwest::multipart::Form::new()
        .part(
            "file",
            reqwest::multipart::Part::bytes(b"one".to_vec()).file_name("one.txt"),
        )
        .part(
            "file",
            reqwest::multipart::Part::bytes(b"two".to_vec()).file_name("two.txt"),
        );

    let res = common::as_superuser(
        server
            .client
            .post(server.url(&format!("/api/chat/sessions/{sid}/files"))),
        uid,
        "u",
    )
    .multipart(form)
    .send()
    .await
    .unwrap();

    assert_eq!(res.status().as_u16(), 503);
    assert_eq!(
        res.text().await.unwrap(),
        "file storage is unavailable; try again"
    );
    let rows: i64 =
        sqlx::query_scalar("SELECT count(*) FROM chat_message_files WHERE session_id = $1")
            .bind(sid)
            .fetch_one(&server.db)
            .await
            .unwrap();
    assert_eq!(rows, 0, "a failed upload leaves no file rows");

    server.cleanup().await;
}

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
        server
            .client
            .get(server.url(&format!("/api/chat/sessions/{sid}/messages/{msg_id}/files"))),
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

async fn uploaded_id(
    server: &common::TestServer,
    user_id: &str,
    session_id: &str,
    bytes: Vec<u8>,
    filename: &str,
    mime: &str,
) -> String {
    let (status, body) = upload_bytes(server, user_id, session_id, bytes, filename, mime).await;
    assert_eq!(status, 201, "{body}");
    let body: Value = serde_json::from_str(&body).unwrap();
    body[0]["id"].as_str().unwrap().to_string()
}

async fn download(server: &common::TestServer, user_id: &str, file_id: &str) -> reqwest::Response {
    common::as_superuser(
        no_redirect_client().get(server.url(&format!("/api/chat/files/{file_id}/download"))),
        user_id,
        "admin",
    )
    .send()
    .await
    .unwrap()
}

fn header<'a>(res: &'a reqwest::Response, name: &str) -> &'a str {
    res.headers()
        .get(name)
        .unwrap_or_else(|| panic!("missing {name}"))
        .to_str()
        .unwrap()
}

async fn seed_chat_file(
    server: &common::TestServer,
    session_id: &str,
    storage_uri: &str,
) -> String {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO chat_message_files (id, session_id, filename, mime_type, size_bytes, storage_uri) \
         VALUES ($1, $2, 'gone.png', 'image/png', 64, $3)",
    )
    .bind(id)
    .bind(session_id)
    .bind(storage_uri)
    .execute(&server.db)
    .await
    .unwrap();
    id.to_string()
}

#[tokio::test]
#[serial]
async fn a_download_is_served_by_the_server_not_redirected_to_storage() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();
    let session = create_session(&server, uid, true).await;
    let sid = session["session_id"].as_str().unwrap();

    let text = uploaded_id(
        &server,
        uid,
        sid,
        b"abc".to_vec(),
        "résumé notes.txt",
        "text/plain",
    )
    .await;
    let res = download(&server, uid, &text).await;
    assert_eq!(res.status(), 200);
    assert_eq!(header(&res, "content-type"), "text/plain");
    assert_eq!(header(&res, "x-content-type-options"), "nosniff");
    assert_eq!(
        header(&res, "content-disposition"),
        "attachment; filename*=UTF-8''r%C3%A9sum%C3%A9%20notes.txt"
    );
    assert_eq!(res.bytes().await.unwrap().as_ref(), b"abc");

    let png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR-preview".to_vec();
    let image = uploaded_id(&server, uid, sid, png.clone(), "chart.png", "image/png").await;
    let res = download(&server, uid, &image).await;
    assert_eq!(res.status(), 200);
    assert_eq!(header(&res, "content-type"), "image/png");
    assert_eq!(
        header(&res, "content-disposition"),
        "inline; filename*=UTF-8''chart.png",
        "a raster image previews in the browser"
    );
    assert_eq!(res.bytes().await.unwrap().as_ref(), png.as_slice());

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_downloaded_html_or_svg_file_is_never_rendered_inline() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();
    let session = create_session(&server, uid, true).await;
    let sid = session["session_id"].as_str().unwrap();

    for (bytes, name, mime) in [
        (
            b"<script>alert(1)</script>".to_vec(),
            "page.html",
            "text/html",
        ),
        (
            b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>".to_vec(),
            "logo.svg",
            "image/svg+xml",
        ),
    ] {
        let id = uploaded_id(&server, uid, sid, bytes, name, mime).await;
        let res = download(&server, uid, &id).await;
        assert_eq!(res.status(), 200, "{name}");
        assert!(
            header(&res, "content-disposition").starts_with("attachment;"),
            "{name} must download, not render on our origin"
        );
        assert_eq!(header(&res, "x-content-type-options"), "nosniff", "{name}");
    }

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_download_whose_bytes_are_gone_is_410() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();
    let session = create_session(&server, uid, true).await;
    let sid = session["session_id"].as_str().unwrap();
    let id = seed_chat_file(
        &server,
        sid,
        &format!("chat-files/{sid}/{}", Uuid::new_v4()),
    )
    .await;

    assert_eq!(download(&server, uid, &id).await.status(), 410);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_download_while_storage_is_down_is_a_503_that_names_no_backend() {
    let server = common::TestServer::start_with_storage(std::sync::Arc::new(StorageDown)).await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();
    let session = create_session(&server, uid, true).await;
    let sid = session["session_id"].as_str().unwrap();
    let id = seed_chat_file(
        &server,
        sid,
        &format!("chat-files/{sid}/{}", Uuid::new_v4()),
    )
    .await;

    let res = download(&server, uid, &id).await;
    assert_eq!(res.status(), 503);
    let body = res.text().await.unwrap();
    assert!(!body.contains("connection refused"), "{body}");

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
        server
            .client
            .delete(server.url(&format!("/api/chat/files/{file_id}"))),
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
        server
            .client
            .delete(server.url(&format!("/api/chat/sessions/{sid}"))),
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
        server
            .client
            .post(server.url(&format!("/api/chat/sessions/{sid}/files"))),
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

async fn upload_bytes(
    server: &common::TestServer,
    user_id: &str,
    session_id: &str,
    bytes: Vec<u8>,
    filename: &str,
    mime: &str,
) -> (u16, String) {
    let part = reqwest::multipart::Part::bytes(bytes)
        .file_name(filename.to_string())
        .mime_str(mime)
        .unwrap();
    let res = common::as_superuser(
        server
            .client
            .post(server.url(&format!("/api/chat/sessions/{session_id}/files"))),
        user_id,
        "admin",
    )
    .multipart(reqwest::multipart::Form::new().part("file", part))
    .send()
    .await
    .unwrap();
    (res.status().as_u16(), res.text().await.unwrap_or_default())
}

fn png_of(len: usize) -> Vec<u8> {
    let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
    bytes.resize(len, 0);
    bytes
}

#[tokio::test]
#[serial]
async fn an_upload_is_typed_and_limited_by_its_own_bytes() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();
    let session = create_session(&server, uid, true).await;
    let sid = session["session_id"].as_str().unwrap();
    let mib = 1024 * 1024;

    let stored_mime = |body: &str| -> String {
        serde_json::from_str::<Value>(body).unwrap()[0]["mime_type"]
            .as_str()
            .unwrap()
            .to_string()
    };

    let (status, body) = upload_bytes(
        &server,
        uid,
        sid,
        png_of(64),
        "photo.pdf",
        "application/pdf",
    )
    .await;
    assert_eq!(status, 201, "{body}");
    assert_eq!(stored_mime(&body), "image/png");

    let html = b"<html><script>alert(1)</script></html>".to_vec();
    let (status, body) = upload_bytes(&server, uid, sid, html, "x.png", "image/png").await;
    assert_eq!(status, 201, "{body}");
    assert_eq!(stored_mime(&body), "application/octet-stream");

    let (status, body) = upload_bytes(
        &server,
        uid,
        sid,
        b"# notes\n".to_vec(),
        "notes.md",
        "application/octet-stream",
    )
    .await;
    assert_eq!(status, 201, "{body}");
    assert_eq!(stored_mime(&body), "text/markdown");

    let (status, body) =
        upload_bytes(&server, uid, sid, png_of(2 * mib), "big.png", "image/png").await;
    assert_eq!(status, 201, "a 2 MB image is under the image limit: {body}");

    let (status, body) = upload_bytes(
        &server,
        uid,
        sid,
        vec![b'a'; mib + mib / 2],
        "big.txt",
        "text/plain",
    )
    .await;
    assert_eq!(status, 413);
    assert_eq!(body, "file 'big.txt' is over the 1 MB limit for text files");

    let (status, body) = upload_bytes(
        &server,
        uid,
        sid,
        vec![0; 11 * mib],
        "blob.bin",
        "application/octet-stream",
    )
    .await;
    assert_eq!(status, 413);
    assert_eq!(
        body,
        "file 'blob.bin' is over the 10 MB limit for other files"
    );

    let (status, body) =
        upload_bytes(&server, uid, sid, png_of(26 * mib), "huge.png", "image/png").await;
    assert_eq!(status, 413);
    assert_eq!(
        body,
        "file 'huge.png' is over the 25 MB limit for image files"
    );

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
        server
            .client
            .get(server.url(&format!("/api/chat/sessions/{sid}/messages/{msg_id}/files"))),
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
        server
            .client
            .delete(server.url(&format!("/api/chat/files/{file_id}"))),
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
        &server,
        admin_id,
        true,
        sid,
        vec![files[0]["id"].as_str().unwrap()],
    )
    .await;
    let msg_id = msg["id"].as_str().unwrap();

    let alice = create_user(&server, admin_id, "alice-list").await;
    let alice_id = alice["id"].as_str().unwrap();

    let res = common::as_member(
        server
            .client
            .get(server.url(&format!("/api/chat/sessions/{sid}/messages/{msg_id}/files"))),
        alice_id,
        "alice-list",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(
        res.status(),
        404,
        "IDOR: other user must not list message files"
    );

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
    assert_eq!(
        msg["has_file_parts"], false,
        "empty file_ids → has_file_parts false"
    );

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
        server
            .client
            .delete(server.url(&format!("/api/chat/files/{file_id}"))),
        uid,
        "admin",
    )
    .send()
    .await
    .unwrap();

    let msgs: Value = common::as_superuser(
        server
            .client
            .get(server.url(&format!("/api/chat/sessions/{sid}/messages"))),
        uid,
        "admin",
    )
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    let updated = msgs["data"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["id"] == msg_id)
        .unwrap();
    assert_eq!(
        updated["has_file_parts"], false,
        "has_file_parts cleared after last file deleted"
    );

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
        server
            .client
            .post(server.url(&format!("/api/chat/sessions/{sid}/messages"))),
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
        server
            .client
            .delete(server.url(&format!("/api/chat/files/{file_id}"))),
        uid,
        "admin",
    )
    .send()
    .await
    .unwrap();

    let msgs: Value = common::as_superuser(
        server
            .client
            .get(server.url(&format!("/api/chat/sessions/{sid}/messages"))),
        uid,
        "admin",
    )
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    let updated = msgs["data"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["id"] == msg_id)
        .unwrap();
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
    assert_eq!(dl.status(), 200, "unattached file should be downloadable");

    let del = common::as_superuser(
        server
            .client
            .delete(server.url(&format!("/api/chat/files/{file_id}"))),
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
async fn duplicate_file_ids_in_send_message_do_not_cause_spurious_400() {
    // Regression for SEC fix #4: a client sending the same file_id twice
    // previously caused `claimed != file_ids.len()` to false-positive (the
    // UPDATE only affects each distinct row once, so `rows_affected()` never
    // reaches the inflated length of a list containing a duplicate),
    // producing a spurious 400 on an otherwise-valid single-file attach.
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    let session = create_session(&server, uid, true).await;
    let sid = session["session_id"].as_str().unwrap();

    let (_, files) = upload_file(&server, uid, true, sid, "dup", "dup.txt").await;
    let file_id = files[0]["id"].as_str().unwrap();

    // Same file_id listed three times.
    let (status, msg) =
        send_message_with_files(&server, uid, true, sid, vec![file_id, file_id, file_id]).await;
    assert_eq!(
        status, 201,
        "duplicate file_ids must not cause a spurious 400: {msg}"
    );
    assert_eq!(msg["has_file_parts"], true);

    // Exactly one file attached — not zero, not three.
    let msg_id = msg["id"].as_str().unwrap();
    let res = common::as_superuser(
        server
            .client
            .get(server.url(&format!("/api/chat/sessions/{sid}/messages/{msg_id}/files"))),
        uid,
        "admin",
    )
    .send()
    .await
    .unwrap();
    let listed: Value = res.json().await.unwrap();
    assert_eq!(
        listed.as_array().unwrap().len(),
        1,
        "file attached exactly once despite duplicates"
    );

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
    assert_eq!(
        status, 404,
        "upload to another user's session must be rejected"
    );

    server.cleanup().await;
}

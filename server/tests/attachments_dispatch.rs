mod common;

use std::sync::{Arc, Mutex};

use axum::{Router, extract::State, routing::post};
use base64::Engine;
use serde_json::{Value, json};
use serial_test::serial;
use uuid::Uuid;

#[derive(Clone, Default)]
struct StubAgent {
    seen: Arc<Mutex<Vec<Value>>>,
    traceparents: Arc<Mutex<Vec<String>>>,
}

async fn stub_agent_handler(
    State(agent): State<StubAgent>,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> axum::response::Response {
    if let Some(traceparent) = headers.get("traceparent").and_then(|v| v.to_str().ok()) {
        agent
            .traceparents
            .lock()
            .unwrap()
            .push(traceparent.to_string());
    }
    let req: Value = serde_json::from_slice(&body).unwrap_or_default();
    let task_id = req["params"]["message"]["taskId"]
        .as_str()
        .unwrap_or("stub-task")
        .to_string();
    let context_id = req["params"]["message"]["contextId"]
        .as_str()
        .unwrap_or("stub-ctx")
        .to_string();
    agent.seen.lock().unwrap().push(req);
    let event =
        nasiko_types::a2a::status_event(nasiko_types::a2a::completed(&task_id, &context_id));
    axum::response::Response::builder()
        .status(200)
        .header("content-type", "text/event-stream")
        .body(axum::body::Body::from(format!(
            "data: {}\n\n",
            serde_json::to_string(&event).unwrap()
        )))
        .unwrap()
}

async fn start_stub_agent() -> (String, StubAgent, tokio::task::JoinHandle<()>) {
    let agent = StubAgent::default();
    let app = Router::new()
        .route("/", post(stub_agent_handler))
        .with_state(agent.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://{addr}"), agent, handle)
}

struct User {
    id: String,
    name: String,
}

async fn admin_and_member(server: &common::TestServer, name: &str) -> (String, User) {
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
    let admin_id = admin["user_id"].as_str().unwrap().to_string();
    let user: Value = common::as_superuser(
        server.client.post(server.url("/api/users")),
        &admin_id,
        "admin",
    )
    .json(&json!({"username": name, "email": format!("{name}@test.local")}))
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    let member = User {
        id: user["id"].as_str().unwrap().to_string(),
        name: name.to_string(),
    };
    (admin_id, member)
}

async fn create_session(server: &common::TestServer, user: &User) -> String {
    let body: Value = common::as_member(
        server.client.post(server.url("/api/chat/sessions")),
        &user.id,
        &user.name,
    )
    .json(&json!({"title": "attachments"}))
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    body["data"]["session_id"].as_str().unwrap().to_string()
}

async fn upload(
    server: &common::TestServer,
    user: &User,
    session_id: &str,
    bytes: Vec<u8>,
    filename: &str,
    mime: &str,
) -> String {
    let part = reqwest::multipart::Part::bytes(bytes)
        .file_name(filename.to_string())
        .mime_str(mime)
        .unwrap();
    let res = common::as_member(
        server
            .client
            .post(server.url(&format!("/api/chat/sessions/{session_id}/files"))),
        &user.id,
        &user.name,
    )
    .multipart(reqwest::multipart::Form::new().part("file", part))
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 201);
    let body: Value = res.json().await.unwrap();
    body[0]["id"].as_str().unwrap().to_string()
}

async fn seed_running_agent(server: &common::TestServer, owner: &User, url: &str) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO agents (name, owner_id, url, status) VALUES ($1, $2, $3, 'running') RETURNING id",
    )
    .bind(format!("attachments-agent-{}", Uuid::new_v4()))
    .bind(Uuid::parse_str(&owner.id).unwrap())
    .bind(url)
    .fetch_one(&server.db)
    .await
    .unwrap()
}

async fn seed_file_row(
    server: &common::TestServer,
    session_id: &str,
    filename: &str,
    storage_uri: &str,
) -> String {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO chat_message_files (id, session_id, filename, mime_type, size_bytes, storage_uri) \
         VALUES ($1, $2, $3, 'image/png', 64, $4)",
    )
    .bind(id)
    .bind(session_id)
    .bind(filename)
    .bind(storage_uri)
    .execute(&server.db)
    .await
    .unwrap();
    id.to_string()
}

async fn send_turn(
    server: &common::TestServer,
    user: &User,
    agent_id: Option<Uuid>,
    session_id: &str,
    file_ids: &[String],
    extra_parts: Vec<Value>,
) -> (u16, String) {
    let mut parts = vec![json!({"text": "what is in the attachment?"})];
    parts.extend(extra_parts);
    let mut metadata = json!({"session_id": session_id, "file_ids": file_ids});
    if let Some(agent_id) = agent_id {
        metadata["agent_id"] = json!(agent_id.to_string());
    }
    let body = json!({
        "jsonrpc": "2.0",
        "id": Uuid::new_v4().to_string(),
        "method": "message/stream",
        "params": {
            "message": {
                "messageId": Uuid::new_v4().to_string(),
                "contextId": Uuid::new_v4().to_string(),
                "role": "ROLE_USER",
                "parts": parts,
            },
            "metadata": metadata,
        },
    });
    let res = common::as_member(
        server.client.post(server.url("/api/orchestrator/a2a")),
        &user.id,
        &user.name,
    )
    .json(&body)
    .send()
    .await
    .unwrap();
    (res.status().as_u16(), res.text().await.unwrap_or_default())
}

fn error_message(body: &str) -> String {
    serde_json::from_str::<Value>(body).unwrap()["error"]["message"]
        .as_str()
        .unwrap()
        .to_string()
}

fn png_of(len: usize) -> Vec<u8> {
    let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
    bytes.resize(len, 7);
    bytes
}

fn raw_part(bytes: &[u8]) -> Value {
    json!({
        "raw": base64::engine::general_purpose::STANDARD.encode(bytes),
        "filename": "inline.bin",
        "mediaType": "application/octet-stream",
    })
}

fn raw_parts_seen(agent: &StubAgent) -> Vec<Value> {
    let seen = agent.seen.lock().unwrap();
    assert_eq!(seen.len(), 1, "the agent should be called once");
    seen[0]["params"]["message"]["parts"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|p| p.get("raw").is_some())
        .cloned()
        .collect()
}

#[tokio::test]
#[serial]
async fn more_than_ten_attachments_are_refused_counting_inline_parts() {
    let server = common::TestServer::start().await;
    let (_, user) = admin_and_member(&server, "att-count").await;
    let sid = create_session(&server, &user).await;

    let eleven: Vec<String> = (0..11).map(|_| Uuid::new_v4().to_string()).collect();
    let (status, body) = send_turn(&server, &user, None, &sid, &eleven, vec![]).await;
    assert_eq!(status, 400, "{body}");
    assert_eq!(error_message(&body), "at most 10 attachments per message");

    let nine: Vec<String> = (0..9).map(|_| Uuid::new_v4().to_string()).collect();
    let (status, body) = send_turn(
        &server,
        &user,
        None,
        &sid,
        &nine,
        vec![raw_part(b"a"), raw_part(b"b")],
    )
    .await;
    assert_eq!(status, 400, "{body}");
    assert_eq!(error_message(&body), "at most 10 attachments per message");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn the_same_file_listed_twice_reaches_the_agent_once() {
    let server = common::TestServer::start().await;
    let (_, user) = admin_and_member(&server, "att-dupe").await;
    let sid = create_session(&server, &user).await;
    let (url, agent, _handle) = start_stub_agent().await;
    let agent_id = seed_running_agent(&server, &user, &url).await;
    let file = upload(&server, &user, &sid, png_of(64), "fact.png", "image/png").await;

    let ids: Vec<String> = std::iter::repeat_n(file, 11).collect();
    let (status, body) = send_turn(&server, &user, Some(agent_id), &sid, &ids, vec![]).await;
    assert_eq!(status, 200, "{body}");
    let raws = raw_parts_seen(&agent);
    assert_eq!(raws.len(), 1);
    assert_eq!(raws[0]["filename"], "fact.png");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn an_unknown_or_someone_elses_file_is_refused_the_same_way() {
    let server = common::TestServer::start().await;
    let (admin_id, user) = admin_and_member(&server, "att-owner").await;
    let sid = create_session(&server, &user).await;
    let other: Value = common::as_superuser(
        server.client.post(server.url("/api/users")),
        &admin_id,
        "admin",
    )
    .json(&json!({"username": "att-other", "email": "att-other@test.local"}))
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    let other = User {
        id: other["id"].as_str().unwrap().to_string(),
        name: "att-other".into(),
    };
    let other_sid = create_session(&server, &other).await;
    let foreign = upload(
        &server,
        &other,
        &other_sid,
        png_of(64),
        "theirs.png",
        "image/png",
    )
    .await;
    let unknown = Uuid::new_v4().to_string();

    for id in [unknown, foreign] {
        let (status, body) = send_turn(
            &server,
            &user,
            None,
            &sid,
            std::slice::from_ref(&id),
            vec![],
        )
        .await;
        assert_eq!(status, 400, "{body}");
        assert_eq!(
            error_message(&body),
            format!("attachment {id} was not found")
        );
    }
    let (status, body) = send_turn(
        &server,
        &user,
        None,
        &sid,
        &["not-a-uuid".to_string()],
        vec![],
    )
    .await;
    assert_eq!(status, 400, "{body}");
    assert_eq!(error_message(&body), "attachment not-a-uuid was not found");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn files_over_thirty_megabytes_together_are_refused() {
    let server = common::TestServer::start().await;
    let (_, user) = admin_and_member(&server, "att-size").await;
    let sid = create_session(&server, &user).await;
    let mib = 1024 * 1024;
    let first = upload(&server, &user, &sid, png_of(16 * mib), "a.png", "image/png").await;
    let second = upload(&server, &user, &sid, png_of(16 * mib), "b.png", "image/png").await;

    let (status, body) = send_turn(&server, &user, None, &sid, &[first, second], vec![]).await;
    assert_eq!(status, 400, "{body}");
    assert_eq!(
        error_message(&body),
        "attachments total 32.0 MB, over the 30 MB per-message limit"
    );

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_file_whose_stored_bytes_are_gone_is_named() {
    let server = common::TestServer::start().await;
    let (_, user) = admin_and_member(&server, "att-gone").await;
    let sid = create_session(&server, &user).await;
    let id = seed_file_row(
        &server,
        &sid,
        "gone.png",
        &format!("chat-files/{sid}/{}", Uuid::new_v4()),
    )
    .await;

    let (status, body) = send_turn(&server, &user, None, &sid, &[id], vec![]).await;
    assert_eq!(status, 400, "{body}");
    assert_eq!(
        error_message(&body),
        "attachment 'gone.png' is no longer stored"
    );

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_workspace_file_row_cannot_be_attached() {
    let server = common::TestServer::start().await;
    let (_, user) = admin_and_member(&server, "att-ws").await;
    let sid = create_session(&server, &user).await;
    let id = seed_file_row(&server, &sid, "chart.png", "workspace://o/a/chart.png").await;

    let (status, body) = send_turn(&server, &user, None, &sid, &[id], vec![]).await;
    assert_eq!(status, 400, "{body}");
    assert_eq!(
        error_message(&body),
        "'chart.png' can't be attached to a message"
    );

    server.cleanup().await;
}

struct ReadsFail;

#[async_trait::async_trait]
impl nasiko_runtime::BlobStore for ReadsFail {
    async fn put_blob(
        &self,
        _: &str,
        data: bytes::Bytes,
    ) -> Result<i64, nasiko_runtime::BlobStoreError> {
        Ok(data.len() as i64)
    }
    async fn get_blob(&self, _: &str) -> Result<bytes::Bytes, nasiko_runtime::BlobStoreError> {
        Err(nasiko_runtime::BlobStoreError::Backend(
            "connection refused".into(),
        ))
    }
    async fn delete_blob(&self, _: &str) -> Result<(), nasiko_runtime::BlobStoreError> {
        Ok(())
    }
    async fn blob_exists(&self, _: &str) -> bool {
        false
    }
    async fn blob_size(&self, _: &str) -> Result<i64, nasiko_runtime::BlobStoreError> {
        Ok(0)
    }
    async fn presigned_get_url(
        &self,
        _: &str,
        _: u64,
    ) -> Result<String, nasiko_runtime::BlobStoreError> {
        Ok(String::new())
    }
    async fn ensure_bucket(&self, _: bool) -> anyhow::Result<()> {
        Ok(())
    }
}

#[tokio::test]
#[serial]
async fn unreadable_storage_is_a_503_naming_the_file() {
    let server = common::TestServer::start_with_storage(Arc::new(ReadsFail)).await;
    let (_, user) = admin_and_member(&server, "att-down").await;
    let sid = create_session(&server, &user).await;
    let id = upload(&server, &user, &sid, png_of(64), "fact.png", "image/png").await;

    let (status, body) = send_turn(&server, &user, None, &sid, &[id], vec![]).await;
    assert_eq!(status, 503, "{body}");
    assert_eq!(
        error_message(&body),
        "attachment 'fact.png' could not be read; try again"
    );
    assert!(!body.contains("connection refused"), "{body}");

    server.cleanup().await;
}

#[derive(Default)]
struct SlowStore {
    blobs: Mutex<Vec<(String, bytes::Bytes)>>,
    in_flight: std::sync::atomic::AtomicUsize,
    most_in_flight: std::sync::atomic::AtomicUsize,
}

#[async_trait::async_trait]
impl nasiko_runtime::BlobStore for SlowStore {
    async fn put_blob(
        &self,
        key: &str,
        data: bytes::Bytes,
    ) -> Result<i64, nasiko_runtime::BlobStoreError> {
        let len = data.len() as i64;
        self.blobs.lock().unwrap().push((key.to_string(), data));
        Ok(len)
    }
    async fn get_blob(&self, key: &str) -> Result<bytes::Bytes, nasiko_runtime::BlobStoreError> {
        use std::sync::atomic::Ordering::SeqCst;
        let (position, data, count) = {
            let blobs = self.blobs.lock().unwrap();
            let position = blobs.iter().position(|(k, _)| k == key).unwrap();
            (position, blobs[position].1.clone(), blobs.len())
        };
        let now = self.in_flight.fetch_add(1, SeqCst) + 1;
        self.most_in_flight.fetch_max(now, SeqCst);
        let slower_first = (count - position) as u64 * 150;
        tokio::time::sleep(std::time::Duration::from_millis(slower_first)).await;
        self.in_flight.fetch_sub(1, SeqCst);
        Ok(data)
    }
    async fn delete_blob(&self, _: &str) -> Result<(), nasiko_runtime::BlobStoreError> {
        Ok(())
    }
    async fn blob_exists(&self, _: &str) -> bool {
        true
    }
    async fn blob_size(&self, _: &str) -> Result<i64, nasiko_runtime::BlobStoreError> {
        Ok(0)
    }
    async fn presigned_get_url(
        &self,
        _: &str,
        _: u64,
    ) -> Result<String, nasiko_runtime::BlobStoreError> {
        Ok(String::new())
    }
    async fn ensure_bucket(&self, _: bool) -> anyhow::Result<()> {
        Ok(())
    }
}

#[tokio::test]
#[serial]
async fn attached_files_are_read_together_and_reach_the_agent_in_order() {
    let store = Arc::new(SlowStore::default());
    let server = common::TestServer::start_with_storage(store.clone()).await;
    let (_, user) = admin_and_member(&server, "att-parallel").await;
    let sid = create_session(&server, &user).await;
    let (url, agent, _handle) = start_stub_agent().await;
    let agent_id = seed_running_agent(&server, &user, &url).await;
    let names = [
        "one.png",
        "two.png",
        "three.png",
        "four.png",
        "five.png",
        "six.png",
    ];
    let mut ids = Vec::new();
    for (n, name) in names.iter().enumerate() {
        ids.push(upload(&server, &user, &sid, png_of(64 + n), name, "image/png").await);
    }

    let (status, body) = send_turn(&server, &user, Some(agent_id), &sid, &ids, vec![]).await;
    assert_eq!(status, 200, "{body}");
    let most = store
        .most_in_flight
        .load(std::sync::atomic::Ordering::SeqCst);
    assert!((2..=4).contains(&most), "reads in flight at once: {most}");
    let raws = raw_parts_seen(&agent);
    let seen: Vec<&str> = raws
        .iter()
        .map(|p| p["filename"].as_str().unwrap())
        .collect();
    assert_eq!(seen, names);
    for (n, part) in raws.iter().enumerate() {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(part["raw"].as_str().unwrap())
            .unwrap();
        assert_eq!(bytes, png_of(64 + n), "{}", names[n]);
    }

    server.cleanup().await;
}

const JWT_SECRET: &str = "attachments-dispatch-secret";

struct ChipSource;

fn chip_reference() -> nasiko_llm_router::attachments::AttachmentRef {
    nasiko_llm_router::attachments::AttachmentRef::Workspace {
        agent_id: Uuid::from_u128(1),
        user_id: Uuid::from_u128(2),
        blob_id: Uuid::from_u128(3),
    }
}

#[async_trait::async_trait]
impl nasiko_server::chat::turn_artifacts::TurnArtifactSource for ChipSource {
    async fn capture(&self, _: &sqlx::PgPool, _: &str, _: Uuid, _: Option<&str>) -> Vec<Value> {
        Vec::new()
    }

    async fn resolve_download(
        &self,
        _: &sqlx::PgPool,
        _: Uuid,
        _: &nasiko_server::chat::turn_artifacts::ChatMessageFile,
    ) -> Option<nasiko_server::chat::turn_artifacts::DownloadTarget> {
        None
    }

    async fn attachment(
        &self,
        _: &sqlx::PgPool,
        _: Uuid,
        storage_uri: &str,
    ) -> nasiko_server::chat::turn_artifacts::Attachable {
        use nasiko_server::chat::turn_artifacts::{Attachable, AttachableFile};
        let file = |size_bytes| {
            Attachable::File(AttachableFile {
                reference: chip_reference(),
                media_type: "image/png".into(),
                size_bytes,
            })
        };
        match storage_uri {
            "workspace-file://ok" => file(1536),
            "workspace-file://huge" => file(31 * 1024 * 1024),
            "workspace-file://down" => Attachable::Unavailable,
            _ => Attachable::Refused,
        }
    }
}

#[tokio::test]
#[serial]
async fn a_workspace_chip_reaches_the_agent_as_a_signed_workspace_line() {
    let server = common::TestServer::start_with(|c| c.agent_jwt_secret = JWT_SECRET.into()).await;
    server.state.turn_artifacts.install(Arc::new(ChipSource));
    let (_, user) = admin_and_member(&server, "att-chip").await;
    let sid = create_session(&server, &user).await;
    let (url, agent, _handle) = start_stub_agent().await;
    let agent_id = seed_running_agent(&server, &user, &url).await;
    let png = upload(&server, &user, &sid, png_of(64), "fact.png", "image/png").await;
    let chip = seed_file_row(&server, &sid, "chart.png", "workspace-file://ok").await;

    let (status, body) =
        send_turn(&server, &user, Some(agent_id), &sid, &[chip, png], vec![]).await;
    assert_eq!(status, 200, "{body}");

    let texts = text_parts_seen(&agent);
    let lines = user_text_attachment_lines(&texts);
    assert_eq!(lines.len(), 2, "{texts:?}");
    assert!(
        lines[0].starts_with("[Attached file: chart.png (image/png, 1.5 KB) "),
        "{texts:?}"
    );
    assert!(
        lines[1].starts_with("[Attached file: fact.png (image/png, 64 B) "),
        "{texts:?}"
    );
    assert_eq!(raw_parts_seen(&agent).len(), 1);

    let signer = nasiko_llm_router::attachments::MarkerSigner::from_secret(JWT_SECRET).unwrap();
    let spans = nasiko_llm_router::attachments::token_spans(lines[0]);
    let claims = signer
        .verify(
            &lines[0][spans[0].token.clone()],
            &flow_id_seen(&agent),
            chrono::Utc::now().timestamp(),
        )
        .unwrap();
    assert_eq!(claims.reference, chip_reference());
    assert_eq!(claims.filename, "chart.png");
    assert_eq!(claims.size_bytes, 1536);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_chip_the_source_refuses_is_named_and_a_failing_source_is_a_503() {
    let server = common::TestServer::start_with(|c| c.agent_jwt_secret = JWT_SECRET.into()).await;
    server.state.turn_artifacts.install(Arc::new(ChipSource));
    let (_, user) = admin_and_member(&server, "att-chip-no").await;
    let sid = create_session(&server, &user).await;
    let (url, agent, _handle) = start_stub_agent().await;
    let agent_id = seed_running_agent(&server, &user, &url).await;

    let refused = seed_file_row(&server, &sid, "gone.png", "workspace-file://denied").await;
    let (status, body) = send_turn(&server, &user, Some(agent_id), &sid, &[refused], vec![]).await;
    assert_eq!(status, 400, "{body}");
    assert_eq!(
        error_message(&body),
        "'gone.png' can't be attached to a message"
    );

    let huge = seed_file_row(&server, &sid, "huge.png", "workspace-file://huge").await;
    let (status, body) = send_turn(&server, &user, Some(agent_id), &sid, &[huge], vec![]).await;
    assert_eq!(status, 400, "{body}");
    assert_eq!(
        error_message(&body),
        "attachments total 31.0 MB, over the 30 MB per-message limit"
    );

    let down = seed_file_row(&server, &sid, "chart.png", "workspace-file://down").await;
    let (status, body) = send_turn(&server, &user, Some(agent_id), &sid, &[down], vec![]).await;
    assert_eq!(status, 503, "{body}");
    assert_eq!(
        error_message(&body),
        "attachment 'chart.png' could not be read; try again"
    );
    assert!(agent.seen.lock().unwrap().is_empty());

    server.cleanup().await;
}

fn text_parts_seen(agent: &StubAgent) -> Vec<String> {
    let seen = agent.seen.lock().unwrap();
    assert_eq!(seen.len(), 1, "the agent should be called once");
    seen[0]["params"]["message"]["parts"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|p| p.get("text").and_then(Value::as_str).map(str::to_string))
        .collect()
}

fn user_text_attachment_lines(texts: &[String]) -> Vec<&str> {
    let token = nasiko_llm_router::attachments::TOKEN_PREFIX;
    assert!(
        texts[1..].iter().all(|t| !t.contains(token)),
        "an attachment line rode outside the user's text part: {texts:?}"
    );
    let mut lines = texts[0].lines();
    assert_eq!(
        lines.next(),
        Some("what is in the attachment?"),
        "{texts:?}"
    );
    lines.collect()
}

fn flow_id_seen(agent: &StubAgent) -> String {
    let traceparents = agent.traceparents.lock().unwrap();
    traceparents[0].split('-').nth(1).unwrap().to_string()
}

#[tokio::test]
#[serial]
async fn a_direct_turn_gives_the_agent_one_signed_line_per_file_for_this_flow() {
    let server = common::TestServer::start_with(|c| c.agent_jwt_secret = JWT_SECRET.into()).await;
    let (_, user) = admin_and_member(&server, "att-note").await;
    let sid = create_session(&server, &user).await;
    let (url, agent, _handle) = start_stub_agent().await;
    let agent_id = seed_running_agent(&server, &user, &url).await;
    let png = upload(&server, &user, &sid, png_of(64), "fact.png", "image/png").await;
    let pdf_bytes = b"%PDF-1.7\nfake".to_vec();
    let pdf_len = pdf_bytes.len() as u64;
    let pdf = upload(&server, &user, &sid, pdf_bytes, "q3.pdf", "application/pdf").await;

    let (status, body) = send_turn(
        &server,
        &user,
        Some(agent_id),
        &sid,
        &[png.clone(), pdf.clone()],
        vec![],
    )
    .await;
    assert_eq!(status, 200, "{body}");

    let texts = text_parts_seen(&agent);
    let lines = user_text_attachment_lines(&texts);
    assert_eq!(lines.len(), 2, "{texts:?}");
    assert!(lines[0].starts_with("[Attached file: fact.png (image/png, 64 B) "));
    assert!(lines[1].starts_with(&format!(
        "[Attached file: q3.pdf (application/pdf, {pdf_len} B) "
    )));
    assert_eq!(raw_parts_seen(&agent).len(), 2);

    let note = texts[0].as_str();
    let signer = nasiko_llm_router::attachments::MarkerSigner::from_secret(JWT_SECRET).unwrap();
    let flow_id = flow_id_seen(&agent);
    let now = chrono::Utc::now().timestamp();
    let spans = nasiko_llm_router::attachments::token_spans(note);
    assert_eq!(spans.len(), 2);
    let claims: Vec<_> = spans
        .iter()
        .map(|span| {
            signer
                .verify(&note[span.token.clone()], &flow_id, now)
                .unwrap()
        })
        .collect();
    assert_eq!(
        claims[0].reference,
        nasiko_llm_router::attachments::AttachmentRef::ChatUpload {
            session_id: sid.clone(),
            file_id: Uuid::parse_str(&png).unwrap(),
        }
    );
    assert_eq!(claims[1].media_type, "application/pdf");
    assert!(claims[0].expires_at > now && claims[0].expires_at <= now + 3600);
    assert!(
        signer
            .verify(&note[spans[0].token.clone()], "0".repeat(32).as_str(), now)
            .is_err()
    );

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn without_a_signing_key_the_agent_gets_no_attachment_line() {
    let server = common::TestServer::start().await;
    let (_, user) = admin_and_member(&server, "att-nokey").await;
    let sid = create_session(&server, &user).await;
    let (url, agent, _handle) = start_stub_agent().await;
    let agent_id = seed_running_agent(&server, &user, &url).await;
    let png = upload(&server, &user, &sid, png_of(64), "fact.png", "image/png").await;

    let (status, body) = send_turn(&server, &user, Some(agent_id), &sid, &[png], vec![]).await;
    assert_eq!(status, 200, "{body}");
    assert!(
        text_parts_seen(&agent)
            .iter()
            .all(|t| !t.contains("nasiko-file://"))
    );
    assert_eq!(raw_parts_seen(&agent).len(), 1);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_turn_without_files_gets_no_attachment_line() {
    let server = common::TestServer::start_with(|c| c.agent_jwt_secret = JWT_SECRET.into()).await;
    let (_, user) = admin_and_member(&server, "att-none").await;
    let sid = create_session(&server, &user).await;
    let (url, agent, _handle) = start_stub_agent().await;
    let agent_id = seed_running_agent(&server, &user, &url).await;

    let (status, body) = send_turn(&server, &user, Some(agent_id), &sid, &[], vec![]).await;
    assert_eq!(status, 200, "{body}");
    // Only the user's text, besides the current time the platform tells every agent.
    let texts = text_parts_seen(&agent);
    let own: Vec<&String> = texts
        .iter()
        .filter(|t| !t.starts_with(nasiko_types::agent_clock::NOTE_LEAD))
        .collect();
    assert_eq!(own.len(), 1, "{texts:?}");
    assert!(!own[0].contains("nasiko-file://"), "{texts:?}");

    server.cleanup().await;
}

fn streamed_tool_call(tool_name: &str, message: &str) -> String {
    let chunk = json!({
        "id": "chatcmpl-1",
        "model": "gpt-4o",
        "choices": [{
            "index": 0,
            "delta": {
                "tool_calls": [{
                    "index": 0,
                    "id": "call_1",
                    "type": "function",
                    "function": {
                        "name": tool_name,
                        "arguments": json!({ "message": message }).to_string()
                    }
                }]
            }
        }]
    });
    format!("data: {chunk}\n\n")
}

#[tokio::test]
#[serial]
async fn a_routed_turn_gives_the_chosen_agent_the_signed_line_for_its_flow() {
    let server = common::TestServer::start_with(|c| c.agent_jwt_secret = JWT_SECRET.into()).await;
    let (_, user) = admin_and_member(&server, "att-routed").await;
    let sid = create_session(&server, &user).await;
    let (url, agent, _handle) = start_stub_agent().await;
    let agent_name = "att-routed-agent";
    sqlx::query("INSERT INTO agents (name, owner_id, url, status) VALUES ($1, $2, $3, 'running')")
        .bind(agent_name)
        .bind(Uuid::parse_str(&user.id).unwrap())
        .bind(&url)
        .execute(&server.db)
        .await
        .unwrap();
    let png = upload(&server, &user, &sid, png_of(64), "fact.png", "image/png").await;

    let mut llm = mockito::Server::new_async().await;
    let llm_mock = llm
        .mock("POST", "/chat/completions")
        .expect_at_least(1)
        .with_status(200)
        .with_body(streamed_tool_call(
            &format!("call_agent_{}", agent_name.replace('-', "_")),
            "describe the picture",
        ))
        .create_async()
        .await;
    unsafe {
        std::env::set_var("OPENAI_BASE_URL", llm.url());
        std::env::set_var("OPENAI_API_KEY", "test-key-for-attachments");
    }

    let (status, body) = send_turn(
        &server,
        &user,
        None,
        &sid,
        std::slice::from_ref(&png),
        vec![],
    )
    .await;
    unsafe {
        std::env::remove_var("OPENAI_BASE_URL");
        std::env::remove_var("OPENAI_API_KEY");
    }
    assert_eq!(status, 200, "{body}");
    assert!(
        llm_mock.matched_async().await,
        "the orchestrator model was never called"
    );

    let first_call = agent
        .seen
        .lock()
        .unwrap()
        .first()
        .cloned()
        .unwrap_or_else(|| panic!("the routed agent was never called: {body}"));
    let texts: Vec<&str> = first_call["params"]["message"]["parts"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|p| p.get("text").and_then(Value::as_str))
        .collect();
    let note = texts[0];
    assert!(
        note.starts_with("describe the picture\n[Attached file: fact.png (image/png, 64 B) "),
        "{texts:?}"
    );
    assert!(
        texts[1..]
            .iter()
            .all(|t| !t.contains(nasiko_llm_router::attachments::TOKEN_PREFIX)),
        "an attachment line rode outside the routed message text: {texts:?}"
    );
    let spans = nasiko_llm_router::attachments::token_spans(note);
    assert_eq!(spans.len(), 1);
    let signer = nasiko_llm_router::attachments::MarkerSigner::from_secret(JWT_SECRET).unwrap();
    let claims = signer
        .verify(
            &note[spans[0].token.clone()],
            &flow_id_seen(&agent),
            chrono::Utc::now().timestamp(),
        )
        .unwrap();
    assert_eq!(
        claims.reference,
        nasiko_llm_router::attachments::AttachmentRef::ChatUpload {
            session_id: sid.clone(),
            file_id: Uuid::parse_str(&png).unwrap(),
        }
    );
    let files: Vec<&Value> = first_call["params"]["message"]["parts"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|p| p.get("file").is_some())
        .collect();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0]["file"]["name"], "fact.png");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn the_orchestrator_model_is_told_which_files_the_user_attached() {
    let server = common::TestServer::start_with(|c| c.agent_jwt_secret = JWT_SECRET.into()).await;
    let (_, user) = admin_and_member(&server, "att-listed").await;
    let sid = create_session(&server, &user).await;
    let (url, _agent, _handle) = start_stub_agent().await;
    let agent_name = "att-listed-agent";
    sqlx::query("INSERT INTO agents (name, owner_id, url, status) VALUES ($1, $2, $3, 'running')")
        .bind(agent_name)
        .bind(Uuid::parse_str(&user.id).unwrap())
        .bind(&url)
        .execute(&server.db)
        .await
        .unwrap();
    let png = upload(&server, &user, &sid, png_of(64), "fact.png", "image/png").await;

    let mut llm = mockito::Server::new_async().await;
    let listed = llm
        .mock("POST", "/chat/completions")
        .match_body(mockito::Matcher::Regex(
            r"\[The user attached: fact\.png \(image/png, 64 B\)\]".into(),
        ))
        .expect_at_least(1)
        .with_status(200)
        .with_body(streamed_tool_call(
            &format!("call_agent_{}", agent_name.replace('-', "_")),
            "describe the picture",
        ))
        .create_async()
        .await;
    unsafe {
        std::env::set_var("OPENAI_BASE_URL", llm.url());
        std::env::set_var("OPENAI_API_KEY", "test-key-for-attachments");
    }

    let (status, body) = send_turn(&server, &user, None, &sid, &[png], vec![]).await;
    unsafe {
        std::env::remove_var("OPENAI_BASE_URL");
        std::env::remove_var("OPENAI_API_KEY");
    }
    assert_eq!(status, 200, "{body}");
    assert!(
        listed.matched_async().await,
        "the orchestrator model never saw the attached file list: {body}"
    );
    assert!(
        !body.contains(nasiko_llm_router::attachments::TOKEN_PREFIX),
        "the orchestrator's input carries no signed token"
    );

    server.cleanup().await;
}

#[derive(Clone)]
struct TextOnlyAgent {
    router_url: String,
    token: Arc<Mutex<String>>,
    router_replies: Arc<Mutex<Vec<(u16, String)>>>,
}

async fn text_only_agent_handler(
    State(agent): State<TextOnlyAgent>,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> axum::response::Response {
    let req: Value = serde_json::from_slice(&body).unwrap_or_default();
    let user_input = req["params"]["message"]["parts"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|p| p.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("\n");
    let traceparent = headers
        .get("traceparent")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let token = agent.token.lock().unwrap().clone();
    let reply = reqwest::Client::new()
        .post(format!("{}/v1/chat/completions", agent.router_url))
        .bearer_auth(token)
        .header("traceparent", traceparent)
        .json(&json!({
            "model": "gpt-4o-mini",
            "messages": [{"role": "user", "content": user_input}],
        }))
        .send()
        .await
        .unwrap();
    let status = reply.status().as_u16();
    let text = reply.text().await.unwrap_or_default();
    agent.router_replies.lock().unwrap().push((status, text));

    let task_id = req["params"]["message"]["taskId"]
        .as_str()
        .unwrap_or("stub-task")
        .to_string();
    let context_id = req["params"]["message"]["contextId"]
        .as_str()
        .unwrap_or("stub-ctx")
        .to_string();
    let event =
        nasiko_types::a2a::status_event(nasiko_types::a2a::completed(&task_id, &context_id));
    axum::response::Response::builder()
        .status(200)
        .header("content-type", "text/event-stream")
        .body(axum::body::Body::from(format!(
            "data: {}\n\n",
            serde_json::to_string(&event).unwrap()
        )))
        .unwrap()
}

const E2E_ENV: [&str; 5] = [
    "AGENT_JWT_SECRET",
    "OPENAI_API_BASE",
    "PLATFORM_OPENAI_API_KEY",
    "DEFAULT_PROVIDER",
    "DEFAULT_MODEL",
];

#[tokio::test]
#[serial]
async fn an_agent_with_no_media_code_gets_the_image_to_its_model_through_the_router() {
    let mut provider = mockito::Server::new_async().await;
    let seen_by_provider = Arc::new(Mutex::new(Vec::<Value>::new()));
    let capture = Arc::clone(&seen_by_provider);
    let _provider_mock = provider
        .mock("POST", "/v1/chat/completions")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body_from_request(move |request| {
            let body = request.body().map(Vec::as_slice).unwrap_or_default();
            capture
                .lock()
                .unwrap()
                .push(serde_json::from_slice(body).unwrap_or(Value::Null));
            json!({
                "id": "chatcmpl-1", "object": "chat.completion", "model": "gpt-4o-mini",
                "choices": [{"index": 0, "message": {"role": "assistant", "content": "blue and yellow"}, "finish_reason": "stop"}],
                "usage": {"prompt_tokens": 10, "completion_tokens": 3, "total_tokens": 13}
            })
            .to_string()
            .into_bytes()
        })
        .create_async()
        .await;
    unsafe {
        std::env::set_var("AGENT_JWT_SECRET", JWT_SECRET);
        std::env::set_var("OPENAI_API_BASE", provider.url());
        std::env::set_var("PLATFORM_OPENAI_API_KEY", "sk-test-platform");
        std::env::set_var("DEFAULT_PROVIDER", "openai");
        std::env::set_var("DEFAULT_MODEL", "gpt-4o-mini");
    }
    let server = common::TestServer::start_with(|c| c.agent_jwt_secret = JWT_SECRET.into()).await;
    let (_, user) = admin_and_member(&server, "att-e2e").await;
    let sid = create_session(&server, &user).await;

    let agent = TextOnlyAgent {
        router_url: server.base_url.clone(),
        token: Arc::new(Mutex::new(String::new())),
        router_replies: Arc::new(Mutex::new(Vec::new())),
    };
    let app = Router::new()
        .route("/", post(text_only_agent_handler))
        .with_state(agent.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let agent_url = format!("http://{}", listener.local_addr().unwrap());
    let _agent_task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let agent_id = seed_running_agent(&server, &user, &agent_url).await;
    *agent.token.lock().unwrap() = nasiko_llm_router::auth::mint_agent_token(
        &agent_id.to_string(),
        &user.id,
        JWT_SECRET,
        600,
        jsonwebtoken::Algorithm::HS256,
    )
    .unwrap();

    let png = png_of(64);
    let file = upload(&server, &user, &sid, png.clone(), "fact.png", "image/png").await;
    let (status, body) = send_turn(&server, &user, Some(agent_id), &sid, &[file], vec![]).await;
    unsafe {
        for key in E2E_ENV {
            std::env::remove_var(key);
        }
    }
    assert_eq!(status, 200, "{body}");

    let router_replies = agent.router_replies.lock().unwrap().clone();
    assert_eq!(router_replies.len(), 1);
    assert_eq!(router_replies[0].0, 200, "{}", router_replies[0].1);
    assert!(router_replies[0].1.contains("blue and yellow"));

    let sent = seen_by_provider.lock().unwrap().clone();
    assert_eq!(sent.len(), 1, "expected exactly one provider call");
    let parts = sent[0]["messages"][0]["content"]
        .as_array()
        .unwrap_or_else(|| panic!("the model got no media parts: {}", sent[0]["messages"]))
        .clone();
    let image_urls: Vec<&str> = parts
        .iter()
        .filter(|p| p["type"] == "image_url")
        .filter_map(|p| p["image_url"]["url"].as_str())
        .collect();
    assert_eq!(
        image_urls,
        vec![format!(
            "data:image/png;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(&png)
        )]
    );
    assert!(
        !sent[0].to_string().contains("nasiko-file://"),
        "the token must not reach the provider"
    );
    assert!(
        parts
            .iter()
            .filter_map(|p| p["text"].as_str())
            .any(|t| t.contains("what is in the attachment?"))
    );

    server.cleanup().await;
}

// ─── Files from earlier turns ────────────────────────────────────────────────

/// Save the user's message the way the web UI does before it dispatches the turn, which ties
/// the turn's uploads to the message.
async fn save_user_message(
    server: &common::TestServer,
    user: &User,
    session_id: &str,
    file_ids: &[String],
) {
    let res = common::as_member(
        server
            .client
            .post(server.url(&format!("/api/chat/sessions/{session_id}/messages"))),
        &user.id,
        &user.name,
    )
    .json(&json!({"role": "user", "content": "see attached", "file_ids": file_ids}))
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 201);
}

/// The filenames of the files the agent got on each of its calls, in order.
fn filenames_per_call(agent: &StubAgent) -> Vec<Vec<String>> {
    agent
        .seen
        .lock()
        .unwrap()
        .iter()
        .map(|req| {
            req["params"]["message"]["parts"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|p| p.get("raw").is_some())
                .map(|p| p["filename"].as_str().unwrap().to_string())
                .collect()
        })
        .collect()
}

#[tokio::test]
#[serial]
async fn a_file_from_an_earlier_turn_comes_with_every_later_turn_once() {
    let server = common::TestServer::start().await;
    let (_, user) = admin_and_member(&server, "att-carry").await;
    let sid = create_session(&server, &user).await;
    let (url, agent, _handle) = start_stub_agent().await;
    let agent_id = seed_running_agent(&server, &user, &url).await;

    let pdf = upload(
        &server,
        &user,
        &sid,
        b"%PDF-1.7 report".to_vec(),
        "report.pdf",
        "application/pdf",
    )
    .await;
    save_user_message(&server, &user, &sid, std::slice::from_ref(&pdf)).await;
    let (status, body) = send_turn(&server, &user, Some(agent_id), &sid, &[pdf], vec![]).await;
    assert_eq!(status, 200, "{body}");

    save_user_message(&server, &user, &sid, &[]).await;
    let (status, body) = send_turn(&server, &user, Some(agent_id), &sid, &[], vec![]).await;
    assert_eq!(status, 200, "{body}");

    let png = upload(&server, &user, &sid, png_of(64), "chart.png", "image/png").await;
    save_user_message(&server, &user, &sid, std::slice::from_ref(&png)).await;
    let (status, body) = send_turn(&server, &user, Some(agent_id), &sid, &[png], vec![]).await;
    assert_eq!(status, 200, "{body}");

    assert_eq!(
        filenames_per_call(&agent),
        [
            vec!["report.pdf"],
            vec!["report.pdf"],
            vec!["report.pdf", "chart.png"],
        ]
    );

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn an_upload_that_was_never_sent_does_not_ride_along() {
    let server = common::TestServer::start().await;
    let (_, user) = admin_and_member(&server, "att-unsent").await;
    let sid = create_session(&server, &user).await;
    let (url, agent, _handle) = start_stub_agent().await;
    let agent_id = seed_running_agent(&server, &user, &url).await;

    // Uploaded, then dropped from the message before it was sent: no message claims it.
    upload(&server, &user, &sid, png_of(64), "dropped.png", "image/png").await;
    save_user_message(&server, &user, &sid, &[]).await;
    let (status, body) = send_turn(&server, &user, Some(agent_id), &sid, &[], vec![]).await;
    assert_eq!(status, 200, "{body}");

    assert_eq!(filenames_per_call(&agent), [Vec::<String>::new()]);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn an_earlier_file_whose_bytes_are_gone_is_left_out_not_refused() {
    let server = common::TestServer::start().await;
    let (_, user) = admin_and_member(&server, "att-carry-gone").await;
    let sid = create_session(&server, &user).await;
    let (url, agent, _handle) = start_stub_agent().await;
    let agent_id = seed_running_agent(&server, &user, &url).await;

    let gone = seed_file_row(
        &server,
        &sid,
        "gone.png",
        &format!("chat-files/{sid}/{}", Uuid::new_v4()),
    )
    .await;
    save_user_message(&server, &user, &sid, &[gone]).await;
    let kept = upload(&server, &user, &sid, png_of(64), "kept.png", "image/png").await;
    save_user_message(&server, &user, &sid, &[kept]).await;

    let (status, body) = send_turn(&server, &user, Some(agent_id), &sid, &[], vec![]).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(filenames_per_call(&agent), [vec!["kept.png"]]);

    server.cleanup().await;
}

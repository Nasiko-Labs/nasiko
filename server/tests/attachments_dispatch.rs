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

const JWT_SECRET: &str = "attachments-dispatch-secret";

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
    let note = texts
        .iter()
        .find(|t| t.contains(nasiko_llm_router::attachments::TOKEN_PREFIX))
        .expect("the agent got no attachment line");
    assert!(texts[0].contains("what is in the attachment?"));
    let lines: Vec<&str> = note.lines().collect();
    assert_eq!(lines.len(), 2, "{note}");
    assert!(lines[0].starts_with("[Attached file: fact.png (image/png, 64 B) "));
    assert!(lines[1].starts_with(&format!(
        "[Attached file: q3.pdf (application/pdf, {pdf_len} B) "
    )));
    assert_eq!(raw_parts_seen(&agent).len(), 2);

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
    assert_eq!(text_parts_seen(&agent).len(), 1);

    server.cleanup().await;
}

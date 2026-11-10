//! Hermetic tests for the WebSocket transport: an in-process server plays the
//! harness shim, answering ACP JSON-RPC over WS Text frames.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use nasiko_acp_client::sdk::schema::ProtocolVersion;
use nasiko_acp_client::sdk::schema::v1::{
    ContentBlock, ContentChunk, InitializeResponse, NewSessionResponse, PermissionOption,
    PermissionOptionKind, PromptResponse, RequestPermissionRequest, SessionId, SessionNotification,
    SessionUpdate, StopReason, TextContent, ToolCallUpdate, ToolCallUpdateFields,
};
use nasiko_acp_client::{
    AcpConnectConfig, AcpConnection, AcpError, ConnectFailure, NewSessionParams, PermissionPolicy,
    TurnEvent, connect_ws,
};
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};

const TEST_TIMEOUT: Duration = Duration::from_secs(5);
const SESSION: &str = "fake-session";

#[derive(Clone, Copy, PartialEq)]
enum Script {
    /// Answer everything, finishing the turn normally.
    Normal,
    /// Push a `session/update` before reading anything from the client.
    UnsolicitedUpdate,
    /// Close the socket as soon as the prompt arrives.
    CloseOnPrompt,
    /// After `session/new`, replay a pending permission request (a harness frozen
    /// mid-approval that the shim resumed).
    ReplayPermission,
    /// After `session/new`, send a Binary frame, which the transport must reject.
    BinaryFrame,
}

/// Responses (messages without a `method`) the shim has received from the client.
type Responses = tokio::sync::mpsc::UnboundedSender<Value>;

/// Bind a shim, serve one connection with `script`, and return its URL plus the
/// `Authorization` header it saw.
async fn spawn_shim(
    script: Script,
) -> (
    String,
    Arc<Mutex<Option<String>>>,
    tokio::sync::mpsc::UnboundedReceiver<Value>,
) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let url = format!("ws://{}/acp", listener.local_addr().unwrap());
    let seen_auth = Arc::new(Mutex::new(None));
    let seen = seen_auth.clone();
    let (sink, responses) = tokio::sync::mpsc::unbounded_channel();
    tokio::spawn(async move {
        let (tcp, _) = listener.accept().await.expect("accept");
        // The `accept_hdr_async` callback below returns tungstenite's fixed
        // `Result<Response, ErrorResponse>`, which clippy flags as a large `Err`.
        #[allow(clippy::result_large_err)]
        let callback = |req: &Request, resp: Response| {
            let auth = req.headers().get("authorization");
            *seen.lock().unwrap() = auth.map(|v| v.to_str().unwrap().to_owned());
            Ok(resp)
        };
        let ws = tokio_tungstenite::accept_hdr_async(tcp, callback)
            .await
            .expect("ws accept");
        serve(ws, script, sink).await;
    });
    (url, seen_auth, responses)
}

fn update_frame() -> String {
    let note = SessionNotification::new(
        SessionId::new(SESSION),
        SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(TextContent::new(
            "replayed",
        )))),
    );
    json!({"jsonrpc": "2.0", "method": "session/update", "params": note}).to_string()
}

fn permission_frame() -> String {
    let params = RequestPermissionRequest::new(
        SessionId::new(SESSION),
        ToolCallUpdate::new("call-1", ToolCallUpdateFields::new()),
        vec![
            PermissionOption::new("deny", "Deny", PermissionOptionKind::RejectOnce),
            PermissionOption::new("allow", "Allow", PermissionOptionKind::AllowOnce),
        ],
    );
    json!({"jsonrpc": "2.0", "id": "p1", "method": "session/request_permission", "params": params})
        .to_string()
}

fn reply(id: &Value, result: Value) -> Message {
    Message::text(json!({"jsonrpc": "2.0", "id": id, "result": result}).to_string())
}

async fn serve(
    mut ws: tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>,
    script: Script,
    responses: Responses,
) {
    if script == Script::UnsolicitedUpdate {
        ws.send(Message::text(update_frame())).await.unwrap();
    }
    while let Some(Ok(msg)) = ws.next().await {
        let Message::Text(text) = msg else { continue };
        let req: Value = serde_json::from_str(text.as_str()).expect("client sent json");
        let id = &req["id"];
        if req.get("method").is_none() {
            let _ = responses.send(req);
            continue;
        }
        let result = match req["method"].as_str() {
            Some("initialize") => json!(InitializeResponse::new(ProtocolVersion::V1)),
            Some("session/new") => json!(NewSessionResponse::new(SessionId::new(SESSION))),
            Some("session/prompt") if script == Script::CloseOnPrompt => {
                let _ = ws.close(None).await;
                return;
            }
            Some("session/prompt") => {
                ws.send(Message::text(update_frame())).await.unwrap();
                json!(PromptResponse::new(StopReason::EndTurn))
            }
            _ => continue,
        };
        ws.send(reply(id, result)).await.unwrap();
        if req["method"] == "session/new" {
            match script {
                Script::ReplayPermission => {
                    ws.send(Message::text(permission_frame())).await.unwrap()
                }
                Script::BinaryFrame => ws.send(Message::binary(vec![1, 2, 3])).await.unwrap(),
                _ => {}
            }
        }
    }
}

async fn connect(url: &str) -> AcpConnection {
    let headers = [("Authorization".to_owned(), "Bearer x".to_owned())];
    connect_ws(url, &headers, AcpConnectConfig::default())
        .await
        .expect("connect_ws")
}

/// Drain a turn, returning (saw an update, terminal event).
async fn drain(conn: &AcpConnection) -> (bool, TurnEvent) {
    let sid = conn
        .new_session(NewSessionParams::new("/workspace"))
        .await
        .expect("new_session");
    let mut turn = conn.prompt(sid, "go").expect("prompt");
    let mut saw_update = false;
    loop {
        let event = tokio::time::timeout(TEST_TIMEOUT, turn.recv())
            .await
            .expect("turn hung")
            .expect("stream ended without a terminal event");
        match event {
            TurnEvent::Update(_) => saw_update = true,
            terminal => return (saw_update, terminal),
        }
    }
}

#[tokio::test]
async fn turn_round_trips_and_sends_headers() {
    let (url, auth, _) = spawn_shim(Script::Normal).await;
    let conn = connect(&url).await;
    let (saw_update, end) = drain(&conn).await;
    assert!(saw_update);
    assert!(matches!(end, TurnEvent::Finished(StopReason::EndTurn)));
    assert_eq!(auth.lock().unwrap().as_deref(), Some("Bearer x"));
}

#[tokio::test]
async fn unsolicited_frame_before_first_request_is_consumed_by_the_client() {
    let (url, _, _) = spawn_shim(Script::UnsolicitedUpdate).await;
    let conn = connect(&url).await;
    let sid = conn
        .new_session(NewSessionParams::new("/workspace"))
        .await
        .expect("new_session");
    let mut turn = conn.prompt(sid, "go").expect("prompt");
    let mut updates = 0;
    loop {
        match tokio::time::timeout(TEST_TIMEOUT, turn.recv())
            .await
            .expect("turn hung")
        {
            Some(TurnEvent::Update(_)) => updates += 1,
            Some(TurnEvent::Finished(_)) => break,
            other => panic!("unexpected end: {other:?}"),
        }
    }
    // The frame replayed before the first request crossed the transport and was
    // consumed by the client, which by design drops updates buffered before a turn
    // starts (`run_turn` drains stragglers). Only the in-turn update is delivered.
    assert_eq!(updates, 1);
}

#[tokio::test]
async fn server_close_mid_turn_fails_the_turn() {
    let (url, _, _) = spawn_shim(Script::CloseOnPrompt).await;
    let conn = connect(&url).await;
    let (_, end) = drain(&conn).await;
    assert!(matches!(end, TurnEvent::Failed(_)));
}

#[tokio::test]
async fn closed_port_is_an_unreachable_connect_failure() {
    let port = {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        l.local_addr().unwrap().port()
    };
    let url = format!("ws://127.0.0.1:{port}/acp");
    match connect_ws(&url, &[], AcpConnectConfig::default()).await {
        Err(AcpError::ConnectFailed {
            kind: ConnectFailure::Unreachable,
            ..
        }) => {}
        Err(other) => panic!("expected Unreachable, got {other:?}"),
        Ok(_) => panic!("expected failure"),
    }
}

#[tokio::test]
async fn replayed_permission_request_reaches_the_delegate() {
    let (url, _, mut responses) = spawn_shim(Script::ReplayPermission).await;
    let config = AcpConnectConfig {
        permission_policy: PermissionPolicy::ApproveAll,
        ..Default::default()
    };
    let conn = connect_ws(&url, &[], config).await.expect("connect_ws");
    let sid = conn
        .new_session(NewSessionParams::new("/workspace"))
        .await
        .expect("new_session");

    // The shim replays the request before any prompt; the client must answer it.
    let answer = tokio::time::timeout(TEST_TIMEOUT, responses.recv())
        .await
        .expect("no permission response reached the shim")
        .expect("shim dropped");
    assert_eq!(answer["id"], "p1");
    assert_eq!(answer["result"]["outcome"]["outcome"], "selected");
    assert_eq!(answer["result"]["outcome"]["optionId"], "allow");

    let mut turn = conn.prompt(sid, "go").expect("prompt");
    loop {
        match tokio::time::timeout(TEST_TIMEOUT, turn.recv())
            .await
            .expect("turn hung")
        {
            Some(TurnEvent::Update(_)) => {}
            Some(TurnEvent::Finished(_)) => break,
            other => panic!("unexpected end: {other:?}"),
        }
    }
}

#[tokio::test]
async fn binary_frame_ends_the_session() {
    let (url, _, _) = spawn_shim(Script::BinaryFrame).await;
    let conn = connect(&url).await;
    let outcome = tokio::time::timeout(TEST_TIMEOUT, async {
        let sid = conn
            .new_session(NewSessionParams::new("/workspace"))
            .await?;
        let mut turn = conn.prompt(sid, "go")?;
        match turn.recv().await {
            Some(TurnEvent::Failed(_)) | None => Ok(()),
            Some(_) => Err(AcpError::Transport("turn did not fail".to_owned())),
        }
    })
    .await
    .expect("session hung after a binary frame");
    match outcome {
        Ok(()) | Err(AcpError::ConnectionClosed | AcpError::Protocol { .. }) => {}
        Err(other) => panic!("unexpected outcome after a binary frame: {other:?}"),
    }
}

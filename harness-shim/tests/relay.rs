//! In-process end-to-end tests of the shim against fake `sh` ACP children.

use std::net::SocketAddr;
use std::time::{Duration, Instant};

use futures::{SinkExt, StreamExt};
use harness_shim::server::{Config, run};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};

type Client = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// Emits a notification then a permission request 200 ms after start, and answers every input
/// line with a result carrying the line's request id and a running count of lines received.
const FAKE_ACP: &str = r#"
(sleep 0.2
 echo '{"jsonrpc":"2.0","method":"session/update","params":{}}'
 echo '{"jsonrpc":"2.0","id":"p1","method":"session/request_permission","params":{}}') &
n=0
while IFS= read -r line; do
  n=$((n+1))
  id=$(printf '%s' "$line" | sed 's/.*"id":\([0-9]*\).*/\1/')
  echo "{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":{\"seq\":$n}}"
done
"#;
const NOTIFICATION: &str = r#"{"jsonrpc":"2.0","method":"session/update","params":{}}"#;
const PERMISSION_REQUEST: &str =
    r#"{"jsonrpc":"2.0","id":"p1","method":"session/request_permission","params":{}}"#;
const DEADLINE: Duration = Duration::from_secs(5);

struct Shim {
    addr: SocketAddr,
    done: JoinHandle<anyhow::Result<i32>>,
}

async fn start(script: &str, token: Option<&str>) -> Shim {
    let config = Config {
        listen: "127.0.0.1:0".parse().unwrap(),
        buffer_lines: 100,
        argv: Some(vec!["sh".into(), "-c".into(), script.into()]),
        mode: harness_shim::handshake::Mode::Env,
        workspace_mount: "/workspace".into(),
        files: vec![],
        token: token.map(str::to_owned),
        workspace: None,
    };
    let (tx, rx) = oneshot::channel();
    let done = tokio::spawn(run(config, Some(tx)));
    Shim {
        addr: rx.await.unwrap(),
        done,
    }
}

async fn connect(addr: SocketAddr) -> Client {
    connect_async(format!("ws://{addr}/acp")).await.unwrap().0
}

async fn next_text(client: &mut Client) -> String {
    loop {
        let msg = tokio::time::timeout(DEADLINE, client.next())
            .await
            .expect("timed out waiting for a frame")
            .expect("stream ended")
            .expect("ws error");
        if let Message::Text(text) = msg {
            return text.to_string();
        }
    }
}

async fn next_close(client: &mut Client) -> CloseFrame {
    tokio::time::timeout(DEADLINE, async {
        while let Some(msg) = client.next().await {
            if let Ok(Message::Close(Some(frame))) = msg {
                return frame;
            }
        }
        panic!("stream ended without a close frame");
    })
    .await
    .expect("timed out waiting for close")
}

async fn send_text(client: &mut Client, text: &str) {
    client.send(Message::Text(text.into())).await.unwrap();
}

/// Writes a `GET /healthz` on `stream` and returns the status and JSON body.
async fn health_on(mut stream: TcpStream) -> (u16, serde_json::Value) {
    tokio::time::timeout(DEADLINE, async {
        stream
            .write_all(b"GET /healthz HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        let mut body = String::new();
        stream.read_to_string(&mut body).await.unwrap();
        let status = body.split_whitespace().nth(1).unwrap().parse().unwrap();
        let json = serde_json::from_str(body.split("\r\n\r\n").nth(1).unwrap().trim()).unwrap();
        (status, json)
    })
    .await
    .expect("healthz timed out")
}

async fn health(addr: SocketAddr) -> serde_json::Value {
    health_on(TcpStream::connect(addr).await.unwrap()).await.1
}

async fn wait_for_buffered(addr: SocketAddr, want: u64) {
    let start = Instant::now();
    while health(addr).await["buffered"] != want {
        assert!(start.elapsed() < DEADLINE, "buffer never reached {want}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn buffers_unsolicited_frames_in_order_and_relays_responses() {
    let shim = start(FAKE_ACP, None).await;
    wait_for_buffered(shim.addr, 2).await;
    assert_eq!(health(shim.addr).await["attached"], false);

    let mut first = connect(shim.addr).await;
    send_text(&mut first, r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#).await;
    assert_eq!(next_text(&mut first).await, NOTIFICATION);
    assert_eq!(next_text(&mut first).await, PERMISSION_REQUEST);
    assert!(next_text(&mut first).await.contains("\"seq\":1"));
    assert_eq!(health(shim.addr).await["attached"], true);

    let mut second = connect(shim.addr).await;
    assert_eq!(next_close(&mut first).await.code, CloseCode::Restart);
    send_text(&mut second, r#"{"jsonrpc":"2.0","id":2,"method":"ping"}"#).await;
    assert!(next_text(&mut second).await.contains("\"seq\":2"));
    assert_eq!(health(shim.addr).await["attached"], true);
}

#[tokio::test]
async fn child_exit_closes_client_and_fails_health() {
    let script = r#"sleep 0.5; echo '{"jsonrpc":"2.0","method":"x"}'; exit 3"#;
    let shim = start(script, None).await;
    let mut client = connect(shim.addr).await;
    // Opened before the exit: the server stops accepting once the shim winds down.
    let health_conn = TcpStream::connect(shim.addr).await.unwrap();

    assert_eq!(
        next_text(&mut client).await,
        r#"{"jsonrpc":"2.0","method":"x"}"#
    );
    let close = next_close(&mut client).await;
    assert_eq!(close.code, CloseCode::Error);
    assert!(
        close.reason.contains("exit") && close.reason.contains('3'),
        "{}",
        close.reason
    );

    let (status, body) = health_on(health_conn).await;
    assert_eq!(status, 503);
    assert_eq!(body, serde_json::json!({"child": "exited"}));
    let code = tokio::time::timeout(DEADLINE, shim.done)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(code, 3);
}

#[tokio::test]
async fn closed_stdout_with_live_child_is_fatal() {
    let shim = start("exec >&-; sleep 30", None).await;
    let code = tokio::time::timeout(Duration::from_secs(8), shim.done)
        .await
        .expect("run did not return")
        .unwrap()
        .unwrap();
    assert_ne!(code, 0);
}

#[tokio::test]
async fn repeated_initialize_is_answered_without_reaching_the_child() {
    let shim = start(FAKE_ACP, None).await;
    let mut first = connect(shim.addr).await;
    send_text(
        &mut first,
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#,
    )
    .await;
    let mut response = next_text(&mut first).await;
    while !response.contains("\"seq\"") {
        response = next_text(&mut first).await;
    }
    assert!(response.contains("\"id\":1") && response.contains("\"seq\":1"));
    first.close(None).await.unwrap();
    drop(first);

    let mut second = connect(shim.addr).await;
    send_text(
        &mut second,
        r#"{"jsonrpc":"2.0","id":7,"method":"initialize","params":{}}"#,
    )
    .await;
    send_text(&mut second, r#"{"jsonrpc":"2.0","id":8,"method":"ping"}"#).await;
    let mut seen = Vec::new();
    while seen.len() < 2 {
        let text = next_text(&mut second).await;
        if text.contains("\"result\"") {
            seen.push(text);
        }
    }
    // Replayed answer carries the new id and the first result; the child saw only 2 lines total.
    assert!(
        seen[0].contains("\"id\":7") && seen[0].contains("\"seq\":1"),
        "{}",
        seen[0]
    );
    assert!(
        seen[1].contains("\"id\":8") && seen[1].contains("\"seq\":2"),
        "{}",
        seen[1]
    );
}

#[tokio::test]
async fn bearer_token_is_required_when_configured() {
    let shim = start(FAKE_ACP, Some("s3cret")).await;
    let url = format!("ws://{}/acp", shim.addr);
    match connect_async(url.clone()).await {
        Err(tokio_tungstenite::tungstenite::Error::Http(response)) => {
            assert_eq!(response.status(), 401)
        }
        other => panic!("expected HTTP 401, got {:?}", other.map(|_| ())),
    }

    let mut wrong = url.clone().into_client_request().unwrap();
    wrong
        .headers_mut()
        .insert("Authorization", "Bearer not-it".parse().unwrap());
    match connect_async(wrong).await {
        Err(tokio_tungstenite::tungstenite::Error::Http(response)) => {
            assert_eq!(response.status(), 401)
        }
        other => panic!(
            "expected HTTP 401 for a wrong token, got {:?}",
            other.map(|_| ())
        ),
    }

    let mut request = url.into_client_request().unwrap();
    request
        .headers_mut()
        .insert("Authorization", "Bearer s3cret".parse().unwrap());
    let (mut client, _) = connect_async(request).await.unwrap();
    assert_eq!(next_text(&mut client).await, NOTIFICATION);
}

#[tokio::test]
async fn a_failed_workspace_clone_is_announced_in_the_first_turn_and_the_shim_stays_up() {
    let mount = tempfile::tempdir().unwrap();
    let config = Config {
        listen: "127.0.0.1:0".parse().unwrap(),
        buffer_lines: 100,
        argv: Some(vec!["sh".into(), "-c".into(), FAKE_ACP.into()]),
        mode: harness_shim::handshake::Mode::Env,
        workspace_mount: "/workspace".into(),
        files: vec![],
        token: None,
        // `file` is not an allowed clone protocol outside the shim's own unit tests, so this
        // repository cannot be fetched.
        workspace: Some(harness_shim::workspace::CloneSpec {
            repo: "file:///no/such/repo".into(),
            token: None,
            mount: mount.path().to_owned(),
            timeout: Duration::from_secs(30),
        }),
    };
    let (tx, rx) = oneshot::channel();
    tokio::spawn(run(config, Some(tx)));
    let addr = rx.await.unwrap();
    assert_eq!(health(addr).await["child"], "running");

    let mut client = connect(addr).await;
    // The scripted child's own startup frames come first; skip to the notice.
    send_text(
        &mut client,
        r#"{"jsonrpc":"2.0","id":7,"method":"session/prompt","params":{"sessionId":"s-1","prompt":[]}}"#,
    )
    .await;
    loop {
        let frame = next_text(&mut client).await;
        if frame.contains("clone failed") {
            assert!(frame.contains(r#""sessionId":"s-1""#), "{frame}");
            assert!(frame.contains("agent_message_chunk"), "{frame}");
            break;
        }
    }
    assert!(
        harness_shim::workspace::needs_clone(mount.path()).unwrap(),
        "the workspace is still empty, so a restart retries"
    );
}

/// Announces its process id as its first frame, then answers every line.
const BOOT_ACP: &str = r#"
echo "{\"jsonrpc\":\"2.0\",\"method\":\"session/update\",\"params\":{\"boot\":\"$$\"}}"
while IFS= read -r line; do :; done
"#;

async fn post_restart(addr: SocketAddr, bearer: Option<&str>) -> u16 {
    let auth = bearer.map_or(String::new(), |b| format!("Authorization: Bearer {b}\r\n"));
    let mut stream = TcpStream::connect(addr).await.unwrap();
    stream
        .write_all(
            format!(
                "POST /control/restart HTTP/1.1\r\nHost: x\r\n{auth}Content-Length: 0\r\nConnection: close\r\n\r\n"
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    let mut body = String::new();
    stream.read_to_string(&mut body).await.unwrap();
    body.split_whitespace().nth(1).unwrap().parse().unwrap()
}

#[tokio::test]
async fn a_restart_request_replaces_the_child_and_needs_the_bearer() {
    let shim = start(BOOT_ACP, Some("s3cret")).await;
    let url = format!("ws://{}/acp", shim.addr);
    let connect_authed = || async {
        let mut request = url.clone().into_client_request().unwrap();
        request
            .headers_mut()
            .insert("Authorization", "Bearer s3cret".parse().unwrap());
        connect_async(request).await.unwrap().0
    };
    let mut first = connect_authed().await;
    let boot_a = next_text(&mut first).await;
    assert!(boot_a.contains("boot"), "{boot_a}");

    assert_eq!(
        post_restart(shim.addr, None).await,
        401,
        "no bearer, no restart"
    );
    assert_eq!(post_restart(shim.addr, Some("wrong")).await, 401);
    assert_eq!(post_restart(shim.addr, Some("s3cret")).await, 200);

    // The old client is told to go; a new one finds a fresh child announcing a different pid.
    let _ = next_close(&mut first).await;
    let mut second = connect_authed().await;
    let boot_b = next_text(&mut second).await;
    assert!(boot_b.contains("boot"), "{boot_b}");
    assert_ne!(boot_a, boot_b, "a new process");
    assert_eq!(health(shim.addr).await["child"], "running");
}

//! Helpers shared by the handshake and eviction tests: a tiny HTTP client and `/acp` access.

#![allow(dead_code)]

use std::net::SocketAddr;
use std::time::{Duration, Instant};

use futures::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};

pub const DEADLINE: Duration = Duration::from_secs(5);

/// How long `wait_for_child` polls; generous because it also waits out a hanging clone.
const WAIT_LIMIT: Duration = Duration::from_secs(20);

pub type Client = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// One HTTP/1.1 request; returns the status and the parsed JSON body (`Null` when empty).
pub async fn http(
    addr: SocketAddr,
    method: &str,
    path: &str,
    bearer: Option<&str>,
    body: &str,
) -> (u16, Value) {
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let auth = bearer.map_or(String::new(), |t| format!("Authorization: Bearer {t}\r\n"));
    let request = format!(
        "{method} {path} HTTP/1.1\r\nHost: shim\r\n{auth}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut raw = String::new();
    tokio::time::timeout(DEADLINE, stream.read_to_string(&mut raw))
        .await
        .unwrap()
        .unwrap();
    let status = raw.split_whitespace().nth(1).unwrap().parse().unwrap();
    let payload = raw.split("\r\n\r\n").nth(1).unwrap_or("");
    (
        status,
        serde_json::from_str(payload.trim()).unwrap_or(Value::Null),
    )
}

pub fn configure_body(token: &str, env: Value, hash: &str) -> String {
    json!({"token": token, "env": env, "workspace": null, "env_hash": hash}).to_string()
}

pub async fn configure(addr: SocketAddr, bearer: Option<&str>, body: &str) -> u16 {
    http(addr, "POST", "/control/configure", bearer, body)
        .await
        .0
}

pub async fn status(addr: SocketAddr, bearer: Option<&str>) -> Value {
    http(addr, "GET", "/control/status", bearer, "").await.1
}

/// Polls `/control/status` until the child is `want`.
pub async fn wait_for_child(addr: SocketAddr, bearer: &str, want: &str) -> Value {
    let started = Instant::now();
    loop {
        let current = status(addr, Some(bearer)).await;
        if current["child"] == want {
            return current;
        }
        assert!(
            started.elapsed() < WAIT_LIMIT,
            "child never became {want}: {current}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

pub async fn connect_acp(addr: SocketAddr, bearer: &str) -> Client {
    let mut request = format!("ws://{addr}/acp").into_client_request().unwrap();
    request
        .headers_mut()
        .insert("Authorization", format!("Bearer {bearer}").parse().unwrap());
    connect_async(request).await.unwrap().0
}

/// The next text frame; `None` when the shim closes the socket instead (a client attached to the
/// old child is told to reconnect on restart).
pub async fn next_text(socket: &mut Client) -> Option<String> {
    match tokio::time::timeout(DEADLINE, socket.next()).await.unwrap() {
        Some(Ok(Message::Text(text))) => Some(text.as_str().to_owned()),
        Some(Ok(Message::Close(_))) => None,
        other => panic!("expected a text or close frame, got {other:?}"),
    }
}

pub async fn first_line(addr: SocketAddr, bearer: &str) -> Option<String> {
    next_text(&mut connect_acp(addr, bearer).await).await
}

/// Sends `line` to a `cat` child over `/acp` and expects it echoed back.
pub async fn assert_echoes(addr: SocketAddr, bearer: &str, line: &str) {
    let mut socket = connect_acp(addr, bearer).await;
    socket.send(Message::Text(line.into())).await.unwrap();
    assert_eq!(next_text(&mut socket).await.as_deref(), Some(line));
}

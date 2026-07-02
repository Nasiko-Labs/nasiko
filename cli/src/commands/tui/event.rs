use std::io::BufRead;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use crossterm::event::{self, Event as CrosstermEvent, KeyEvent};

use crate::config;

use super::session::Session;

// ─── Events flowing into the TUI ────────────────────────────────────────────

#[derive(Debug)]
pub enum AppEvent {
    Key(KeyEvent),
    Tick,
    StreamToken(String),
    StreamStatus(StatusEvent),
    StreamDone,
    StreamError(String),
}

#[derive(Debug, Clone)]
pub struct StatusEvent {
    pub kind: StatusKind,
    pub text: String,
}

#[derive(Debug, Clone)]
pub enum StatusKind {
    Thinking,
    ToolCall,
    ToolResult { success: bool },
    Working,
    Failed,
}

// ─── Event loop: polls crossterm + receives from stream thread ──────────────

pub struct EventLoop {
    rx: mpsc::Receiver<AppEvent>,
    _tick_handle: thread::JoinHandle<()>,
}

impl EventLoop {
    pub fn new() -> (Self, mpsc::Sender<AppEvent>) {
        let (tx, rx) = mpsc::channel();

        let tick_tx = tx.clone();
        let tick_handle = thread::spawn(move || {
            loop {
                if event::poll(Duration::from_millis(50)).unwrap_or(false)
                    && let Ok(CrosstermEvent::Key(key)) = event::read()
                        && tick_tx.send(AppEvent::Key(key)).is_err() {
                            break;
                        }
                if tick_tx.send(AppEvent::Tick).is_err() {
                    break;
                }
                thread::sleep(Duration::from_millis(16));
            }
        });

        (EventLoop { rx, _tick_handle: tick_handle }, tx)
    }

    pub fn next(&self) -> Option<AppEvent> {
        self.rx.recv().ok()
    }
}

// ─── SSE streaming in a background thread ───────────────────────────────────

pub fn send_message_streaming(
    session: &Session,
    text: &str,
    tx: mpsc::Sender<AppEvent>,
) -> thread::JoinHandle<()> {
    let endpoint = session.endpoint.clone();
    let context_id = session.context_id.clone();
    let text = text.to_string();

    thread::spawn(move || {
        if let Err(e) = do_stream_request(&endpoint, &context_id, &text, &tx) {
            let _ = tx.send(AppEvent::StreamError(e.to_string()));
        }
    })
}

fn do_stream_request(
    endpoint: &str,
    context_id: &str,
    text: &str,
    tx: &mpsc::Sender<AppEvent>,
) -> anyhow::Result<()> {
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": uuid::Uuid::new_v4().to_string(),
        "method": "SendStreamingMessage",
        "params": {
            "message": {
                "messageId": uuid::Uuid::new_v4().to_string(),
                "role": "ROLE_USER",
                "parts": [{"text": text}],
                "contextId": context_id
            }
        }
    });

    let http = ureq::Agent::new_with_config(
        ureq::config::Config::builder().timeout_global(None).build(),
    );

    let token = config::active_token().ok().flatten().filter(|_| {
        config::active_url()
            .ok()
            .is_some_and(|u| endpoint.starts_with(&u))
    });

    let trace_id = uuid::Uuid::new_v4().to_string().replace('-', "");
    let span_id = &trace_id[..16];
    let traceparent = format!("00-{trace_id}-{span_id}-01");

    let mut req = http
        .post(endpoint)
        .header("Content-Type", "application/json")
        .header("A2A-Version", "1.0")
        .header("traceparent", &traceparent);
    if let Some(ref t) = token {
        req = req.header("Authorization", &format!("Bearer {t}"));
    }

    let resp = req.send_json(&body)?;

    if resp.status().as_u16() >= 400 {
        let mut resp = resp;
        let err_body = resp.body_mut().read_to_string().unwrap_or_default();
        let _ = tx.send(AppEvent::StreamError(format!(
            "HTTP {}: {err_body}",
            resp.status().as_u16()
        )));
        let _ = tx.send(AppEvent::StreamDone);
        return Ok(());
    }

    let content_type = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();

    if content_type.contains("text/event-stream") {
        parse_sse_stream(resp, tx);
    } else {
        let mut resp = resp;
        if let Ok(json) = resp.body_mut().read_json::<serde_json::Value>() {
            let result = json.get("result").unwrap_or(&json);
            if let Some(text) = nasiko_types::a2a::extract_text(result) {
                let _ = tx.send(AppEvent::StreamToken(text));
            } else if let Some(err) = json.get("error") {
                let _ = tx.send(AppEvent::StreamError(format!("A2A error: {err}")));
            }
        }
        let _ = tx.send(AppEvent::StreamDone);
    }

    Ok(())
}

fn parse_sse_stream(resp: ureq::http::Response<ureq::Body>, tx: &mpsc::Sender<AppEvent>) {
    let (_parts, body) = resp.into_parts();
    let buf = std::io::BufReader::new(body.into_reader());

    for line in buf.lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };
        let data = match line.strip_prefix("data:") {
            Some(d) => d.trim(),
            None => continue,
        };
        if data.is_empty() {
            continue;
        }

        let event: serde_json::Value = match serde_json::from_str(data) {
            Ok(v) => v,
            Err(_) => continue,
        };

        let result = event.get("result").unwrap_or(&event);

        if let Some(task) = result.get("task") {
            if let Some(text) = nasiko_types::a2a::extract_text(task) {
                let _ = tx.send(AppEvent::StreamToken(text));
            }
            let _ = tx.send(AppEvent::StreamDone);
            break;
        } else if let Some(status_update) = result.get("statusUpdate") {
            handle_status_event(status_update, tx);
            if is_terminal(status_update) {
                let _ = tx.send(AppEvent::StreamDone);
                break;
            }
        } else if let Some(artifact_update) = result.get("artifactUpdate") {
            emit_artifact_text(artifact_update, tx);
        } else if let Some(kind) = result.get("kind").and_then(|k| k.as_str()) {
            match kind {
                "artifact-update" => emit_artifact_text(result, tx),
                "status-update" => {
                    handle_status_event(result, tx);
                    if is_terminal(result) {
                        let _ = tx.send(AppEvent::StreamDone);
                        break;
                    }
                }
                _ => {}
            }
        }
    }

    let _ = tx.send(AppEvent::StreamDone);
}

fn handle_status_event(event: &serde_json::Value, tx: &mpsc::Sender<AppEvent>) {
    let state = event
        .pointer("/status/state")
        .and_then(|s| s.as_str())
        .unwrap_or("");

    match state {
        "TASK_STATE_WORKING" | "working" => {
            if let Some(parts) = event
                .pointer("/status/message/parts")
                .and_then(|p| p.as_array())
            {
                for part in parts {
                    if let Some(data) = part.get("data") {
                        emit_status_data(data, tx);
                    } else if let Some(text) = part.get("text").and_then(|t| t.as_str()) {
                        let _ = tx.send(AppEvent::StreamStatus(StatusEvent {
                            kind: StatusKind::Working,
                            text: text.to_string(),
                        }));
                    }
                }
            }
        }
        "TASK_STATE_FAILED" | "failed" => {
            let mut err_text = String::new();
            if let Some(parts) = event
                .pointer("/status/message/parts")
                .and_then(|p| p.as_array())
            {
                for part in parts {
                    if let Some(text) = part.get("text").and_then(|t| t.as_str()) {
                        err_text.push_str(text);
                    }
                }
            }
            let _ = tx.send(AppEvent::StreamStatus(StatusEvent {
                kind: StatusKind::Failed,
                text: err_text,
            }));
        }
        _ => {}
    }
}

fn emit_status_data(data: &serde_json::Value, tx: &mpsc::Sender<AppEvent>) {
    let event_type = data.get("type").and_then(|t| t.as_str()).unwrap_or("");
    match event_type {
        "thinking" => {
            let content = data.get("content").and_then(|c| c.as_str()).unwrap_or("");
            let _ = tx.send(AppEvent::StreamStatus(StatusEvent {
                kind: StatusKind::Thinking,
                text: content.to_string(),
            }));
        }
        "tool_call" => {
            let agent = data.get("agent").and_then(|a| a.as_str()).unwrap_or("?");
            let message = data.get("message").and_then(|m| m.as_str()).unwrap_or("");
            let _ = tx.send(AppEvent::StreamStatus(StatusEvent {
                kind: StatusKind::ToolCall,
                text: format!("{agent}: {message}"),
            }));
        }
        "tool_result" => {
            let agent = data.get("agent").and_then(|a| a.as_str()).unwrap_or("?");
            let success = data.get("success").and_then(|s| s.as_bool()).unwrap_or(false);
            let result = data.get("result").and_then(|r| r.as_str()).unwrap_or("");
            let display = if result.len() > 120 {
                let n = result.floor_char_boundary(120);
                format!("{}...", &result[..n])
            } else {
                result.to_string()
            };
            let _ = tx.send(AppEvent::StreamStatus(StatusEvent {
                kind: StatusKind::ToolResult { success },
                text: format!("{agent}: {display}"),
            }));
        }
        _ => {}
    }
}

fn emit_artifact_text(event: &serde_json::Value, tx: &mpsc::Sender<AppEvent>) {
    let parts = event
        .pointer("/artifact/parts")
        .or_else(|| event.get("parts"))
        .and_then(|p| p.as_array());

    if let Some(parts) = parts {
        for part in parts {
            if let Some(text) = part.get("text").and_then(|t| t.as_str()) {
                let _ = tx.send(AppEvent::StreamToken(text.to_string()));
            }
        }
    }
}

fn is_terminal(event: &serde_json::Value) -> bool {
    let state = event
        .pointer("/status/state")
        .and_then(|s| s.as_str())
        .unwrap_or("");
    matches!(
        state,
        "TASK_STATE_COMPLETED"
            | "TASK_STATE_FAILED"
            | "TASK_STATE_CANCELED"
            | "completed"
            | "failed"
            | "canceled"
    )
}

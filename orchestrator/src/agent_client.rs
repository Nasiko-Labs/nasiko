use std::time::Duration;

use async_stream::stream;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use futures::Stream;
use reqwest::Client;
use serde::Serialize;
use uuid::Uuid;

use crate::error::RouterError;
use crate::types::FilePart;

pub struct AgentClient {
    http: Client,
}

/// A single SSE event decoded from the agent's stream response.
#[derive(Debug, Clone)]
pub struct SseEvent {
    pub event: Option<String>,
    pub data: String,
}

impl AgentClient {
    pub fn new(timeout_secs: u64) -> Self {
        Self {
            http: Client::builder()
                .timeout(Duration::from_secs(timeout_secs))
                .build()
                .expect("failed to build HTTP client"),
        }
    }

    /// Stream SSE events from an A2A agent at `url`.
    ///
    /// - Builds a JSON-RPC 2.0 `message/stream` payload
    /// - Injects `traceparent` header for cross-service trace correlation
    /// - Decodes the SSE byte stream line-by-line
    pub fn send(
        &self,
        url: String,
        query: String,
        file_parts: Vec<FilePart>,
        context_id: String,
        traceparent: Option<String>,
    ) -> impl Stream<Item = Result<SseEvent, RouterError>> + '_ {
        let payload = build_payload(&query, &file_parts, &context_id);

        stream! {
            let mut req = self
                .http
                .post(&url)
                .header("Content-Type", "application/json")
                .header("Accept", "text/event-stream");

            if let Some(ref tp) = traceparent {
                req = req.header("traceparent", tp.as_str());
            }

            let mut resp = req
                .json(&payload)
                .send()
                .await
                .map_err(|e| RouterError::Internal(format!("agent request failed: {e}")))?;

            if !resp.status().is_success() {
                let status = resp.status();
                let body = resp.text().await.unwrap_or_default();
                yield Err(RouterError::Internal(format!("agent returned {status}: {body}")));
                return;
            }

            let mut buf = String::new();
            let mut current_event: Option<String> = None;
            let mut current_data = String::new();

            loop {
                let chunk = resp.chunk().await.map_err(|e| RouterError::Internal(format!("stream read error: {e}")))?;
                let Some(chunk) = chunk else { break };
                buf.push_str(&String::from_utf8_lossy(&chunk));

                while let Some(pos) = buf.find('\n') {
                    let line = buf[..pos].trim_end_matches('\r').to_string();
                    buf.drain(..=pos);

                    if line.is_empty() {
                        // Blank line = end of one SSE event
                        if !current_data.is_empty() {
                            yield Ok(SseEvent {
                                event: current_event.take(),
                                data: std::mem::take(&mut current_data),
                            });
                        }
                    } else if let Some(rest) = line.strip_prefix("event:") {
                        current_event = Some(rest.trim().to_string());
                    } else if let Some(rest) = line.strip_prefix("data:") {
                        if !current_data.is_empty() {
                            current_data.push('\n');
                        }
                        current_data.push_str(rest.trim());
                    }
                    // ignore "id:" and "retry:" fields
                }
            }

            // Flush any trailing event (stream closed without trailing blank line)
            if !current_data.is_empty() {
                yield Ok(SseEvent {
                    event: current_event,
                    data: current_data,
                });
            }
        }
    }
}

impl FilePart {
    /// Encode a raw file into the `FilePart` format stored in orchestrator types.
    /// The `data` field becomes a base64 data URI: `data:<mime>;base64,<bytes>`.
    pub fn encode(filename: String, bytes: &[u8], mime_type: String) -> Self {
        let encoded = B64.encode(bytes);
        let data_uri = format!("data:{};base64,{}", mime_type, encoded);
        FilePart {
            filename,
            content_type: mime_type,
            data: data_uri.into_bytes(),
        }
    }
}

// ── JSON-RPC payload ──────────────────────────────────────────────────────────

#[derive(Serialize)]
struct JsonRpcRequest {
    jsonrpc: &'static str,
    id: String,
    method: &'static str,
    params: MessageParams,
}

#[derive(Serialize)]
struct MessageParams {
    message: A2AMessage,
    #[serde(rename = "contextId")]
    context_id: String,
}

#[derive(Serialize)]
struct A2AMessage {
    role: &'static str,
    parts: Vec<MessagePart>,
    #[serde(rename = "messageId")]
    message_id: String,
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
enum MessagePart {
    Text { text: String },
    File { file: FileData },
}

#[derive(Serialize)]
struct FileData {
    name: String,
    #[serde(rename = "mimeType")]
    mime_type: String,
    bytes: String,
}

fn build_payload(query: &str, file_parts: &[FilePart], context_id: &str) -> JsonRpcRequest {
    let mut parts: Vec<MessagePart> = vec![MessagePart::Text {
        text: query.to_string(),
    }];

    for fp in file_parts {
        // `fp.data` is stored as a base64 data URI — extract just the base64 payload after the comma
        let raw = String::from_utf8_lossy(&fp.data);
        let base64_bytes = if let Some(comma_pos) = raw.find(',') {
            raw[comma_pos + 1..].to_string()
        } else {
            raw.to_string()
        };

        parts.push(MessagePart::File {
            file: FileData {
                name: fp.filename.clone(),
                mime_type: fp.content_type.clone(),
                bytes: base64_bytes,
            },
        });
    }

    JsonRpcRequest {
        jsonrpc: "2.0",
        id: Uuid::new_v4().to_string(),
        method: "message/stream",
        params: MessageParams {
            message: A2AMessage {
                role: "user",
                parts,
                message_id: Uuid::new_v4().to_string(),
            },
            context_id: context_id.to_string(),
        },
    }
}


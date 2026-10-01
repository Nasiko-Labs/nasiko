use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Budget for a unary call (`message/send`, agent card fetches). Short by
/// design: the same client serves discovery, where a long hang is a bug, not
/// patience. A caller that makes real agent turns sets its own with
/// [`A2aClient::with_timeout`].
const DEFAULT_UNARY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Budget for a streaming call (`message/stream`). Streams legitimately outlive
/// unary calls — progress events keep the caller informed while the agent works.
const DEFAULT_STREAM_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(600);

/// A2A JSON-RPC client for calling remote agents via the protocol.
#[derive(Clone)]
pub struct A2aClient {
    http: reqwest::Client,
    default_timeout: std::time::Duration,
    stream_timeout: std::time::Duration,
    request_metadata: Option<serde_json::Value>,
    extra_headers: Vec<(String, String)>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct A2aResponse {
    pub jsonrpc: String,
    pub id: String,
    pub result: Option<serde_json::Value>,
    pub error: Option<A2aJsonRpcError>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct A2aJsonRpcError {
    pub code: i32,
    pub message: String,
    pub data: Option<serde_json::Value>,
}

/// Everything needed to resume the sub-agent's own paused task later. Captured here, server-side,
/// from the live A2A response — never round-tripped through an LLM tool-call argument, since
/// nothing guarantees an LLM will supply the right `task_id`/`context_id` on a later call.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PauseInfo {
    pub kind: nasiko_types::a2a::AwaitingHumanKind,
    pub message: String,
    /// The sub-agent's own A2A task id — not the caller's.
    pub task_id: String,
    /// The sub-agent's own A2A context id — may differ from the context id this call was sent
    /// with (see `extract_task_and_context_id`).
    pub context_id: String,
    pub metadata: serde_json::Value,
}

/// The outcome of a completed `send_message_streaming[_dialect]` call. A pause is not an error —
/// distinguishing it as its own outcome (rather than an `Err`, or a plain `String` a caller has to
/// sniff) is what lets a caller act on it deliberately instead of accidentally treating a
/// question as a finished answer.
#[derive(Debug, Clone, PartialEq)]
pub enum SendOutcome {
    Text(String),
    AwaitingHuman(PauseInfo),
}

/// Live event relayed from a streaming agent call (see
/// [`A2aClient::send_message_streaming`]).
#[derive(Debug, Clone, PartialEq)]
pub enum AgentStreamEvent {
    /// The agent's own progress narration (e.g. "web_search: <query>").
    Status(String),
    /// A chunk of the agent's reply text as it generates.
    Content(String),
    /// The agent needs a human before it can continue.
    AwaitingHuman(PauseInfo),
    /// A structured data part from the called agent's own stream (e.g. a
    /// nested orchestrator's `agent_invoke`/`agent_result` for its own
    /// sub-agents). Relayed as-is — depth is still bounded independently by
    /// `FlowGuard::max_depth`, so this cannot grow unbounded.
    Data(serde_json::Value),
}

/// One JSON-RPC `result` object from a streaming call, unclassified — for a
/// caller that needs the raw `kind`/`append`/`final` shape
/// [`AgentStreamEvent`] collapses away (see
/// [`A2aClient::send_message_streaming_raw`]).
#[derive(Debug)]
pub enum RawAgentFrame {
    Frame(serde_json::Value),
    Error(A2aClientError),
}

/// A2A method-name / message-shape dialect.
///
/// Standard JSON-RPC agents (`preferredTransport: JSONRPC`, e.g. a2a-sdk) use
/// `message/send`/`message/stream` with `role: "user"` and typed
/// `{"kind":"text"}` parts. Some endpoints (notably the control-plane registry)
/// instead expose the gRPC/proto method names over JSON-RPC, with `role:
/// "ROLE_USER"` and untyped `{"text":…}` parts. `AgentInfo` doesn't carry the
/// transport, so the client attempts [`Dialect::JsonRpc`] first and falls back
/// to [`Dialect::Proto`] on `-32601` (method not found).
#[derive(Clone, Copy, Debug)]
enum Dialect {
    JsonRpc,
    Proto,
}

impl Dialect {
    fn method(self, streaming: bool) -> &'static str {
        match (self, streaming) {
            (Dialect::JsonRpc, false) => "message/send",
            (Dialect::JsonRpc, true) => "message/stream",
            (Dialect::Proto, false) => "SendMessage",
            (Dialect::Proto, true) => "SendStreamingMessage",
        }
    }

    fn role(self) -> &'static str {
        match self {
            Dialect::JsonRpc => "user",
            Dialect::Proto => "ROLE_USER",
        }
    }

    fn text_part(self, text: &str) -> serde_json::Value {
        match self {
            Dialect::JsonRpc => serde_json::json!({"kind": "text", "text": text}),
            Dialect::Proto => serde_json::json!({"text": text}),
        }
    }
}

impl Default for A2aClient {
    fn default() -> Self {
        Self::new()
    }
}

impl A2aClient {
    pub fn new() -> Self {
        Self::with_http_client(reqwest::Client::new())
    }

    pub fn with_http_client(http: reqwest::Client) -> Self {
        Self {
            http,
            default_timeout: DEFAULT_UNARY_TIMEOUT,
            stream_timeout: DEFAULT_STREAM_TIMEOUT,
            request_metadata: None,
            extra_headers: Vec::new(),
        }
    }

    /// Set one budget for both unary and streaming calls. Callers that make
    /// real agent turns pass the platform's agent-call timeout here, which
    /// lifts `message/send` off the short discovery default — the fallback path
    /// taken by agents that reject `message/stream` is a full agent turn too.
    pub fn with_timeout(mut self, timeout: std::time::Duration) -> Self {
        self.default_timeout = timeout;
        self.stream_timeout = timeout;
        self
    }

    /// Set metadata to inject into `params.metadata` on all outbound A2A requests.
    pub fn with_metadata(mut self, metadata: serde_json::Value) -> Self {
        self.request_metadata = Some(metadata);
        self
    }

    /// Set extra HTTP headers on all outbound requests (e.g. traceparent for OTel).
    pub fn with_headers(mut self, headers: Vec<(String, String)>) -> Self {
        self.extra_headers = headers;
        self
    }

    /// The forwarded `traceparent` header value (used only for flow-correlation logs),
    /// or `"<none>"` when the client wasn't given one. The trace id inside it is the
    /// platform flow/conversation id the gateway maps back to a `flows` row.
    fn forwarded_traceparent(&self) -> &str {
        self.extra_headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("traceparent"))
            .map(|(_, v)| v.as_str())
            .unwrap_or("<none>")
    }

    /// Build the JSON-RPC request body for `message/send`/`message/stream` (or
    /// their proto-named equivalents), injecting `request_metadata` into
    /// `params.metadata`.
    fn build_message_body(
        &self,
        dialect: Dialect,
        streaming: bool,
        message: &str,
        ctx: &str,
        extra_parts: &[serde_json::Value],
    ) -> serde_json::Value {
        let mut parts = vec![dialect.text_part(message)];
        parts.extend(extra_parts.iter().cloned());

        let mut body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": Uuid::new_v4().to_string(),
            "method": dialect.method(streaming),
            "params": {
                "message": {
                    "messageId": Uuid::new_v4().to_string(),
                    "role": dialect.role(),
                    "parts": parts,
                    "contextId": ctx
                }
            }
        });

        if let Some(ref metadata) = self.request_metadata
            && let Some(params) = body.get_mut("params")
        {
            params
                .as_object_mut()
                .map(|p| p.insert("metadata".to_string(), metadata.clone()));
        }

        body
    }

    /// Send a message to an A2A agent and block until task completion.
    ///
    /// Tries the standard A2A JSON-RPC dialect (`message/send`) first; if the
    /// agent doesn't recognize that method (`-32601`), retries with the
    /// gRPC/proto method name (`SendMessage`). This lets the orchestrator talk
    /// to both `preferredTransport: JSONRPC` agents and proto-name endpoints
    /// (e.g. the control-plane registry) without a prior capability lookup.
    pub async fn send_message(
        &self,
        endpoint: &str,
        message: &str,
        context_id: Option<&str>,
    ) -> Result<A2aResponse, A2aClientError> {
        self.send_message_with_headers(endpoint, message, context_id, &[], &[])
            .await
            .map(|(_, response)| response)
    }

    /// Like [`send_message`], plus per-call headers layered on top of the
    /// client-wide `extra_headers` (e.g. a delegation token scoped to the one
    /// specific agent being called, which differs per call unlike `traceparent`).
    ///
    /// Returns the resolved `context_id` alongside the response — when the caller passes `None`,
    /// this mints a fresh one and sends it as the real `contextId` on the wire, so a caller that
    /// needs to know what conversation id the agent was actually talked under (e.g. to record a
    /// pause the agent's own reply didn't echo an id for) must use this returned value, not the
    /// `None` it originally passed in.
    pub async fn send_message_with_headers(
        &self,
        endpoint: &str,
        message: &str,
        context_id: Option<&str>,
        per_call_headers: &[(String, String)],
        extra_parts: &[serde_json::Value],
    ) -> Result<(String, A2aResponse), A2aClientError> {
        let ctx = context_id
            .map(|s| s.to_string())
            .unwrap_or_else(|| Uuid::new_v4().to_string());

        tracing::debug!(
            target: "nasiko::flow",
            endpoint,
            context_id = %ctx,
            traceparent = self.forwarded_traceparent(),
            "a2a send_message → forwarding to agent (trace_id in traceparent = flow/conversation id)"
        );

        let result = match self
            .send_message_dialect(
                endpoint,
                message,
                &ctx,
                per_call_headers,
                Dialect::JsonRpc,
                extra_parts,
            )
            .await
        {
            Err(A2aClientError::A2aProtocol { code: -32601, .. }) => {
                tracing::debug!(
                    endpoint,
                    "agent rejected JSON-RPC method (-32601); retrying with proto method name"
                );
                self.send_message_dialect(
                    endpoint,
                    message,
                    &ctx,
                    per_call_headers,
                    Dialect::Proto,
                    extra_parts,
                )
                .await
            }
            other => other,
        };
        result.map(|response| (ctx, response))
    }

    async fn send_message_dialect(
        &self,
        endpoint: &str,
        message: &str,
        ctx: &str,
        per_call_headers: &[(String, String)],
        dialect: Dialect,
        extra_parts: &[serde_json::Value],
    ) -> Result<A2aResponse, A2aClientError> {
        let mut body = self.build_message_body(dialect, false, message, ctx, extra_parts);

        if let Some(ref metadata) = self.request_metadata
            && let Some(params) = body.get_mut("params")
        {
            params
                .as_object_mut()
                .map(|p| p.insert("metadata".to_string(), metadata.clone()));
        }

        let mut req = self
            .http
            .post(endpoint)
            .header("A2A-Version", nasiko_types::a2a::A2A_VERSION_HEADER_VALUE)
            .json(&body)
            .timeout(self.default_timeout);

        for (key, value) in self.extra_headers.iter().chain(per_call_headers) {
            req = req.header(key, value);
        }

        let resp = req
            .send()
            .await
            .map_err(|e| A2aClientError::Network(e.to_string()))?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(A2aClientError::Http(status.as_u16(), body));
        }

        let a2a_resp: A2aResponse = resp
            .json()
            .await
            .map_err(|e| A2aClientError::InvalidResponse(e.to_string()))?;

        if let Some(ref err) = a2a_resp.error {
            return Err(A2aClientError::A2aProtocol {
                code: err.code,
                message: err.message.clone(),
            });
        }

        Ok(a2a_resp)
    }

    /// Send a message via `message/stream` (proto: `SendStreamingMessage`) and
    /// consume the SSE stream.
    ///
    /// Live events are relayed through `progress` (if provided): the agent's
    /// working-status updates (its internal tool activity) and its reply text
    /// as it generates. Sends await channel capacity — nothing is dropped —
    /// but a closed receiver (caller went away) is tolerated: the stream is
    /// still consumed to completion so the collected text can serve as the
    /// tool result. Agents that answer with plain JSON instead of an event
    /// stream are handled transparently, so callers don't need a capability
    /// check first.
    pub async fn send_message_streaming(
        &self,
        endpoint: &str,
        message: &str,
        context_id: Option<&str>,
        progress: Option<tokio::sync::mpsc::Sender<AgentStreamEvent>>,
        per_call_headers: &[(String, String)],
        extra_parts: &[serde_json::Value],
    ) -> Result<SendOutcome, A2aClientError> {
        let ctx = context_id
            .map(|s| s.to_string())
            .unwrap_or_else(|| Uuid::new_v4().to_string());

        tracing::debug!(
            target: "nasiko::flow",
            endpoint,
            context_id = %ctx,
            traceparent = self.forwarded_traceparent(),
            "a2a send_message_streaming → forwarding to agent (trace_id in traceparent = flow/conversation id)"
        );

        // Try standard JSON-RPC (`message/stream`) first, falling back to the
        // proto method name on `-32601`. A method-not-found reply arrives as a
        // plain JSON error before any SSE event, so no `progress` events are
        // emitted on the failed attempt and the retry is clean.
        match self
            .send_message_streaming_dialect(
                endpoint,
                message,
                &ctx,
                progress.clone(),
                per_call_headers,
                Dialect::JsonRpc,
                extra_parts,
            )
            .await
        {
            Err(A2aClientError::A2aProtocol { code: -32601, .. }) => {
                tracing::debug!(
                    endpoint,
                    "agent rejected JSON-RPC streaming method (-32601); retrying with proto method name"
                );
                self.send_message_streaming_dialect(
                    endpoint,
                    message,
                    &ctx,
                    progress,
                    per_call_headers,
                    Dialect::Proto,
                    extra_parts,
                )
                .await
            }
            other => other,
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn send_message_streaming_dialect(
        &self,
        endpoint: &str,
        message: &str,
        ctx: &str,
        progress: Option<tokio::sync::mpsc::Sender<AgentStreamEvent>>,
        per_call_headers: &[(String, String)],
        dialect: Dialect,
        extra_parts: &[serde_json::Value],
    ) -> Result<SendOutcome, A2aClientError> {
        use futures::StreamExt as _;

        let mut body = self.build_message_body(dialect, true, message, ctx, extra_parts);

        if let Some(ref metadata) = self.request_metadata
            && let Some(params) = body.get_mut("params")
        {
            params
                .as_object_mut()
                .map(|p| p.insert("metadata".to_string(), metadata.clone()));
        }

        let mut req = self
            .http
            .post(endpoint)
            .header("A2A-Version", nasiko_types::a2a::A2A_VERSION_HEADER_VALUE)
            .header("Accept", "text/event-stream")
            .json(&body)
            .timeout(self.stream_timeout);

        for (key, value) in self.extra_headers.iter().chain(per_call_headers) {
            req = req.header(key, value);
        }

        let resp = req
            .send()
            .await
            .map_err(|e| A2aClientError::Network(e.to_string()))?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(A2aClientError::Http(status.as_u16(), body));
        }

        let content_type = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();

        if !content_type.contains("text/event-stream") {
            // Agent answered non-streaming — treat as a SendMessage response.
            let a2a: A2aResponse = resp
                .json()
                .await
                .map_err(|e| A2aClientError::InvalidResponse(e.to_string()))?;
            if let Some(ref err) = a2a.error {
                return Err(A2aClientError::A2aProtocol {
                    code: err.code,
                    message: err.message.clone(),
                });
            }
            // A non-streaming reply can pause too (an agent that never opens an event stream
            // is not exempt from the A2A task-lifecycle states) — classify it the same way as a
            // streamed statusUpdate, not just the streaming branch below.
            let value = a2a.result.clone().unwrap_or(serde_json::Value::Null);
            for sse in nasiko_types::a2a::classify_sse_event(&value) {
                if let nasiko_types::a2a::SseEvent::AwaitingHuman {
                    kind,
                    message,
                    metadata,
                } = sse
                {
                    let (task_id, context_id) = Self::extract_task_and_context_id(&value, ctx);
                    let pause = PauseInfo {
                        kind,
                        message,
                        task_id,
                        context_id,
                        metadata,
                    };
                    // Mirror the SSE branch below: relay live via `progress` before returning.
                    // Without this, a sub-agent that pauses without ever opening an event stream
                    // (mockito's default content-type, and plausibly many real agents) produces a
                    // pause `run_stream_inner` correctly detects via the returned value, but never
                    // announces on the orchestrator event stream — `run_stream_inner`'s call sites
                    // deliberately don't send it themselves, on the assumption that this relay
                    // already did. That assumption only held for the SSE branch until now.
                    if let Some(ref tx) = progress {
                        let _ = tx
                            .send(AgentStreamEvent::AwaitingHuman(pause.clone()))
                            .await;
                    }
                    return Ok(SendOutcome::AwaitingHuman(pause));
                }
            }
            return Ok(SendOutcome::Text(
                Self::extract_text(&a2a).unwrap_or_default(),
            ));
        }

        let mut collected = String::new();
        // Byte buffer, decoded per complete line: a multibyte char split
        // across chunk boundaries must not be lossy-decoded mid-sequence.
        let mut buffer: Vec<u8> = Vec::new();
        let mut stream = resp.bytes_stream();

        'stream: while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| A2aClientError::Network(e.to_string()))?;
            buffer.extend_from_slice(&chunk);

            while let Some(pos) = buffer.iter().position(|&b| b == b'\n') {
                let line_bytes: Vec<u8> = buffer.drain(..=pos).collect();
                let line = String::from_utf8_lossy(&line_bytes);
                let line = line.trim_end_matches(['\n', '\r']);

                let Some(data) = line.strip_prefix("data:") else {
                    continue;
                };
                let data = data.trim();
                if data.is_empty() {
                    continue;
                }
                let Ok(event) = serde_json::from_str::<serde_json::Value>(data) else {
                    continue;
                };

                for sse in nasiko_types::a2a::classify_sse_event(&event) {
                    use nasiko_types::a2a::SseEvent;
                    match sse {
                        SseEvent::ArtifactText(text) => {
                            if let Some(ref tx) = progress {
                                // Closed receiver is fine — keep collecting for
                                // the tool result even if nobody is watching.
                                let _ = tx.send(AgentStreamEvent::Content(text.clone())).await;
                            }
                            collected.push_str(&text);
                        }
                        SseEvent::StatusText(text) => {
                            if let Some(ref tx) = progress {
                                let _ = tx.send(AgentStreamEvent::Status(text)).await;
                            }
                        }
                        SseEvent::StatusData(data) => {
                            if let Some(ref tx) = progress {
                                let _ = tx.send(AgentStreamEvent::Data(data)).await;
                            }
                        }
                        SseEvent::Completed { snapshot_text } => {
                            if collected.is_empty()
                                && let Some(t) = snapshot_text
                            {
                                collected = t;
                            }
                            break 'stream;
                        }
                        SseEvent::Failed { reason } => {
                            return Err(A2aClientError::A2aProtocol {
                                code: -1,
                                message: reason,
                            });
                        }
                        SseEvent::AwaitingHuman {
                            kind,
                            message,
                            metadata,
                        } => {
                            let (task_id, context_id) =
                                Self::extract_task_and_context_id(&event, ctx);
                            let pause = PauseInfo {
                                kind,
                                message,
                                task_id,
                                context_id,
                                metadata,
                            };
                            if let Some(ref tx) = progress {
                                let _ = tx
                                    .send(AgentStreamEvent::AwaitingHuman(pause.clone()))
                                    .await;
                            }
                            return Ok(SendOutcome::AwaitingHuman(pause));
                        }
                    }
                }
            }
        }

        Ok(SendOutcome::Text(collected))
    }

    /// Recovers the sub-agent's own `taskId`/`contextId` from a raw SSE event payload — fields
    /// `classify_sse_event` deliberately doesn't carry, since `SseEvent` is semantic content, not
    /// protocol envelope. Covers the same wire shapes `classify_sse_event` understands
    /// (JSONRPC-wrapped or bare `statusUpdate`; full `task` snapshot). `context_id` falls back to
    /// the id this call was sent with — the agent is expected to preserve it, but if it doesn't
    /// echo one back, the id we sent is still the correct conversation to resume against.
    /// `task_id` has no such fallback: a task in a pause state necessarily already has one.
    ///
    /// `pub(crate)`: also used by `tool.rs`'s non-streaming fallback path, which hits the same
    /// pause-detection requirement through a different A2A response shape (`A2aResponse`, not a
    /// raw SSE event) but needs the identical id-recovery logic.
    pub(crate) fn extract_task_and_context_id(
        event: &serde_json::Value,
        sent_context_id: &str,
    ) -> (String, String) {
        let result = event.get("result").unwrap_or(event);
        let envelope = result
            .get("statusUpdate")
            .or_else(|| result.get("task"))
            .unwrap_or(result);

        let task_id = envelope
            .get("taskId")
            .or_else(|| envelope.get("id"))
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        let context_id = envelope
            .get("contextId")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .unwrap_or_else(|| sent_context_id.to_string());

        (task_id, context_id)
    }

    /// Like [`send_message_streaming`], but relays every JSON-RPC `result`
    /// object over `tx` unclassified — for a caller that needs frame-level
    /// fidelity (`kind`/`append`/`final`) [`AgentStreamEvent`] can't give.
    pub async fn send_message_streaming_raw(
        &self,
        endpoint: &str,
        message: &str,
        context_id: Option<&str>,
        tx: tokio::sync::mpsc::Sender<RawAgentFrame>,
        per_call_headers: &[(String, String)],
    ) -> Result<(), A2aClientError> {
        use futures::StreamExt as _;

        let ctx = context_id
            .map(|s| s.to_string())
            .unwrap_or_else(|| Uuid::new_v4().to_string());

        let mut body = self.build_message_body(Dialect::JsonRpc, true, message, &ctx, &[]);
        // On `params.message.metadata`, not `params.metadata` — this method's
        // only real caller (weave_surface.rs) sets `request_metadata` to carry
        // `{skill: "generate-ui", ...}`, and the receiving agent's A2A server
        // reads the skill id off the MESSAGE object it dispatches
        // (`params.message.metadata.skill`), not off `params` itself.
        // `build_message_body`'s own `params.metadata` insertion above is a
        // separate, unrelated convention (e.g. traceparent forwarding for
        // other callers of this client) and is left as-is; this is additive,
        // not a replacement.
        if let Some(ref metadata) = self.request_metadata
            && let Some(message_obj) = body.pointer_mut("/params/message")
        {
            message_obj
                .as_object_mut()
                .map(|m| m.insert("metadata".to_string(), metadata.clone()));
        }

        let mut req = self
            .http
            .post(endpoint)
            .header("A2A-Version", "1.0")
            .header("Accept", "text/event-stream")
            .json(&body)
            .timeout(self.stream_timeout);
        for (key, value) in self.extra_headers.iter().chain(per_call_headers) {
            req = req.header(key, value);
        }

        let resp = req
            .send()
            .await
            .map_err(|e| A2aClientError::Network(e.to_string()))?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(A2aClientError::Http(status.as_u16(), body));
        }

        let content_type = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();

        if !content_type.contains("text/event-stream") {
            let a2a: A2aResponse = resp
                .json()
                .await
                .map_err(|e| A2aClientError::InvalidResponse(e.to_string()))?;
            if let Some(ref err) = a2a.error {
                return Err(A2aClientError::A2aProtocol {
                    code: err.code,
                    message: err.message.clone(),
                });
            }
            if let Some(result) = a2a.result {
                let _ = tx.send(RawAgentFrame::Frame(result)).await;
            }
            return Ok(());
        }

        let mut buffer: Vec<u8> = Vec::new();
        let mut stream = resp.bytes_stream();

        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| A2aClientError::Network(e.to_string()))?;
            buffer.extend_from_slice(&chunk);

            while let Some(pos) = buffer.iter().position(|&b| b == b'\n') {
                let line_bytes: Vec<u8> = buffer.drain(..=pos).collect();
                let line = String::from_utf8_lossy(&line_bytes);
                let line = line.trim_end_matches(['\n', '\r']);

                let Some(data) = line.strip_prefix("data:") else {
                    continue;
                };
                let data = data.trim();
                if data.is_empty() {
                    continue;
                }
                let Ok(event) = serde_json::from_str::<serde_json::Value>(data) else {
                    continue;
                };

                if let Some(err) = event.get("error") {
                    let code = err.get("code").and_then(|c| c.as_i64()).unwrap_or(-1) as i32;
                    let message = err
                        .get("message")
                        .and_then(|m| m.as_str())
                        .unwrap_or("error")
                        .to_string();
                    let _ = tx
                        .send(RawAgentFrame::Error(A2aClientError::A2aProtocol {
                            code,
                            message,
                        }))
                        .await;
                    continue;
                }
                if let Some(result) = event.get("result") {
                    let _ = tx.send(RawAgentFrame::Frame(result.clone())).await;
                }
            }
        }

        Ok(())
    }

    /// Extract text content from an A2A response (artifacts or status message).
    pub fn extract_text(response: &A2aResponse) -> Option<String> {
        let result = response.result.as_ref()?;
        Self::extract_text_from_value(result)
    }

    pub fn extract_text_from_value(result: &serde_json::Value) -> Option<String> {
        // v1.0: result.task.artifacts[].parts[].text
        let task = result.get("task").unwrap_or(result);

        if let Some(artifacts) = task.get("artifacts").and_then(|a| a.as_array()) {
            let text = collect_text_parts(artifacts.iter().filter_map(|a| a.get("parts")));
            if !text.is_empty() {
                return Some(text);
            }
        }

        // v1.0: result.task.status.message.parts[].text
        if let Some(parts) = task
            .pointer("/status/message/parts")
            .and_then(|p| p.as_array())
        {
            let text: String = parts
                .iter()
                .filter_map(|p| p.get("text").and_then(|t| t.as_str()))
                .collect();
            if !text.is_empty() {
                return Some(text);
            }
        }

        // v0.3 fallback: result.artifacts[].parts[].text
        if let Some(artifacts) = result.get("artifacts").and_then(|a| a.as_array()) {
            let text = collect_text_parts(artifacts.iter().filter_map(|a| a.get("parts")));
            if !text.is_empty() {
                return Some(text);
            }
        }

        if let Some(parts) = result
            .pointer("/status/message/parts")
            .and_then(|p| p.as_array())
        {
            let text: String = parts
                .iter()
                .filter_map(|p| p.get("text").and_then(|t| t.as_str()))
                .collect();
            if !text.is_empty() {
                return Some(text);
            }
        }

        None
    }
}

fn collect_text_parts<'a>(parts_arrays: impl Iterator<Item = &'a serde_json::Value>) -> String {
    // Parts within one artifact are contiguous chunks — streaming agents emit
    // one part per token — so they concatenate directly. Only distinct
    // artifacts get a newline between them.
    let mut artifact_texts = Vec::new();
    for parts_val in parts_arrays {
        if let Some(parts) = parts_val.as_array() {
            let text: String = parts
                .iter()
                .filter_map(|p| p.get("text").and_then(|t| t.as_str()))
                .collect();
            if !text.is_empty() {
                artifact_texts.push(text);
            }
        }
    }
    artifact_texts.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jsonrpc_dialect_uses_standard_method_role_and_typed_parts() {
        let client = A2aClient::new();
        let body = client.build_message_body(Dialect::JsonRpc, false, "hi", "ctx-1", &[]);
        assert_eq!(body["method"], "message/send");
        assert_eq!(body["params"]["message"]["role"], "user");
        assert_eq!(body["params"]["message"]["parts"][0]["kind"], "text");
        assert_eq!(body["params"]["message"]["parts"][0]["text"], "hi");
        assert_eq!(body["params"]["message"]["contextId"], "ctx-1");

        let stream = client.build_message_body(Dialect::JsonRpc, true, "hi", "ctx-1", &[]);
        assert_eq!(stream["method"], "message/stream");
    }

    #[test]
    fn proto_dialect_uses_grpc_method_role_and_untyped_parts() {
        let client = A2aClient::new();
        let body = client.build_message_body(Dialect::Proto, false, "hi", "ctx-1", &[]);
        assert_eq!(body["method"], "SendMessage");
        assert_eq!(body["params"]["message"]["role"], "ROLE_USER");
        // Proto parts carry no `kind` discriminator.
        assert!(body["params"]["message"]["parts"][0].get("kind").is_none());
        assert_eq!(body["params"]["message"]["parts"][0]["text"], "hi");

        let stream = client.build_message_body(Dialect::Proto, true, "hi", "ctx-1", &[]);
        assert_eq!(stream["method"], "SendStreamingMessage");
    }

    #[test]
    fn request_metadata_is_injected_into_params() {
        let client = A2aClient::new().with_metadata(serde_json::json!({"traceparent": "abc"}));
        let body = client.build_message_body(Dialect::JsonRpc, false, "hi", "ctx-1", &[]);
        assert_eq!(body["params"]["metadata"]["traceparent"], "abc");
    }

    #[test]
    fn extra_file_parts_are_included_in_message() {
        let client = A2aClient::new();
        let file_part = serde_json::json!({"raw": "aGVsbG8=", "filename": "test.txt", "mediaType": "text/plain"});
        let body = client.build_message_body(
            Dialect::JsonRpc,
            false,
            "analyze this",
            "ctx-1",
            &[file_part],
        );
        let parts = body["params"]["message"]["parts"].as_array().unwrap();
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0]["text"], "analyze this");
        assert_eq!(parts[1]["raw"], "aGVsbG8=");
        assert_eq!(parts[1]["filename"], "test.txt");
    }

    #[test]
    fn extracts_v1_task_artifact_text() {
        let result = serde_json::json!({
            "task": {
                "artifacts": [{"artifactId": "a1", "parts": [{"text": "Hello."}]}],
                "status": {"state": "TASK_STATE_COMPLETED"}
            }
        });
        assert_eq!(
            A2aClient::extract_text_from_value(&result).as_deref(),
            Some("Hello.")
        );
    }

    #[test]
    fn concatenates_streamed_token_parts_without_newlines() {
        // Streaming agents emit one part per token chunk; they must
        // concatenate seamlessly, not be newline-joined.
        let result = serde_json::json!({
            "task": {
                "artifacts": [{
                    "artifactId": "a1",
                    "parts": [{"text": "I"}, {"text": "'ll"}, {"text": " start"}]
                }]
            }
        });
        assert_eq!(
            A2aClient::extract_text_from_value(&result).as_deref(),
            Some("I'll start")
        );
    }

    #[test]
    fn empty_text_parts_yield_none() {
        // An agent that lost its final answer returns {"text": ""} — the
        // caller must see None, not an empty string masquerading as content.
        let result = serde_json::json!({
            "task": {"artifacts": [{"artifactId": "a1", "parts": [{"text": ""}]}]}
        });
        assert_eq!(A2aClient::extract_text_from_value(&result), None);
    }

    #[tokio::test]
    async fn send_message_streaming_raw_relays_each_frame_unmodified() {
        let mut server = mockito::Server::new_async().await;
        let body = concat!(
            "data: {\"jsonrpc\":\"2.0\",\"id\":\"1\",\"result\":{\"kind\":\"status-update\",\"status\":{\"state\":\"working\"}}}\n\n",
            "data: {\"jsonrpc\":\"2.0\",\"id\":\"1\",\"result\":{\"kind\":\"artifact-update\",\"append\":true,\"final\":false}}\n\n",
        );
        let mock = server
            .mock("POST", "/")
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(body)
            .create_async()
            .await;

        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        let client = A2aClient::new();
        client
            .send_message_streaming_raw(&server.url(), "hi", Some("ctx-1"), tx, &[])
            .await
            .unwrap();
        mock.assert_async().await;

        let first = rx.recv().await.unwrap();
        let RawAgentFrame::Frame(v) = first else {
            panic!("expected a Frame");
        };
        assert_eq!(v["kind"], "status-update");

        let second = rx.recv().await.unwrap();
        let RawAgentFrame::Frame(v) = second else {
            panic!("expected a Frame");
        };
        assert_eq!(v["kind"], "artifact-update");
        assert_eq!(v["append"], true);
    }

    #[tokio::test]
    async fn send_message_streaming_raw_puts_metadata_on_the_message_not_params() {
        // Regression: this metadata used to land on `params.metadata`, a
        // sibling of `params.message` — but a receiving A2A server (weave2.0's
        // `_skill_id`, e.g.) reads the skill id off `params.message.metadata`,
        // the MESSAGE object it actually dispatches. With the old placement,
        // `skill: "generate-ui"` was silently invisible to the agent, which
        // fell back to its own default skill and rejected the request with
        // "unknown skill" — a real, live bug this pins down.
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/")
            .match_body(mockito::Matcher::PartialJson(serde_json::json!({
                "params": {"message": {"metadata": {"skill": "generate-ui"}}}
            })))
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body("data: {\"jsonrpc\":\"2.0\",\"id\":\"1\",\"result\":{\"kind\":\"artifact-update\",\"final\":true}}\n\n")
            .create_async()
            .await;

        let (tx, _rx) = tokio::sync::mpsc::channel(8);
        let client = A2aClient::new().with_metadata(serde_json::json!({"skill": "generate-ui"}));
        client
            .send_message_streaming_raw(&server.url(), "hi", Some("ctx-1"), tx, &[])
            .await
            .unwrap();
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn send_message_streaming_raw_relays_error_frames() {
        let mut server = mockito::Server::new_async().await;
        let body = "data: {\"jsonrpc\":\"2.0\",\"id\":\"1\",\"error\":{\"code\":-32000,\"message\":\"boom\"}}\n\n";
        let mock = server
            .mock("POST", "/")
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(body)
            .create_async()
            .await;

        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        let client = A2aClient::new();
        client
            .send_message_streaming_raw(&server.url(), "hi", Some("ctx-1"), tx, &[])
            .await
            .unwrap();
        mock.assert_async().await;

        match rx.recv().await.unwrap() {
            RawAgentFrame::Error(A2aClientError::A2aProtocol { code, message }) => {
                assert_eq!(code, -32000);
                assert_eq!(message, "boom");
            }
            other => panic!("expected an Error frame, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn send_message_streaming_raw_handles_a_non_streaming_json_response() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                serde_json::json!({
                    "jsonrpc": "2.0", "id": "1",
                    "result": {"kind": "artifact-update", "artifact": {"parts": [{"text": "hi"}]}},
                })
                .to_string(),
            )
            .create_async()
            .await;

        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        let client = A2aClient::new();
        client
            .send_message_streaming_raw(&server.url(), "hi", Some("ctx-1"), tx, &[])
            .await
            .unwrap();
        mock.assert_async().await;

        let RawAgentFrame::Frame(v) = rx.recv().await.unwrap() else {
            panic!("expected a Frame");
        };
        assert_eq!(v["artifact"]["parts"][0]["text"], "hi");
    }

    #[test]
    fn distinct_artifacts_are_newline_separated() {
        let result = serde_json::json!({
            "task": {
                "artifacts": [
                    {"artifactId": "a1", "parts": [{"text": "one"}]},
                    {"artifactId": "a2", "parts": [{"text": "two"}]}
                ]
            }
        });
        assert_eq!(
            A2aClient::extract_text_from_value(&result).as_deref(),
            Some("one\ntwo")
        );
    }

    // A statusUpdate/task-in-progress pause must never resolve to SendOutcome::Text — that's the
    // exact misclassification that lets a sub-agent's question get fed back to an LLM as if it
    // were the answer.

    #[test]
    fn extract_task_and_context_id_reads_status_update_shape() {
        let event = serde_json::json!({"result": {"statusUpdate": {
            "taskId": "task-123",
            "contextId": "ctx-456",
            "status": {"state": "TASK_STATE_INPUT_REQUIRED"}
        }}});
        assert_eq!(
            A2aClient::extract_task_and_context_id(&event, "sent-ctx"),
            ("task-123".to_string(), "ctx-456".to_string())
        );
    }

    #[test]
    fn extract_task_and_context_id_reads_full_task_snapshot_shape() {
        let event = serde_json::json!({"result": {"task": {
            "id": "task-789",
            "contextId": "ctx-999",
            "status": {"state": "TASK_STATE_AUTH_REQUIRED"}
        }}});
        assert_eq!(
            A2aClient::extract_task_and_context_id(&event, "sent-ctx"),
            ("task-789".to_string(), "ctx-999".to_string())
        );
    }

    #[test]
    fn extract_task_and_context_id_falls_back_to_sent_context_id() {
        // The agent doesn't echo a contextId — resuming against the id we sent is still the
        // correct conversation, since the agent had no way to change what we already told it.
        let event = serde_json::json!({"result": {"statusUpdate": {
            "taskId": "task-123",
            "status": {"state": "TASK_STATE_INPUT_REQUIRED"}
        }}});
        assert_eq!(
            A2aClient::extract_task_and_context_id(&event, "sent-ctx"),
            ("task-123".to_string(), "sent-ctx".to_string())
        );
    }

    #[tokio::test]
    async fn streamed_input_required_yields_awaiting_human_never_text() {
        let mut server = mockito::Server::new_async().await;
        let sse_body = concat!(
            "data: {\"result\":{\"statusUpdate\":{\"taskId\":\"task-123\",\"contextId\":\"ctx-456\",",
            "\"status\":{\"state\":\"TASK_STATE_INPUT_REQUIRED\",\"message\":{\"parts\":",
            "[{\"text\":\"Which repository?\"}],\"metadata\":{\"expected_input\":\"free_text\"}}}}}}\n\n",
        );
        let mock = server
            .mock("POST", "/")
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(sse_body)
            .create_async()
            .await;

        let client = A2aClient::new();
        let result = client
            .send_message_streaming(&server.url(), "hi", Some("sent-ctx"), None, &[], &[])
            .await;

        mock.assert_async().await;
        assert_eq!(
            result.unwrap(),
            SendOutcome::AwaitingHuman(PauseInfo {
                kind: nasiko_types::a2a::AwaitingHumanKind::InputRequired,
                message: "Which repository?".into(),
                task_id: "task-123".into(),
                context_id: "ctx-456".into(),
                metadata: serde_json::json!({"expected_input": "free_text"}),
            })
        );
    }

    #[tokio::test]
    async fn non_streaming_input_required_reply_also_yields_awaiting_human() {
        // An agent that never opens an event stream is not exempt from the same task-lifecycle
        // states — the plain-JSON fallback path must classify a pause exactly like the SSE path.
        let mut server = mockito::Server::new_async().await;
        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": "1",
            "result": {"task": {
                "id": "task-321",
                "contextId": "ctx-654",
                "status": {
                    "state": "TASK_STATE_INPUT_REQUIRED",
                    "message": {"parts": [{"text": "Which repository?"}]}
                }
            }}
        });
        let mock = server
            .mock("POST", "/")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(body.to_string())
            .create_async()
            .await;

        let client = A2aClient::new();
        let result = client
            .send_message_streaming(&server.url(), "hi", Some("sent-ctx"), None, &[], &[])
            .await;

        mock.assert_async().await;
        assert_eq!(
            result.unwrap(),
            SendOutcome::AwaitingHuman(PauseInfo {
                kind: nasiko_types::a2a::AwaitingHumanKind::InputRequired,
                message: "Which repository?".into(),
                task_id: "task-321".into(),
                context_id: "ctx-654".into(),
                metadata: serde_json::Value::Null,
            })
        );
    }
}

#[derive(Debug, thiserror::Error)]
pub enum A2aClientError {
    #[error("network error: {0}")]
    Network(String),

    #[error("HTTP {0}: {1}")]
    Http(u16, String),

    #[error("invalid response: {0}")]
    InvalidResponse(String),

    #[error("A2A protocol error ({code}): {message}")]
    A2aProtocol { code: i32, message: String },
}

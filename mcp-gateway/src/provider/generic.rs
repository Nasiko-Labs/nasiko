//! Generic streamable-HTTP MCP client.
//!
//! Faithful Rust port of the PoC's `tool_aggregator.call_backend`. This is the
//! shared transport the gateway uses to talk to **any** MCP JSON-RPC endpoint —
//! a generic third-party server (SerpAPI, Firecrawl, …) *and* the per-user
//! Composio Tool Router session URL, which is itself just an MCP endpoint. It is
//! not tied to any auth type: credentials are already baked into
//! `MCPServerConfig.headers` / `.url` by the session resolver before we get here.
//!
//! MCP's streamable-HTTP transport may answer a single JSON-RPC request with
//! either `application/json` or a `text/event-stream` (SSE) body carrying one
//! `data:` frame — this client handles both, exactly like the PoC.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};

use crate::error::{McpError, Result};
use crate::types::MCPServerConfig;

/// Protocol version advertised in the MCP `initialize` handshake. Servers that
/// only speak an older revision negotiate down in their response.
const MCP_PROTOCOL_VERSION: &str = "2025-06-18";

/// Default per-request timeout for a `tools/call` forward.
pub const DEFAULT_CALL_TIMEOUT: Duration = Duration::from_secs(60);
/// Shorter timeout for `tools/list` fan-out so one slow backend can't stall the
/// whole aggregation.
pub const LIST_TIMEOUT: Duration = Duration::from_secs(10);
/// Cap on a backend response body — a misbehaving/malicious backend must not be
/// able to force the gateway to buffer an unbounded body into memory.
const MAX_RESPONSE_BYTES: usize = 10 * 1024 * 1024;

/// Read a response body into a string, rejecting anything over [`MAX_RESPONSE_BYTES`]
/// (checks the advertised length first, then enforces while streaming since
/// Content-Length may be absent or dishonest).
async fn read_capped(mut resp: reqwest::Response, server_name: &str) -> Result<String> {
    if let Some(len) = resp.content_length()
        && len as usize > MAX_RESPONSE_BYTES
    {
        return Err(McpError::Backend(format!("backend '{server_name}' response too large ({len} bytes)")));
    }
    let mut buf: Vec<u8> = Vec::new();
    while let Some(chunk) = resp.chunk().await? {
        if buf.len() + chunk.len() > MAX_RESPONSE_BYTES {
            return Err(McpError::Backend(format!(
                "backend '{server_name}' response exceeded {MAX_RESPONSE_BYTES} bytes"
            )));
        }
        buf.extend_from_slice(&chunk);
    }
    String::from_utf8(buf).map_err(|_| McpError::Backend(format!("backend '{server_name}' returned invalid UTF-8")))
}

/// Outcome of one HTTP attempt: a parsed response, or a signal that the backend
/// needs an MCP session we haven't negotiated yet.
enum Attempt {
    Done(Value),
    NeedsSession,
}

#[derive(Clone)]
pub struct GenericMcpProvider {
    http: reqwest::Client,
    /// Negotiated `Mcp-Session-Id` per backend URL. In-process (not Redis): an
    /// MCP session is bound to the node that initialized it, so sharing it across
    /// horizontally-scaled nodes would be wrong — each node negotiates its own,
    /// re-negotiating on demand if a cached one goes stale.
    sessions: Arc<Mutex<HashMap<String, String>>>,
}

impl GenericMcpProvider {
    pub fn new(http: reqwest::Client) -> Self {
        Self { http, sessions: Arc::new(Mutex::new(HashMap::new())) }
    }

    /// POST a JSON-RPC request to an MCP backend and return the parsed response.
    ///
    /// Handles both transport modes transparently: a **stateless** server works
    /// on the first attempt (no extra round-trip — identical to the bare call);
    /// a **stateful** server (the MCP SDK default, which rejects calls lacking
    /// `Mcp-Session-Id`) triggers a one-time `initialize` handshake whose session
    /// id is then cached per backend and reused. A stale session (server restart)
    /// re-negotiates automatically.
    ///
    /// Errors (transport failure, non-2xx, empty stream, bad JSON) are returned
    /// as [`McpError::Backend`] / transport errors so callers can decide whether
    /// to skip the backend (aggregation) or surface the failure (single call).
    pub async fn request(
        &self,
        server: &MCPServerConfig,
        body: &Value,
        timeout: Duration,
        traceparent: Option<&str>,
    ) -> Result<Value> {
        let cached = self.sessions.lock().unwrap().get(&server.url).cloned();
        // Fast path: stateless backends (and already-negotiated sessions) return
        // here on the first attempt, exactly like the original bare request.
        if let Attempt::Done(v) = self.send_once(server, body, timeout, traceparent, cached.as_deref()).await? {
            return Ok(v);
        }
        // Stateful backend with no valid session yet: initialize, cache, retry once.
        let session_id = self.initialize_session(server, timeout, traceparent).await?;
        if let Some(sid) = &session_id {
            self.sessions.lock().unwrap().insert(server.url.clone(), sid.clone());
        }
        match self.send_once(server, body, timeout, traceparent, session_id.as_deref()).await? {
            Attempt::Done(v) => Ok(v),
            Attempt::NeedsSession => Err(McpError::Backend(format!(
                "backend '{}' still requires a session after initialize",
                server.name
            ))),
        }
    }

    /// One HTTP attempt. Returns [`Attempt::NeedsSession`] (instead of erroring)
    /// when the backend rejects the call for lack of a valid MCP session, so
    /// [`request`](Self::request) can negotiate one and retry.
    async fn send_once(
        &self,
        server: &MCPServerConfig,
        body: &Value,
        timeout: Duration,
        traceparent: Option<&str>,
        session_id: Option<&str>,
    ) -> Result<Attempt> {
        // `.json()` sets the body and Content-Type. We add Accept for both
        // response encodings, then layer the per-server auth headers on top.
        let mut req = self
            .http
            .post(&server.url)
            .timeout(timeout)
            .header("Accept", "application/json, text/event-stream")
            .json(body);
        // Propagate the agent's inbound trace context to the backend so a tool
        // call shows up in the same distributed trace, not as an orphan span.
        if let Some(tp) = traceparent {
            req = req.header("traceparent", tp);
        }
        if let Some(sid) = session_id {
            req = req.header("Mcp-Session-Id", sid);
        }
        for (k, v) in &server.headers {
            req = req.header(k.as_str(), v.as_str());
        }

        let resp = req.send().await?;
        let status = resp.status();
        let content_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        let text = read_capped(resp, &server.name).await?;

        if !status.is_success() {
            // A stateful backend answers a session-less (or stale-session) call
            // with 400/404 mentioning "session" — signal a re-negotiation rather
            // than surfacing it as a hard error.
            let code = status.as_u16();
            if (code == 400 || code == 404) && text.to_lowercase().contains("session") {
                return Ok(Attempt::NeedsSession);
            }
            return Err(McpError::Backend(format!(
                "backend '{}' returned HTTP {}: {}",
                server.name,
                code,
                truncate(&text, 200),
            )));
        }

        Ok(Attempt::Done(parse_jsonrpc(&content_type, &text, &server.name)?))
    }

    /// Perform the MCP `initialize` handshake, returning the negotiated
    /// `Mcp-Session-Id` (None for a stateless server that issues none). Sends the
    /// spec-required `notifications/initialized` follow-up when a session exists.
    async fn initialize_session(
        &self,
        server: &MCPServerConfig,
        timeout: Duration,
        traceparent: Option<&str>,
    ) -> Result<Option<String>> {
        let init = json!({
            "jsonrpc": "2.0",
            "id": "init",
            "method": "initialize",
            "params": {
                "protocolVersion": MCP_PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": {"name": "nasiko-mcp-gateway", "version": env!("CARGO_PKG_VERSION")},
            },
        });

        let mut req = self
            .http
            .post(&server.url)
            .timeout(timeout)
            .header("Accept", "application/json, text/event-stream")
            .json(&init);
        if let Some(tp) = traceparent {
            req = req.header("traceparent", tp);
        }
        for (k, v) in &server.headers {
            req = req.header(k.as_str(), v.as_str());
        }

        let resp = req.send().await?;
        let status = resp.status();
        let session_id = resp
            .headers()
            .get("mcp-session-id")
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);
        let text = read_capped(resp, &server.name).await?;
        if !status.is_success() {
            return Err(McpError::Backend(format!(
                "backend '{}' initialize failed HTTP {}: {}",
                server.name,
                status.as_u16(),
                truncate(&text, 200),
            )));
        }

        // Complete the handshake so the server marks the session ready (spec
        // requires this before normal requests). Best-effort — it returns 202
        // with no body — and only meaningful when a session was actually issued.
        if let Some(sid) = &session_id {
            let note = json!({"jsonrpc": "2.0", "method": "notifications/initialized"});
            let mut n = self
                .http
                .post(&server.url)
                .timeout(timeout)
                .header("Accept", "application/json, text/event-stream")
                .header("Mcp-Session-Id", sid)
                .json(&note);
            if let Some(tp) = traceparent {
                n = n.header("traceparent", tp);
            }
            for (k, v) in &server.headers {
                n = n.header(k.as_str(), v.as_str());
            }
            let _ = n.send().await;
        }

        Ok(session_id)
    }

    /// Fetch a backend's tool list. Returns the raw `result.tools` array
    /// (each tool preserved verbatim so the aggregator can namespace `name`).
    pub async fn list_tools(
        &self,
        server: &MCPServerConfig,
        timeout: Duration,
        traceparent: Option<&str>,
    ) -> Result<Vec<Value>> {
        let body = json!({"jsonrpc": "2.0", "method": "tools/list", "id": 1, "params": {}});
        let resp = self.request(server, &body, timeout, traceparent).await?;
        let tools = resp
            .get("result")
            .and_then(|r| r.get("tools"))
            .and_then(|t| t.as_array())
            .cloned()
            .unwrap_or_default();
        Ok(tools)
    }

    /// Forward a `tools/call` to a backend and return its full JSON-RPC response
    /// (the gateway passes the backend's result straight through to the agent).
    pub async fn call_tool(
        &self,
        server: &MCPServerConfig,
        req_id: &Value,
        tool_name: &str,
        arguments: &Value,
        timeout: Duration,
        traceparent: Option<&str>,
    ) -> Result<Value> {
        let body = json!({
            "jsonrpc": "2.0",
            "id": req_id,
            "method": "tools/call",
            "params": {"name": tool_name, "arguments": arguments},
        });
        self.request(server, &body, timeout, traceparent).await
    }
}

/// Parse an MCP response body into a JSON value. The streamable-HTTP transport
/// may answer with either `application/json` or a `text/event-stream` (SSE) body
/// carrying one `data:` JSON-RPC frame — handle both.
fn parse_jsonrpc(content_type: &str, text: &str, server_name: &str) -> Result<Value> {
    if content_type.contains("text/event-stream") {
        // Read SSE frames until the first non-empty `data:` payload.
        for raw in text.lines() {
            let line = raw.trim();
            if let Some(data) = line.strip_prefix("data:") {
                let data = data.trim();
                if !data.is_empty() && data != "[DONE]" {
                    return Ok(serde_json::from_str(data)?);
                }
            }
        }
        return Err(McpError::Backend(format!(
            "backend '{server_name}' returned an empty event-stream"
        )));
    }
    Ok(serde_json::from_str(text)?)
}

/// Truncate on a char boundary for safe error messages.
fn truncate(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

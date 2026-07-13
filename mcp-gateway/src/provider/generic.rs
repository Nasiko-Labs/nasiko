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

use std::time::Duration;

use serde_json::{Value, json};

use crate::error::{McpError, Result};
use crate::types::MCPServerConfig;

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

#[derive(Clone)]
pub struct GenericMcpProvider {
    http: reqwest::Client,
}

impl GenericMcpProvider {
    pub fn new(http: reqwest::Client) -> Self {
        Self { http }
    }

    /// POST a JSON-RPC request to an MCP backend and return the parsed response.
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
            return Err(McpError::Backend(format!(
                "backend '{}' returned HTTP {}: {}",
                server.name,
                status.as_u16(),
                truncate(&text, 200),
            )));
        }

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
                "backend '{}' returned an empty event-stream",
                server.name,
            )));
        }

        Ok(serde_json::from_str(&text)?)
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

/// Truncate on a char boundary for safe error messages.
fn truncate(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

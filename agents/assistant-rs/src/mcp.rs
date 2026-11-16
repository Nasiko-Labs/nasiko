//! MCP gateway client (env: `MCP_GATEWAY_URL`, `MCP_GATEWAY_TOKEN`).
//!
//! The calling user is resolved by the gateway from the forwarded `traceparent`; the tool
//! list is query-aware, so one [`McpSession`] is opened per A2A request and grows whenever
//! the model calls `nasiko_search_tools`.

use std::collections::HashSet;
use std::sync::Arc;

use serde_json::{Value, json};

/// Gateway tool-search meta-tool; its result carries full tool definitions.
pub const SEARCH_TOOL: &str = "nasiko_search_tools";

/// JSON-RPC code the gateway returns when a tool requires HITL confirmation.
pub const TOOL_ASK_CODE: i64 = -32001;
/// JSON-RPC code the gateway returns when the user must connect an account.
pub const AUTH_REQUIRED_CODE: i64 = -32002;

/// The user must connect (or reconnect) an account before the tool can run.
#[derive(Debug, Clone, PartialEq)]
pub struct AuthRequired {
    /// Toolkit / connector slug, e.g. `gmail`.
    pub connector: String,
    pub connector_id: Option<String>,
    /// Pending HITL row the gateway already recorded; links the agent's pause to it.
    pub hitl_request_id: Option<String>,
    /// Composio connect link, when the gateway minted one.
    pub auth_url: Option<String>,
    pub message: String,
}

impl AuthRequired {
    /// Parses a JSON-RPC `error` object; `None` unless it is an `AUTH_REQUIRED` refusal.
    pub fn from_rpc_error(err: &Value) -> Option<Self> {
        if err.get("code").and_then(Value::as_i64) != Some(AUTH_REQUIRED_CODE) {
            return None;
        }
        let data: &Value = err.get("data").unwrap_or(&Value::Null);
        let text = |key: &str| -> Option<String> {
            data.get(key)
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
        };
        let connector: String = text("connector").unwrap_or_else(|| "account".to_owned());
        let message: String = err
            .get("message")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| format!("Connect {connector} to continue."));
        Some(Self {
            connector,
            connector_id: text("connector_id"),
            hitl_request_id: text("hitl_request_id"),
            auth_url: text("auth_url"),
            message,
        })
    }
}

/// The user must approve a gated tool call before it can run.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolAsk {
    pub hitl_request_id: Option<String>,
    pub message: String,
}

impl ToolAsk {
    pub fn from_rpc_error(err: &Value) -> Option<Self> {
        if err.get("code").and_then(Value::as_i64) != Some(TOOL_ASK_CODE) {
            return None;
        }
        let hitl_request_id = err
            .get("data")
            .and_then(|d| d.get("hitl_request_id"))
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_owned);
        let message = err
            .get("message")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| "Approval required to run this tool.".to_owned());
        Some(Self {
            hitl_request_id,
            message,
        })
    }
}

/// Result of one gateway tool call.
#[derive(Debug, Clone, PartialEq)]
pub enum ToolOutcome {
    /// Text for the model (errors included, so it can recover).
    Text(String),
    /// Pause the task until the user connects the account.
    AuthRequired(AuthRequired),
    /// Pause the task until the user approves the tool call.
    ToolAsk(ToolAsk),
}

#[derive(Debug)]
enum RpcError {
    Auth(AuthRequired),
    ToolAsk(ToolAsk),
    Other(String),
}

/// Thin JSON-RPC client; holds no tool state.
pub struct McpClient {
    url: String,
    token: String,
    http: reqwest::Client,
}

impl McpClient {
    /// `None` when the platform did not inject the gateway env.
    pub fn from_env() -> Option<Self> {
        let url: Option<String> = std::env::var("MCP_GATEWAY_URL")
            .ok()
            .filter(|u| !u.is_empty());
        let token: Option<String> = std::env::var("MCP_GATEWAY_TOKEN")
            .ok()
            .filter(|t| !t.is_empty());
        match (url, token) {
            (Some(url), Some(token)) => Some(Self {
                url,
                token,
                http: reqwest::Client::new(),
            }),
            _ => {
                tracing::info!("mcp: MCP_GATEWAY_URL/MCP_GATEWAY_TOKEN not set — no gateway tools");
                None
            }
        }
    }

    async fn rpc(&self, traceparent: &str, body: Value) -> Result<Value, RpcError> {
        let mut req = self
            .http
            .post(&self.url)
            .bearer_auth(&self.token)
            .json(&body);
        if !traceparent.is_empty() {
            req = req.header("traceparent", traceparent);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| RpcError::Other(e.to_string()))?;
        let status = resp.status();
        let raw: String = resp
            .text()
            .await
            .map_err(|e| RpcError::Other(e.to_string()))?;
        if !status.is_success() {
            return Err(RpcError::Other(format!(
                "gateway HTTP {}: {raw}",
                status.as_u16()
            )));
        }
        let parsed: Value = serde_json::from_str(&raw)
            .map_err(|e| RpcError::Other(format!("gateway response parse: {e}")))?;
        if let Some(err) = parsed.get("error").filter(|e| !e.is_null()) {
            if let Some(auth) = AuthRequired::from_rpc_error(err) {
                return Err(RpcError::Auth(auth));
            }
            if let Some(ask) = ToolAsk::from_rpc_error(err) {
                return Err(RpcError::ToolAsk(ask));
            }
            return Err(RpcError::Other(format!("gateway error: {err}")));
        }
        Ok(parsed.get("result").cloned().unwrap_or_else(|| json!({})))
    }

    /// Startup health check (without traceparent the gateway lists only the search tool).
    pub async fn probe(&self) {
        match self
            .rpc(
                "",
                json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}),
            )
            .await
        {
            Ok(_) => tracing::info!("mcp: gateway reachable"),
            Err(e) => tracing::warn!(error = ?e, "mcp: gateway unreachable at startup"),
        }
    }
}

/// Gateway tools for one A2A request.
pub struct McpSession {
    client: Arc<McpClient>,
    traceparent: String,
    defs: Vec<Value>,
    names: HashSet<String>,
}

impl McpSession {
    pub async fn open(client: Arc<McpClient>, traceparent: &str) -> Self {
        let mut session = Self {
            client,
            traceparent: traceparent.to_owned(),
            defs: Vec::new(),
            names: HashSet::new(),
        };
        let list = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"});
        match session.client.rpc(&session.traceparent, list).await {
            Ok(result) => {
                session.absorb(&result);
            }
            Err(e) => tracing::warn!(error = ?e, "mcp: tools/list failed"),
        }
        session
    }

    /// OpenAI function-shaped definitions to advertise.
    pub fn tool_defs(&self) -> &[Value] {
        &self.defs
    }

    pub fn handles(&self, name: &str) -> bool {
        self.names.contains(name)
    }

    /// Adds new `tools[]` entries; returns all tool names returned by the search.
    pub fn absorb(&mut self, result: &Value) -> Vec<String> {
        let mut found: Vec<String> = Vec::new();
        let tools_opt = result
            .get("structuredContent")
            .and_then(|sc| sc.get("tools"))
            .or_else(|| result.get("tools"))
            .and_then(Value::as_array);
        let Some(tools) = tools_opt else {
            return found;
        };
        for tool in tools {
            let Some(name) = tool.get("name").and_then(Value::as_str) else {
                continue;
            };
            found.push(name.to_owned());
            if !self.names.insert(name.to_owned()) {
                continue;
            }
            let parameters: Value = tool
                .get("inputSchema")
                .filter(|s| s.is_object())
                .cloned()
                .unwrap_or_else(|| json!({"type": "object", "properties": {}}));
            self.defs.push(json!({
                "type": "function",
                "function": {
                    "name": name,
                    "description": tool.get("description").and_then(Value::as_str).unwrap_or(""),
                    "parameters": parameters,
                }
            }));
        }
        found
    }

    /// Runs one gateway tool. `AUTH_REQUIRED` comes back as [`ToolOutcome::AuthRequired`].
    pub async fn call_tool(&mut self, name: &str, args_json: &str) -> ToolOutcome {
        let arguments: Value = match serde_json::from_str(args_json) {
            Ok(a) => a,
            Err(e) => return ToolOutcome::Text(format!("bad arguments: {e}")),
        };
        let body = json!({
            "jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": {"name": name, "arguments": arguments},
        });
        let result: Value = match self.client.rpc(&self.traceparent, body).await {
            Ok(r) => r,
            Err(RpcError::Auth(auth)) => return ToolOutcome::AuthRequired(auth),
            Err(RpcError::ToolAsk(ask)) => return ToolOutcome::ToolAsk(ask),
            Err(RpcError::Other(e)) => {
                return ToolOutcome::Text(format!("tool {name} failed: {e}"));
            }
        };
        if name == SEARCH_TOOL {
            let found: Vec<String> = self.absorb(&result);
            return ToolOutcome::Text(if found.is_empty() {
                "No matching tools were found. Do not search again for the same thing.".to_owned()
            } else {
                format!(
                    "These tools are now callable: {}. Call the one you need.",
                    found.join(", ")
                )
            });
        }
        ToolOutcome::Text(result_text(&result))
    }

    /// System-prompt fragment explaining the connected-account tools.
    pub fn system_prompt_hint(&self) -> String {
        if self.defs.is_empty() {
            return String::new();
        }
        format!(
            "\n\nConnected-account tools (MCP gateway) reach the user's OWN accounts (email, \
             calendar, chat, issues) and return real data. For any question about the user's \
             data you MUST use them; call `{SEARCH_TOOL}` to find a tool, then call the tool it \
             returns. Never search twice for the same capability. If a tool needs the user to \
             connect an account, the platform pauses the task and shows a Connect button — \
             never invent a link."
        )
    }
}

/// First non-empty MCP text content, else the raw JSON.
pub fn result_text(result: &Value) -> String {
    result
        .get("content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|p| p.get("type").and_then(Value::as_str) == Some("text"))
        .filter_map(|p| p.get("text").and_then(Value::as_str))
        .find(|t| !t.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| result.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_auth_required_with_link() {
        let err = json!({"code": -32002, "message": "Connect gmail", "data": {
            "connector": "gmail", "connector_id": "c1", "hitl_request_id": "h1",
            "auth_url": "https://connect.composio.dev/x"}});
        let auth: AuthRequired = AuthRequired::from_rpc_error(&err).unwrap();
        assert_eq!(auth.connector, "gmail");
        assert_eq!(auth.hitl_request_id.as_deref(), Some("h1"));
        assert_eq!(
            auth.auth_url.as_deref(),
            Some("https://connect.composio.dev/x")
        );
    }

    #[test]
    fn other_errors_are_not_auth() {
        assert!(AuthRequired::from_rpc_error(&json!({"code": -32603, "message": "x"})).is_none());
    }

    #[test]
    fn empty_auth_url_is_none() {
        let err = json!({"code": -32002, "data": {"connector": "gmail", "auth_url": ""}});
        let auth: AuthRequired = AuthRequired::from_rpc_error(&err).unwrap();
        assert_eq!(auth.auth_url, None);
        assert_eq!(auth.message, "Connect gmail to continue.");
    }

    #[test]
    fn result_text_prefers_text_content() {
        let r = json!({"content": [{"type": "text", "text": ""}, {"type": "text", "text": "hi"}]});
        assert_eq!(result_text(&r), "hi");
    }
}

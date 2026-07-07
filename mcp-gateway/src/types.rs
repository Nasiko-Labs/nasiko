//! Core MCP protocol and domain types shared across the gateway.
//!
//! These are pure data types with no I/O — the JSON-RPC 2.0 envelope the agent
//! speaks, the backend/auth descriptors the session resolver builds, and the
//! per-agent permission stance. Nothing here touches Postgres, Redis, or HTTP.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// MCP protocol version advertised in the `initialize` handshake. Matches the
/// version the PoC negotiated and the streamable-HTTP transport expects.
pub const PROTOCOL_VERSION: &str = "2024-11-05";

// ─── JSON-RPC 2.0 envelope ──────────────────────────────────────────────────

/// An inbound JSON-RPC request from the agent. A missing `id` denotes a
/// notification (no response expected).
#[derive(Debug, Clone, Deserialize)]
pub struct JsonRpcRequest {
    #[serde(default = "default_jsonrpc")]
    pub jsonrpc: String,
    #[serde(default)]
    pub id: Option<Value>,
    pub method: String,
    #[serde(default)]
    pub params: Option<Value>,
}

fn default_jsonrpc() -> String {
    "2.0".to_string()
}

/// A JSON-RPC error object.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcError {
    pub code: i64,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl JsonRpcError {
    pub fn new(code: i64, message: impl Into<String>) -> Self {
        Self { code, message: message.into(), data: None }
    }
}

/// A JSON-RPC response. Exactly one of `result` / `error` is populated.
#[derive(Debug, Clone, Serialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: &'static str,
    pub id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

impl JsonRpcResponse {
    /// A successful response carrying `result`.
    pub fn ok(id: Value, result: Value) -> Self {
        Self { jsonrpc: "2.0", id, result: Some(result), error: None }
    }

    /// An error response.
    pub fn err(id: Value, error: JsonRpcError) -> Self {
        Self { jsonrpc: "2.0", id, result: None, error: Some(error) }
    }
}

/// Standard JSON-RPC / MCP error codes used throughout the gateway.
pub mod codes {
    /// Malformed JSON.
    pub const PARSE_ERROR: i64 = -32700;
    /// Method not recognised.
    pub const METHOD_NOT_FOUND: i64 = -32601;
    /// Invalid params / unroutable tool name.
    pub const INVALID_PARAMS: i64 = -32602;
    /// Internal server / upstream error.
    pub const INTERNAL_ERROR: i64 = -32603;
    /// Tool blocked for this agent (permission stance = block).
    pub const TOOL_BLOCKED: i64 = -32000;
    /// Tool requires user approval (permission stance = ask).
    pub const TOOL_ASK: i64 = -32001;
}

// ─── Backend / auth descriptors ─────────────────────────────────────────────

/// How the gateway authenticates to a generic (non-Composio) backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AuthType {
    #[serde(rename = "none")]
    None,
    #[serde(rename = "bearer")]
    Bearer,
    #[serde(rename = "basic")]
    Basic,
    #[serde(rename = "oauth2")]
    OAuth2,
    #[serde(rename = "url_param")]
    UrlParam,
}

impl AuthType {
    /// Canonical string form, matching the DB `auth_type` column values.
    pub fn as_str(&self) -> &'static str {
        match self {
            AuthType::None => "none",
            AuthType::Bearer => "bearer",
            AuthType::Basic => "basic",
            AuthType::OAuth2 => "oauth2",
            AuthType::UrlParam => "url_param",
        }
    }

    /// Parse from the DB / API string form. Returns `Option` (not the fallible
    /// `FromStr` contract) since callers treat an unknown value as "skip".
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "none" => Some(AuthType::None),
            "bearer" => Some(AuthType::Bearer),
            "basic" => Some(AuthType::Basic),
            "oauth2" => Some(AuthType::OAuth2),
            "url_param" => Some(AuthType::UrlParam),
            _ => None,
        }
    }
}

/// The two kinds of tool backends the gateway aggregates over.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ServerType {
    /// The per-user Composio MCP session (meta-tools, keeps original names).
    Composio,
    /// A generic streamable-HTTP MCP server (tools namespaced `{server}__{tool}`).
    Mcp,
}

impl ServerType {
    pub fn as_str(&self) -> &'static str {
        match self {
            ServerType::Composio => "composio",
            ServerType::Mcp => "mcp",
        }
    }
}

/// A resolved backend the gateway will fan out to. The Composio session is
/// always entry `[0]` with `name = "composio"`; generic servers follow with
/// per-user credentials already injected into `headers` / `url`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MCPServerConfig {
    pub name: String,
    pub url: String,
    #[serde(default)]
    pub headers: HashMap<String, String>,
    #[serde(default = "default_transport")]
    pub transport: String,
}

fn default_transport() -> String {
    "streamable_http".to_string()
}

// ─── Permissions ────────────────────────────────────────────────────────────

/// Per-tool permission stance for an agent. Evaluation priority is
/// `block > ask > allow`; the default when no rule matches is `allow`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Stance {
    Allow,
    Ask,
    Block,
}

impl Stance {
    pub fn as_str(&self) -> &'static str {
        match self {
            Stance::Allow => "allow",
            Stance::Ask => "ask",
            Stance::Block => "block",
        }
    }

    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "allow" => Some(Stance::Allow),
            "ask" => Some(Stance::Ask),
            "block" => Some(Stance::Block),
            _ => None,
        }
    }
}

// ─── Tools ──────────────────────────────────────────────────────────────────

/// One tool entry as returned by a backend's `tools/list`. MCP tools carry
/// arbitrary provider-specific fields (`inputSchema`, `annotations`, …), so
/// everything except `name` is preserved verbatim in `rest`. This lets the
/// aggregator rewrite only the `name` (for namespacing) and re-serialize the
/// tool unchanged.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolInfo {
    pub name: String,
    #[serde(flatten)]
    pub rest: Map<String, Value>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn auth_type_round_trips_and_rejects_unknown() {
        for (v, s) in [
            (AuthType::None, "none"), (AuthType::Bearer, "bearer"), (AuthType::Basic, "basic"),
            (AuthType::OAuth2, "oauth2"), (AuthType::UrlParam, "url_param"),
        ] {
            assert_eq!(v.as_str(), s);
            assert_eq!(AuthType::from_str(s), Some(v));
            assert_eq!(serde_json::to_value(v).unwrap(), json!(s));
        }
        assert_eq!(AuthType::from_str("nope"), None);
        assert_eq!(AuthType::from_str(""), None);
    }

    #[test]
    fn stance_and_server_type_round_trip() {
        assert_eq!(Stance::from_str("allow"), Some(Stance::Allow));
        assert_eq!(Stance::from_str("block"), Some(Stance::Block));
        assert_eq!(Stance::from_str("bogus"), None);
        assert_eq!(serde_json::to_value(Stance::Ask).unwrap(), json!("ask"));
        assert_eq!(serde_json::to_value(ServerType::Composio).unwrap(), json!("composio"));
        assert_eq!(ServerType::Mcp.as_str(), "mcp");
    }

    #[test]
    fn tool_info_flatten_preserves_unknown_fields_and_renames_name() {
        let raw = json!({"name": "search", "description": "d", "inputSchema": {"type": "object"}, "annotations": {"x": 1}});
        let mut tool: ToolInfo = serde_json::from_value(raw).unwrap();
        assert_eq!(tool.name, "search");
        // Rename name (as the aggregator does) and re-serialize — extras survive.
        tool.name = "serpapi__search".into();
        let out = serde_json::to_value(&tool).unwrap();
        assert_eq!(out["name"], json!("serpapi__search"));
        assert_eq!(out["inputSchema"], json!({"type": "object"}));
        assert_eq!(out["annotations"], json!({"x": 1}));
    }

    #[test]
    fn jsonrpc_request_defaults_and_notification() {
        // Missing id (notification) + missing params tolerated; jsonrpc defaulted.
        let req: JsonRpcRequest = serde_json::from_str(r#"{"method":"ping"}"#).unwrap();
        assert_eq!(req.method, "ping");
        assert!(req.id.is_none());
        assert!(req.params.is_none());
        assert_eq!(req.jsonrpc, "2.0");
    }

    #[test]
    fn jsonrpc_response_omits_none_fields() {
        let ok = serde_json::to_value(JsonRpcResponse::ok(json!(1), json!({"a":1}))).unwrap();
        assert!(ok.get("result").is_some() && ok.get("error").is_none());
        let e = serde_json::to_value(JsonRpcResponse::err(json!(1), JsonRpcError::new(codes::TOOL_BLOCKED, "no"))).unwrap();
        assert!(e.get("error").is_some() && e.get("result").is_none());
        assert_eq!(e["error"]["code"], json!(codes::TOOL_BLOCKED));
    }
}

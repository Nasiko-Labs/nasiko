//! Core MCP protocol and domain types shared across the gateway.
//!
//! These are pure data types with no I/O — the JSON-RPC 2.0 envelope the agent
//! speaks, the backend/auth descriptors the session resolver builds, and the
//! per-agent permission stance. Nothing here touches Postgres, Redis, or HTTP.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use uuid::Uuid;

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
        Self {
            code,
            message: message.into(),
            data: None,
        }
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
        Self {
            jsonrpc: "2.0",
            id,
            result: Some(result),
            error: None,
        }
    }

    /// An error response.
    pub fn err(id: Value, error: JsonRpcError) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            result: None,
            error: Some(error),
        }
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
    /// A generic streamable-HTTP MCP server (tools namespaced by connector id).
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

/// DB discriminator for `mcp_connectors.provider_type`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProviderType {
    #[serde(rename = "composio")]
    Composio,
    #[serde(rename = "mcp_server")]
    McpServer,
}

impl ProviderType {
    pub fn as_str(&self) -> &'static str {
        match self {
            ProviderType::Composio => "composio",
            ProviderType::McpServer => "mcp_server",
        }
    }

    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "composio" => Some(ProviderType::Composio),
            "mcp_server" => Some(ProviderType::McpServer),
            _ => None,
        }
    }
}

/// Sharing grant type for `mcp_connector_grants.grant_type`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GrantType {
    User,
    Public,
}

impl GrantType {
    pub fn as_str(&self) -> &'static str {
        match self {
            GrantType::User => "user",
            GrantType::Public => "public",
        }
    }

    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "user" => Some(GrantType::User),
            "public" => Some(GrantType::Public),
            _ => None,
        }
    }
}

/// Sentinel `grantee_id` for a public ("everyone") grant.
pub const PUBLIC_GRANTEE: &str = "*";

/// One person with access to a connector, and why. `via` is one of
/// `"owner" | "direct" | "public" | "team" | "department"` — the last two are
/// EE-only (`via_label` carries the team/department name in that case).
/// Someone reachable through more than one path is still listed once, with
/// the most specific reason (owner > direct > team/department > public).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccessReason {
    pub user_id: Uuid,
    pub username: String,
    pub display_name: Option<String>,
    pub via: String,
    pub via_label: Option<String>,
}

/// A team or department with a direct grant on a connector — the entity
/// itself, not exploded per member (contrast [`AccessReason`], which lists
/// the individual people reachable through that grant). Feeds the
/// consumers view's Teams/Departments tables. OSS has no team/department
/// concept, so this is always empty outside EE.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrgGrantConsumer {
    pub id: Uuid,
    pub name: String,
    pub granted_by: Option<Uuid>,
    pub created_at: DateTime<Utc>,
}

/// A resolved backend the gateway will fan out to. The Composio session (when
/// present) is entry `[0]` with `kind = Composio`; generic servers follow with
/// per-user credentials already injected into `headers` / `url`. `connector_id`
/// is the routing key: generic tools are namespaced `{connector_prefix}__{tool}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MCPServerConfig {
    /// For generic servers, the connector id. For the Composio aggregate this is
    /// `Uuid::nil()` (its tools resolve to per-toolkit connectors separately).
    pub connector_id: Uuid,
    pub kind: ServerType,
    /// Display/log label only — never the routing key.
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

/// Tool-routing namespace prefix for a connector — first 16 hex of its id.
/// Derived from `id`, never `name`, so two owners sharing a display name don't
/// collide (fix #1). The 16-hex (64-bit) width makes an *id*-prefix collision
/// negligible platform-wide (fix #5) — negligible, not impossible: there is no
/// DB uniqueness constraint on the prefix, so this is a probabilistic bound, not
/// a hard guarantee. Widen further or add a uniqueness index if that ever matters.
pub fn connector_prefix(id: Uuid) -> String {
    let mut s = id.simple().to_string();
    s.truncate(16);
    s
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
            (AuthType::None, "none"),
            (AuthType::Bearer, "bearer"),
            (AuthType::Basic, "basic"),
            (AuthType::OAuth2, "oauth2"),
            (AuthType::UrlParam, "url_param"),
        ] {
            assert_eq!(v.as_str(), s);
            assert_eq!(AuthType::from_str(s), Some(v));
            assert_eq!(serde_json::to_value(v).unwrap(), json!(s));
        }
        assert_eq!(AuthType::from_str("nope"), None);
        assert_eq!(AuthType::from_str(""), None);
    }

    #[test]
    fn provider_and_grant_type_round_trip() {
        assert_eq!(
            ProviderType::from_str("composio"),
            Some(ProviderType::Composio)
        );
        assert_eq!(
            ProviderType::from_str("mcp_server"),
            Some(ProviderType::McpServer)
        );
        assert_eq!(ProviderType::from_str("nope"), None);
        assert_eq!(
            serde_json::to_value(ProviderType::McpServer).unwrap(),
            json!("mcp_server")
        );
        assert_eq!(GrantType::from_str("public"), Some(GrantType::Public));
        assert_eq!(GrantType::from_str(""), None);
        assert_eq!(
            serde_json::to_value(GrantType::User).unwrap(),
            json!("user")
        );
    }

    #[test]
    fn connector_prefix_is_16_hex_and_id_derived() {
        let a = Uuid::parse_str("11111111-2222-3333-4444-555555555555").unwrap();
        assert_eq!(connector_prefix(a), "1111111122223333");
        assert_eq!(connector_prefix(a).len(), 16);
        // Different ids → different prefixes; round-trips through `__` split.
        let b = Uuid::parse_str("abcdef00-2222-3333-4444-555555555555").unwrap();
        assert_ne!(connector_prefix(a), connector_prefix(b));
        let name = format!("{}__{}", connector_prefix(b), "SEND_EMAIL");
        assert_eq!(
            name.split_once("__"),
            Some(("abcdef0022223333", "SEND_EMAIL"))
        );
    }

    #[test]
    fn stance_and_server_type_round_trip() {
        assert_eq!(Stance::from_str("allow"), Some(Stance::Allow));
        assert_eq!(Stance::from_str("block"), Some(Stance::Block));
        assert_eq!(Stance::from_str("bogus"), None);
        assert_eq!(serde_json::to_value(Stance::Ask).unwrap(), json!("ask"));
        assert_eq!(
            serde_json::to_value(ServerType::Composio).unwrap(),
            json!("composio")
        );
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
        let e = serde_json::to_value(JsonRpcResponse::err(
            json!(1),
            JsonRpcError::new(codes::TOOL_BLOCKED, "no"),
        ))
        .unwrap();
        assert!(e.get("error").is_some() && e.get("result").is_none());
        assert_eq!(e["error"]["code"], json!(codes::TOOL_BLOCKED));
    }

    #[test]
    fn connector_prefix_is_always_16_lowercase_hex_chars() {
        let is_lower_hex_16 = |s: &str| {
            s.len() == 16
                && s.chars()
                    .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        };
        for id in [
            Uuid::nil(),
            Uuid::max(),
            Uuid::parse_str("ABCDEF12-2222-3333-4444-555555555555").unwrap(), // uppercase input UUID
            Uuid::new_v4(),
        ] {
            let p = connector_prefix(id);
            assert!(
                is_lower_hex_16(&p),
                "prefix '{p}' for {id} is not 16 lowercase hex chars"
            );
        }
        // Uppercase-hex UUID input still lowercases in the prefix.
        assert_eq!(
            connector_prefix(Uuid::parse_str("ABCDEF12-2222-3333-4444-555555555555").unwrap()),
            "abcdef1222223333"
        );
    }

    #[test]
    fn connector_prefix_depends_only_on_id_field_not_name_or_owner() {
        // `connector_prefix` takes only a `Uuid` — it cannot see name/owner. Two
        // "connectors" sharing an id but differing elsewhere yield the same prefix.
        struct Connector {
            id: Uuid,
            name: &'static str,
            owner: &'static str,
        }
        let shared_id = Uuid::new_v4();
        let mut c1 = Connector {
            id: shared_id,
            name: "gmail-prod",
            owner: "alice",
        };
        let c2 = Connector {
            id: shared_id,
            name: "totally-different-name",
            owner: "bob",
        };
        assert_eq!(connector_prefix(c1.id), connector_prefix(c2.id));

        let before = connector_prefix(c1.id);
        c1.name = "renamed-again";
        c1.owner = "carol";
        assert_eq!(connector_prefix(c1.id), before);
    }

    #[test]
    fn connector_prefix_collision_needs_a_full_8_byte_match_after_fix5() {
        // Fix #5 widened the prefix to the first 16 hex chars (8 bytes / 64 bits),
        // so a collision now requires two distinct ids to match on all 8 leading
        // bytes — far narrower than the old 4-byte (32-bit) surface. Constructed
        // deterministically to prove the prefix is exactly those 8 bytes.
        let a = Uuid::from_bytes([
            0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0, 0, 0, 0, 0, 0, 0, 0x01,
        ]);
        let b = Uuid::from_bytes([
            0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0, 0, 0, 0, 0, 0, 0, 0x02,
        ]);
        assert_ne!(a, b, "the two ids must be genuinely distinct connectors");
        assert_eq!(connector_prefix(a), connector_prefix(b));
        assert_eq!(connector_prefix(a), "1122334455667788");
        // Differing within the first 8 bytes must NOT collide.
        let c = Uuid::from_bytes([
            0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x89, 0, 0, 0, 0, 0, 0, 0, 0x01,
        ]);
        assert_ne!(connector_prefix(a), connector_prefix(c));
    }
}

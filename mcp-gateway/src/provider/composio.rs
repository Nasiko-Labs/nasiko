//! Composio Tool Router client — hand-built against the Composio v3 / v3.1 HTTP
//! API (there is no Rust SDK).
//!
//! Ports `composio_service.py` from the PoC, which used the Python SDK. The SDK
//! methods map to REST as:
//!
//! | PoC SDK call                              | REST endpoint                                   |
//! |-------------------------------------------|-------------------------------------------------|
//! | `client.auth_configs.create`             | `POST /api/v3/auth_configs`                     |
//! | `client.connected_accounts.link`         | `POST /api/v3/connected_accounts/link`          |
//! | `client.connected_accounts.list`         | `GET  /api/v3/connected_accounts`               |
//! | `composio.create()`                       | `POST /api/v3.1/tool_router/session`            |
//! | `composio.use(session_id)`               | `GET  /api/v3.1/tool_router/session/{id}`       |
//! | `sess.update(connected_accounts=…)`      | `PATCH /api/v3.1/tool_router/session/{id}`      |
//! | revoke                                    | `POST /api/v3.1/connected_accounts/{id}/revoke` |
//!
//! All requests authenticate with the `x-api-key` header.
//!
//! Response parsing is deliberately tolerant (`first_str` over candidate keys,
//! nested-`mcp` fallbacks) because the exact Tool Router JSON envelope is not
//! fully published — the same defensive stance the PoC took with `_extract_value`.
//! VERIFY-LIVE markers flag the request/response shapes to confirm against a real
//! `COMPOSIO_API_KEY` during end-to-end verification (plan §17/§20.6). The
//! `ToolProvider` trait isolates all of this so a shape fix touches only this file.

use std::collections::HashMap;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{Value, json};

use crate::error::{McpError, Result};

use super::{
    AuthConfigCreated, ComposioSession, ConnectedAccounts, ConnectionInitiated, ConnectionStatus,
    ToolDescriptor, ToolProvider, first_str, v_str,
};

const SESSION_TIMEOUT: Duration = Duration::from_secs(30);
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(15);

pub struct ComposioProvider {
    http: reqwest::Client,
    api_key: String,
    /// Base URL without a trailing slash (normalized by `McpConfig`).
    base_url: String,
}

impl ComposioProvider {
    pub fn new(http: reqwest::Client, api_key: String, base_url: String) -> Self {
        Self { http, api_key, base_url: base_url.trim_end_matches('/').to_string() }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url, path)
    }

    /// POST JSON to a Composio endpoint and parse the response body.
    async fn post_json(&self, path: &str, body: &Value, timeout: Duration) -> Result<Value> {
        let resp = self
            .http
            .post(self.url(path))
            .timeout(timeout)
            .header("x-api-key", &self.api_key)
            .json(body)
            .send()
            .await?;
        self.parse(path, resp).await
    }

    /// PATCH JSON to a Composio endpoint and parse the response body.
    async fn patch_json(&self, path: &str, body: &Value, timeout: Duration) -> Result<Value> {
        let resp = self
            .http
            .patch(self.url(path))
            .timeout(timeout)
            .header("x-api-key", &self.api_key)
            .json(body)
            .send()
            .await?;
        self.parse(path, resp).await
    }

    /// GET a Composio endpoint. Returns `Ok(None)` on 404 (missing resource),
    /// `Ok(Some(value))` on success, `Err` otherwise.
    async fn get_json_opt(
        &self,
        path: &str,
        query: &[(&str, &str)],
        timeout: Duration,
    ) -> Result<Option<Value>> {
        let resp = self
            .http
            .get(self.url(path))
            .timeout(timeout)
            .header("x-api-key", &self.api_key)
            .query(query)
            .send()
            .await?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        Ok(Some(self.parse(path, resp).await?))
    }

    /// Turn a response into JSON, mapping non-2xx to a `Composio` error with a
    /// truncated body for diagnostics.
    async fn parse(&self, path: &str, resp: reqwest::Response) -> Result<Value> {
        let status = resp.status();
        let text = resp.text().await?;
        if !status.is_success() {
            return Err(McpError::Composio(format!(
                "{} → HTTP {}: {}",
                path,
                status.as_u16(),
                truncate(&text, 300),
            )));
        }
        if text.trim().is_empty() {
            return Ok(Value::Null);
        }
        Ok(serde_json::from_str(&text)?)
    }

    /// Extract `session_id` + MCP `url` from a Tool Router session response and
    /// build the headers the gateway must send to that MCP url.
    ///
    /// Verified live: the response nests `mcp.url` (session_id top-level) and does
    /// **not** include headers, and the Tool Router MCP endpoint returns 401
    /// without an `x-api-key` header. So we always inject `x-api-key` here (any
    /// headers the API does return are preserved, then x-api-key layered on).
    fn parse_session(&self, v: &Value) -> Result<ComposioSession> {
        let session_id = first_str(v, &["session_id", "id", "nanoid"]).ok_or_else(|| {
            McpError::Composio("tool router session response missing session_id".to_string())
        })?;

        let mcp = v.get("mcp");
        let mcp_url = mcp
            .and_then(|m| v_str(m, "url"))
            .or_else(|| first_str(v, &["mcp_url", "url"]))
            .ok_or_else(|| {
                McpError::Composio("tool router session response missing mcp url".to_string())
            })?;

        let mut mcp_headers = mcp
            .and_then(|m| m.get("headers"))
            .or_else(|| v.get("mcp_headers"))
            .and_then(|h| h.as_object())
            .map(|obj| {
                obj.iter()
                    .filter_map(|(k, val)| val.as_str().map(|s| (k.clone(), s.to_string())))
                    .collect::<HashMap<_, _>>()
            })
            .unwrap_or_default();
        // The Tool Router MCP url authenticates via x-api-key (not embedded in the
        // url); the session response omits it, so inject it for the gateway.
        mcp_headers.insert("x-api-key".to_string(), self.api_key.clone());

        Ok(ComposioSession { session_id, mcp_url, mcp_headers })
    }
}

#[async_trait]
impl ToolProvider for ComposioProvider {
    async fn create_auth_config(
        &self,
        toolkit: &str,
        use_composio_managed: bool,
        client_id: Option<&str>,
        client_secret: Option<&str>,
        scopes: Option<&[String]>,
    ) -> Result<AuthConfigCreated> {
        let auth_config = if use_composio_managed {
            json!({ "type": "use_composio_managed_auth" })
        } else {
            let mut credentials = serde_json::Map::new();
            if let Some(id) = client_id {
                credentials.insert("client_id".into(), json!(id));
            }
            if let Some(secret) = client_secret {
                credentials.insert("client_secret".into(), json!(secret));
            }
            if let Some(sc) = scopes {
                credentials.insert("scopes".into(), json!(sc));
            }
            json!({
                "type": "use_custom_auth",
                "authScheme": "OAUTH2",
                "credentials": Value::Object(credentials),
            })
        };

        let body = json!({ "toolkit": { "slug": toolkit }, "auth_config": auth_config });
        let resp = self.post_json("/api/v3/auth_configs", &body, DEFAULT_TIMEOUT).await?;

        // Preferred shape nests the id under `auth_config`; fall back to top-level.
        let auth_config_id = resp
            .get("auth_config")
            .and_then(|a| first_str(a, &["id", "nanoid", "auth_config_id"]))
            .or_else(|| first_str(&resp, &["id", "auth_config_id", "nanoid"]))
            .ok_or_else(|| {
                McpError::Composio("auth_configs response missing auth config id".to_string())
            })?;

        Ok(AuthConfigCreated { auth_config_id })
    }

    async fn initiate_connection(
        &self,
        user_id: &str,
        auth_config_id: &str,
        callback_url: Option<&str>,
    ) -> Result<ConnectionInitiated> {
        let mut body = json!({ "auth_config_id": auth_config_id, "user_id": user_id });
        if let Some(cb) = callback_url {
            body["callback_url"] = json!(cb);
        }

        let resp = self
            .post_json("/api/v3/connected_accounts/link", &body, DEFAULT_TIMEOUT)
            .await?;

        // redirect_url may be top-level or nested under connection_data.
        let redirect_url = first_str(&resp, &["redirect_url", "redirectUrl"]).or_else(|| {
            resp.get("connection_data")
                .and_then(|d| first_str(d, &["redirect_url", "redirectUrl"]))
        });
        let status = first_str(&resp, &["status"]).unwrap_or_else(|| "INITIATED".to_string());

        Ok(ConnectionInitiated { redirect_url, status })
    }

    async fn check_connection_status(
        &self,
        user_id: &str,
        auth_config_id: &str,
    ) -> Result<ConnectionStatus> {
        let resp = self
            .get_json_opt(
                "/api/v3/connected_accounts",
                &[("user_ids", user_id), ("auth_config_ids", auth_config_id)],
                DEFAULT_TIMEOUT,
            )
            .await?;

        let Some(resp) = resp else {
            return Ok(ConnectionStatus { status: "NOT_FOUND".to_string(), account_id: None });
        };

        // Response is paginated: `{ items: [ { id, status, auth_config: { id } } ] }`.
        let items = resp.get("items").and_then(|i| i.as_array());
        if let Some(items) = items {
            for item in items {
                let item_ac_id = item
                    .get("auth_config")
                    .and_then(|a| first_str(a, &["id", "nanoid", "auth_config_id"]))
                    .or_else(|| first_str(item, &["auth_config_id"]));
                if item_ac_id.as_deref() == Some(auth_config_id) {
                    return Ok(ConnectionStatus {
                        status: first_str(item, &["status"]).unwrap_or_else(|| "UNKNOWN".to_string()),
                        account_id: first_str(item, &["id", "nanoid"]),
                    });
                }
            }
        }

        Ok(ConnectionStatus { status: "NOT_FOUND".to_string(), account_id: None })
    }

    async fn create_session(
        &self,
        user_id: &str,
        connected_accounts: &ConnectedAccounts,
    ) -> Result<ComposioSession> {
        // Tool Router session create. Verified live against the v3.1 API:
        //   - `manage_connections` must be an OBJECT, not a bool — so it is omitted
        //     (the API defaults it); sending `false` is a 400.
        //   - `connected_accounts` (a `{toolkit: [ca_id]}` object) is accepted and
        //     scopes the session; Composio otherwise resolves accounts by user_id.
        //   - Toolkits are intentionally omitted so the session is unrestricted.
        let mut body = json!({ "user_id": user_id });
        if !connected_accounts.is_empty() {
            body["connected_accounts"] = serde_json::to_value(connected_accounts)?;
        }

        let resp = self
            .post_json("/api/v3.1/tool_router/session", &body, SESSION_TIMEOUT)
            .await?;
        self.parse_session(&resp)
    }

    async fn reuse_session(&self, session_id: &str) -> Result<Option<ComposioSession>> {
        // `composio.use(session_id)`. Any failure (404, dead session, transport)
        // is treated as "session gone" so the caller recreates — matching the
        // PoC, which caught all exceptions and returned None.
        let path = format!("/api/v3.1/tool_router/session/{session_id}");
        match self.get_json_opt(&path, &[], SESSION_TIMEOUT).await {
            Ok(Some(resp)) => match self.parse_session(&resp) {
                Ok(session) => Ok(Some(session)),
                Err(e) => {
                    tracing::warn!(session_id, error = %e, "tool router session unparseable — treating as dead");
                    Ok(None)
                }
            },
            Ok(None) => Ok(None),
            Err(e) => {
                tracing::warn!(session_id, error = %e, "tool router session reuse failed — treating as dead");
                Ok(None)
            }
        }
    }

    async fn patch_session(
        &self,
        session_id: &str,
        connected_accounts: &ConnectedAccounts,
    ) -> Result<bool> {
        // VERIFY-LIVE: `sess.update(connected_accounts=…)`. Only connected
        // accounts are sent (never toolkits) so an unrestricted session stays
        // unrestricted. Failure → false so the caller recreates.
        let path = format!("/api/v3.1/tool_router/session/{session_id}");
        let body = json!({ "connected_accounts": connected_accounts });
        match self.patch_json(&path, &body, SESSION_TIMEOUT).await {
            Ok(_) => Ok(true),
            Err(e) => {
                tracing::info!(session_id, error = %e, "tool router session patch failed");
                Ok(false)
            }
        }
    }

    async fn revoke_connection(&self, connected_account_id: &str) -> Result<bool> {
        let path = format!("/api/v3.1/connected_accounts/{connected_account_id}/revoke");
        let resp = self
            .http
            .post(self.url(&path))
            .timeout(DEFAULT_TIMEOUT)
            .header("x-api-key", &self.api_key)
            .send()
            .await?;
        let ok = resp.status().is_success();
        if !ok {
            let code = resp.status().as_u16();
            let text = resp.text().await.unwrap_or_default();
            tracing::warn!(connected_account_id, code, body = %truncate(&text, 200), "composio revoke failed");
        }
        Ok(ok)
    }

    async fn list_toolkit_tools(&self, toolkit: &str) -> Result<Vec<ToolDescriptor>> {
        // GET /api/v3/tools?toolkit_slugs=<toolkit> → { items: [ { slug, description } ] }.
        // VERIFY-LIVE: confirm the tool name field is `slug` (fallback `name`).
        let resp = self
            .get_json_opt("/api/v3/tools", &[("toolkit_slugs", &toolkit.to_uppercase())], DEFAULT_TIMEOUT)
            .await?;
        let Some(resp) = resp else { return Ok(Vec::new()) };
        let items = resp.get("items").and_then(|i| i.as_array()).cloned().unwrap_or_default();
        Ok(items
            .iter()
            .filter_map(|it| {
                first_str(it, &["slug", "name"]).map(|name| ToolDescriptor {
                    name,
                    description: first_str(it, &["description"]),
                })
            })
            .collect())
    }
}

/// Truncate on a char boundary for safe error/log messages.
fn truncate(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

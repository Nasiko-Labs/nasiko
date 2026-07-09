//! Generic-server credential injection.
//!
//! Builds the list of generic (non-Composio) [`MCPServerConfig`] backends for a
//! user, injecting per-`auth_type` credentials. Faithful port of the PoC's
//! `session_resolver.build_servers_list`, with plaintext columns replaced by
//! `SecretsCrypto::for_user`-decrypted values.
//!
//! | auth_type   | injection                                                   |
//! |-------------|-------------------------------------------------------------|
//! | none        | nothing                                                     |
//! | bearer/basic| credential value → `Authorization` (or custom header)       |
//! | url_param   | credential value → `?{param}=…` appended to the URL         |
//! | oauth2      | `Authorization: Bearer <access_token>` (auto-refreshed)     |
//!
//! Two batch queries load all of the user's credentials + tokens up front (no
//! N+1). A server with no credential/token for the user is silently skipped.

use std::collections::HashMap;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use nasiko_secrets::SecretsCrypto;
use serde_json::Value;
use uuid::Uuid;

use crate::error::{McpError, Result};
use crate::oauth;
use crate::repo::{self, McpServer};
use crate::state::McpState;
use crate::types::MCPServerConfig;

/// Build the ordered list of generic backends for `user_id`, with credentials
/// injected. Servers lacking a required credential/token are skipped.
pub async fn build_generic_servers(
    state: &McpState,
    user_id: Uuid,
) -> Result<Vec<MCPServerConfig>> {
    let servers = repo::list_mcp_servers_for_user(&state.db, user_id).await?;

    // Composio-only user: skip the two credential/token batch queries entirely.
    if servers.is_empty() {
        return Ok(Vec::new());
    }

    // Batch-load all credentials + OAuth tokens for the user (no N+1).
    let creds = repo::get_user_credentials_for_user(&state.db, user_id).await?;
    let tokens = repo::get_mcp_oauth_tokens_for_user(&state.db, user_id).await?;
    let cred_by_server: HashMap<Uuid, _> = creds.into_iter().map(|c| (c.mcp_server_id, c)).collect();
    let token_by_server: HashMap<Uuid, _> =
        tokens.into_iter().map(|t| (t.mcp_server_id, t)).collect();

    let crypto = SecretsCrypto::for_user(user_id);
    let mut result = Vec::with_capacity(servers.len());

    for server in &servers {
        let mut headers = parse_headers(&server.headers);
        let mut url = server.url.clone();

        match server.auth_type.as_str() {
            "none" => {}

            "bearer" | "basic" => {
                match cred_by_server.get(&server.id) {
                    Some(cred) => {
                        let Some(value) = decrypt_or_skip(&crypto, &cred.credential_value, server)
                        else {
                            continue;
                        };
                        headers.insert(credential_header(server), value);
                    }
                    None if server.is_platform => {
                        // Platform bearer/basic server: user must register a credential.
                        tracing::debug!(server = %server.name, %user_id, "no credential for platform bearer/basic server — skipping");
                        continue;
                    }
                    None => {
                        // User-scoped: fall back to static headers from the server row
                        // (already merged into `headers`). If empty, nothing to inject.
                    }
                }
            }

            "url_param" => {
                let Some(param) = server.url_param_name.as_deref() else {
                    tracing::warn!(server = %server.name, "url_param server missing url_param_name — skipping");
                    continue;
                };
                let Some(cred) = cred_by_server.get(&server.id) else {
                    tracing::debug!(server = %server.name, %user_id, "no credential for url_param server — skipping");
                    continue;
                };
                let Some(value) = decrypt_or_skip(&crypto, &cred.credential_value, server) else {
                    continue;
                };
                url = inject_url_param(&url, param, &value)?;
            }

            "oauth2" => {
                let Some(token) = token_by_server.get(&server.id) else {
                    tracing::debug!(server = %server.name, %user_id, "no oauth token for server — skipping");
                    continue;
                };
                match oauth::access_token_for(state, &crypto, user_id, server, token).await? {
                    Some(access) => {
                        headers.insert("Authorization".to_string(), format!("Bearer {access}"));
                    }
                    None => continue,
                }
            }

            other => {
                tracing::warn!(server = %server.name, auth_type = other, "unknown auth_type — skipping");
                continue;
            }
        }

        result.push(MCPServerConfig {
            name: server.name.clone(),
            url,
            headers,
            transport: server.transport.clone(),
        });
    }

    Ok(result)
}

/// The header a bearer/basic credential is injected into (default `Authorization`).
fn credential_header(server: &McpServer) -> String {
    server
        .credential_header_name
        .clone()
        .unwrap_or_else(|| "Authorization".to_string())
}

/// Decrypt a stored credential; on failure log and signal skip (`None`).
fn decrypt_or_skip(crypto: &SecretsCrypto, encrypted: &str, server: &McpServer) -> Option<String> {
    match crypto.decrypt(encrypted) {
        Ok(v) => Some(v),
        Err(e) => {
            tracing::warn!(server = %server.name, error = %e, "failed to decrypt credential — skipping server");
            None
        }
    }
}

/// Parse a server's static `headers` JSONB into a string map.
fn parse_headers(raw: &Option<Value>) -> HashMap<String, String> {
    raw.as_ref()
        .and_then(|v| v.as_object())
        .map(|obj| {
            obj.iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                .collect()
        })
        .unwrap_or_default()
}

/// Append `?{param}={value}` to a URL, preserving existing query params and
/// percent-encoding correctly (via `reqwest::Url`).
fn inject_url_param(raw: &str, param: &str, value: &str) -> Result<String> {
    let mut url = reqwest::Url::parse(raw)
        .map_err(|e| McpError::BadRequest(format!("invalid server url '{raw}': {e}")))?;
    url.query_pairs_mut().append_pair(param, value);
    Ok(url.to_string())
}

// ─── Management (CRUD) ──────────────────────────────────────────────────────
//
// Per-user credential registration/status/deletion behind
// `/api/mcp/servers/{id}/credential*`. Distinct from `build_generic_servers`
// above (which reads credentials back out at session-resolution time), but
// same domain — kept in this module rather than splitting into a second file.

/// Load a server and confirm `user_id` may manage a credential for it
/// (platform servers: any authed user; user-scoped: the owner only).
pub async fn authorize_server(state: &McpState, user_id: Uuid, server_id: Uuid) -> Result<McpServer> {
    let server = repo::get_mcp_server_by_id(&state.db, server_id)
        .await?
        .ok_or_else(|| McpError::NotFound(format!("MCP server '{server_id}' not found")))?;
    if !server.is_platform && server.user_id != Some(user_id) {
        return Err(McpError::Forbidden("this server does not belong to you".into()));
    }
    Ok(server)
}

/// Apply the PoC's credential normalization: auto-prefix `Bearer `/`Basic ` for
/// the standard Authorization header, base64-encode basic `user:pass`, and leave
/// url_param / custom-header raw. Shared with the unified connect flow.
pub fn normalize_for(server: &McpServer, raw: &str) -> String {
    let header = server.credential_header_name.as_deref().unwrap_or("Authorization");
    let lower = raw.to_ascii_lowercase();
    match server.auth_type.as_str() {
        "bearer" if header.eq_ignore_ascii_case("Authorization") && !lower.starts_with("bearer ") => {
            format!("Bearer {raw}")
        }
        "basic" if !lower.starts_with("basic ") => {
            // Accept either a raw "user:pass" or an already-encoded blob.
            if raw.contains(':') {
                format!("Basic {}", B64.encode(raw))
            } else {
                format!("Basic {raw}")
            }
        }
        // url_param and custom-header credentials are stored raw.
        _ => raw.to_string(),
    }
}

/// Store the caller's credential for `server` (already authorized + confirmed
/// bearer/basic/url_param by the caller).
pub async fn register_credential(
    state: &McpState,
    user_id: Uuid,
    server: &McpServer,
    credential_type: &str,
    credential_value: &str,
) -> Result<()> {
    if !matches!(server.auth_type.as_str(), "bearer" | "basic" | "url_param") {
        return Err(McpError::BadRequest(format!(
            "credential registration is only for bearer/basic/url_param servers, not '{}'",
            server.auth_type
        )));
    }

    // Normalize the credential the same way the PoC's connect flow did, so the
    // session resolver can inject it verbatim.
    let value = normalize_for(server, credential_value);
    let encrypted = SecretsCrypto::for_user(user_id)
        .encrypt(&value)
        .map_err(|e| McpError::Internal(format!("encrypt credential: {e}")))?;
    repo::upsert_user_credential(&state.db, server.id, user_id, credential_type, &encrypted).await?;
    tracing::info!(server = %server.name, %user_id, "registered user credential");
    Ok(())
}

/// The caller's credential status for `server` (never the decrypted value).
pub async fn credential_status(state: &McpState, server_id: Uuid, user_id: Uuid) -> Result<Option<String>> {
    let cred = repo::get_user_credential(&state.db, server_id, user_id).await?;
    Ok(cred.map(|c| c.credential_type))
}

/// Remove the caller's credential for `server_id`. Errors `NotFound` if there
/// was none. Invalidates the session cache so the removed credential stops
/// being injected.
pub async fn delete_credential(state: &McpState, server: &McpServer, user_id: Uuid) -> Result<()> {
    if !repo::delete_user_credential(&state.db, server.id, user_id).await? {
        return Err(McpError::NotFound("no credential to delete".into()));
    }
    crate::session::invalidate_session_cache(state, user_id).await;
    tracing::info!(server = %server.name, %user_id, "deleted user credential");
    Ok(())
}

#[cfg(test)]
mod management_tests {
    use base64::Engine;
    use chrono::Utc;
    use uuid::Uuid;

    use super::{B64, normalize_for};
    use crate::repo::McpServer;

    fn server(auth_type: &str, credential_header_name: Option<&str>) -> McpServer {
        McpServer {
            id: Uuid::new_v4(),
            name: "test".into(),
            url: "https://example.com".into(),
            transport: "streamable_http".into(),
            auth_type: auth_type.into(),
            url_param_name: None,
            credential_header_name: credential_header_name.map(str::to_string),
            headers: None,
            description: None,
            display_name: None,
            logo_url: None,
            is_platform: false,
            user_id: Some(Uuid::new_v4()),
            is_active: true,
            oauth_authorization_endpoint: None,
            oauth_token_endpoint: None,
            oauth_client_id: None,
            oauth_client_secret: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    #[test]
    fn normalize_bearer_adds_prefix() {
        let s = server("bearer", None);
        assert_eq!(normalize_for(&s, "abc123"), "Bearer abc123");
    }

    #[test]
    fn normalize_bearer_leaves_existing_prefix() {
        let s = server("bearer", None);
        assert_eq!(normalize_for(&s, "Bearer abc123"), "Bearer abc123");
        assert_eq!(normalize_for(&s, "bearer abc123"), "bearer abc123");
    }

    #[test]
    fn normalize_bearer_with_custom_header_is_raw() {
        let s = server("bearer", Some("X-Api-Key"));
        assert_eq!(normalize_for(&s, "abc123"), "abc123");
    }

    #[test]
    fn normalize_basic_from_userpass() {
        let s = server("basic", None);
        assert_eq!(normalize_for(&s, "user:pass"), format!("Basic {}", B64.encode("user:pass")));
    }

    #[test]
    fn normalize_basic_already_encoded_no_colon() {
        let s = server("basic", None);
        let encoded = B64.encode("user:pass");
        assert_eq!(normalize_for(&s, &encoded), format!("Basic {encoded}"));
    }

    #[test]
    fn normalize_basic_leaves_existing_prefix() {
        let s = server("basic", None);
        assert_eq!(normalize_for(&s, "Basic xyz"), "Basic xyz");
    }

    #[test]
    fn normalize_url_param_is_raw() {
        let s = server("url_param", None);
        assert_eq!(normalize_for(&s, "abc123"), "abc123");
    }

    #[test]
    fn normalize_none_auth_type_is_raw() {
        let s = server("none", None);
        assert_eq!(normalize_for(&s, "abc123"), "abc123");
    }
}

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

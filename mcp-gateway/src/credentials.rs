//! Generic-connector credential injection + per-user credential management.
//!
//! Builds the generic (non-Composio) [`MCPServerConfig`] backends for a user,
//! injecting per-`auth_type` credentials read from `mcp_user_connections`
//! (decrypted with `SecretsCrypto::for_user`). A connector with no required
//! credential for the user is silently skipped.

use std::collections::HashMap;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use nasiko_secrets::SecretsCrypto;
use serde_json::Value;
use uuid::Uuid;

use crate::error::{McpError, Result};
use crate::oauth;
use crate::provider::GenericMcpProvider;
use crate::provider::generic::LIST_TIMEOUT;
use crate::repo::{self, McpConnector, McpUserConnection};
use crate::state::McpState;
use crate::types::{MCPServerConfig, ServerType};

/// Build the ordered list of generic backends for `user_id`, credentials injected.
pub async fn build_generic_servers(
    state: &McpState,
    user_id: Uuid,
) -> Result<Vec<MCPServerConfig>> {
    let connectors = state
        .authorizer
        .list_accessible_mcp_connectors(&state.db, user_id)
        .await?;
    if connectors.is_empty() {
        return Ok(Vec::new());
    }

    let conns = repo::list_user_connections(&state.db, user_id, None).await?;
    let conn_by_connector: HashMap<Uuid, _> =
        conns.into_iter().map(|c| (c.connector_id, c)).collect();

    let crypto = SecretsCrypto::for_user(user_id);
    let mut result = Vec::with_capacity(connectors.len());

    for connector in &connectors {
        let conn = conn_by_connector.get(&connector.id);
        if let Some(cfg) = build_server_config(state, &crypto, user_id, connector, conn).await? {
            result.push(cfg);
        }
    }

    Ok(result)
}

/// Build one connector's `MCPServerConfig`, injecting whatever per-user
/// credential/OAuth token applies for its `auth_type` — `Ok(None)` means
/// "not currently usable" (no credential yet, decrypt failure, etc.), not an
/// error. The single source of truth both `build_generic_servers` (the live
/// list for a user's session) and `verify_connector_live` (proving a
/// just-configured credential actually works) route through, so verification
/// can never drift from what a real tool call would actually send.
async fn build_server_config(
    state: &McpState,
    crypto: &SecretsCrypto,
    user_id: Uuid,
    connector: &McpConnector,
    conn: Option<&McpUserConnection>,
) -> Result<Option<MCPServerConfig>> {
    let Some(base_url) = connector.url.clone().filter(|u| !u.is_empty()) else {
        return Ok(None);
    };
    let auth_type = connector.auth_type.as_deref().unwrap_or("none");
    let mut headers = parse_headers(&connector.headers);
    let mut url = base_url;

    match auth_type {
        "none" => {}

        "bearer" | "basic" => match conn.and_then(|c| c.encrypted_credential.as_deref()) {
            Some(enc) => {
                let Some(value) = decrypt_or_skip(crypto, enc, connector) else {
                    return Ok(None);
                };
                headers.insert(credential_header(connector), value);
            }
            // No per-user credential: rely on static headers, else skip.
            None if headers.is_empty() => return Ok(None),
            None => {}
        },

        "url_param" => {
            let Some(param) = connector.url_param_name.as_deref() else {
                tracing::warn!(connector = %connector.name, "url_param connector missing url_param_name — skipping");
                return Ok(None);
            };
            let Some(enc) = conn.and_then(|c| c.encrypted_credential.as_deref()) else {
                return Ok(None);
            };
            let Some(value) = decrypt_or_skip(crypto, enc, connector) else {
                return Ok(None);
            };
            url = inject_url_param(&url, param, &value)?;
        }

        "oauth2" => {
            let Some(conn) = conn else { return Ok(None) };
            match oauth::access_token_for(state, crypto, user_id, connector, conn).await? {
                Some(access) => {
                    headers.insert("Authorization".to_string(), format!("Bearer {access}"));
                }
                None => return Ok(None),
            }
        }

        other => {
            tracing::warn!(connector = %connector.name, auth_type = other, "unknown auth_type — skipping");
            return Ok(None);
        }
    }

    Ok(Some(MCPServerConfig {
        connector_id: connector.id,
        kind: ServerType::Mcp,
        name: connector.name.clone(),
        url,
        headers,
        transport: connector
            .transport
            .clone()
            .unwrap_or_else(|| "streamable_http".to_string()),
        // The ONLY place `trusted` is ever computed — see MCPServerConfig's
        // doc comment. Read straight off the already-joined connector row,
        // no new query.
        trusted: connector.source_kind == repo::SourceKind::UploadedBuild,
    }))
}

/// Outcome of actually calling the connector, not just submitting config for it.
pub struct VerifyOutcome {
    pub verified: bool,
    pub error: Option<String>,
}

/// Prove a connector's currently-configured auth actually works: a real,
/// authenticated `tools/list` against the real server — the actual
/// correctness gate, not another local guess. `user_id`'s own credential/OAuth
/// token is used, same as any real tool call would use.
pub async fn verify_connector_live(
    state: &McpState,
    user_id: Uuid,
    connector: &McpConnector,
) -> VerifyOutcome {
    let crypto = SecretsCrypto::for_user(user_id);
    let conn = match repo::get_user_connection(&state.db, user_id, connector.id).await {
        Ok(c) => c,
        Err(e) => {
            return VerifyOutcome {
                verified: false,
                error: Some(format!("failed to load credential: {e}")),
            };
        }
    };

    let cfg = match build_server_config(state, &crypto, user_id, connector, conn.as_ref()).await {
        Ok(Some(cfg)) => cfg,
        Ok(None) => {
            return VerifyOutcome {
                verified: false,
                error: Some(
                    "no usable credential is configured for this connector yet".to_string(),
                ),
            };
        }
        Err(e) => {
            return VerifyOutcome {
                verified: false,
                error: Some(format!("failed to prepare request: {e}")),
            };
        }
    };

    let provider =
        GenericMcpProvider::new(state.guarded_http_client.clone(), state.http_client.clone());
    match provider.list_tools(&cfg, LIST_TIMEOUT, None).await {
        Ok(_) => VerifyOutcome {
            verified: true,
            error: None,
        },
        Err(e) => VerifyOutcome {
            verified: false,
            error: Some(e.to_string()),
        },
    }
}

/// The header a bearer/basic credential is injected into (default `Authorization`).
fn credential_header(connector: &McpConnector) -> String {
    connector
        .credential_header_name
        .clone()
        .unwrap_or_else(|| "Authorization".to_string())
}

fn decrypt_or_skip(
    crypto: &SecretsCrypto,
    encrypted: &str,
    connector: &McpConnector,
) -> Option<String> {
    match crypto.decrypt(encrypted) {
        Ok(v) => Some(v),
        Err(e) => {
            tracing::warn!(connector = %connector.name, error = %e, "failed to decrypt credential — skipping");
            None
        }
    }
}

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

/// Append `?{param}={value}` to a URL, percent-encoding correctly.
fn inject_url_param(raw: &str, param: &str, value: &str) -> Result<String> {
    let mut url = reqwest::Url::parse(raw)
        .map_err(|e| McpError::BadRequest(format!("invalid connector url '{raw}': {e}")))?;
    url.query_pairs_mut().append_pair(param, value);
    Ok(url.to_string())
}

// ─── Management (CRUD) ──────────────────────────────────────────────────────

/// Load a connector and confirm `user_id` may reach it (owner / grant / composio).
pub async fn authorize_connector(
    state: &McpState,
    user_id: Uuid,
    connector_id: Uuid,
) -> Result<McpConnector> {
    let connector = repo::get_connector_by_id(&state.db, connector_id)
        .await?
        .ok_or_else(|| McpError::NotFound(format!("connector '{connector_id}' not found")))?;
    if !state
        .authorizer
        .can_access_connector(&state.db, user_id, connector_id)
        .await?
    {
        return Err(McpError::Forbidden(
            "you do not have access to this connector".into(),
        ));
    }
    Ok(connector)
}

/// Auto-prefix `Bearer `/`Basic `, base64-encode basic `user:pass`, leave the rest raw.
pub fn normalize_for(connector: &McpConnector, raw: &str) -> String {
    let header = connector
        .credential_header_name
        .as_deref()
        .unwrap_or("Authorization");
    let auth_type = connector.auth_type.as_deref().unwrap_or("none");
    let lower = raw.to_ascii_lowercase();
    match auth_type {
        "bearer"
            if header.eq_ignore_ascii_case("Authorization") && !lower.starts_with("bearer ") =>
        {
            format!("Bearer {raw}")
        }
        "basic" if !lower.starts_with("basic ") => {
            if raw.contains(':') {
                format!("Basic {}", B64.encode(raw))
            } else {
                format!("Basic {raw}")
            }
        }
        _ => raw.to_string(),
    }
}

/// Store the caller's credential for `connector` (bearer/basic/url_param), then
/// prove it actually works with a real call before marking it `active` — a
/// submitted value is not the same as a working one (see `verify_connector_live`).
/// The credential is kept either way: a wrong value is something the user can
/// see and fix via this same endpoint, not something that should force a
/// delete-and-restart.
pub async fn register_credential(
    state: &McpState,
    user_id: Uuid,
    connector: &McpConnector,
    credential_value: &str,
) -> Result<VerifyOutcome> {
    let auth_type = connector.auth_type.as_deref().unwrap_or("none");
    if !matches!(auth_type, "bearer" | "basic" | "url_param") {
        return Err(McpError::BadRequest(format!(
            "credential registration is only for bearer/basic/url_param connectors, not '{auth_type}'"
        )));
    }
    let value = normalize_for(connector, credential_value);
    let encrypted = SecretsCrypto::for_user(user_id).encrypt(&value);
    repo::upsert_connection_credential(&state.db, user_id, connector.id, &encrypted).await?;

    let outcome = verify_connector_live(state, user_id, connector).await;
    let (status, error) = if outcome.verified {
        ("active", None)
    } else {
        ("failed", outcome.error.as_deref())
    };
    repo::set_connector_setup_status(&state.db, connector.id, status, error).await?;

    crate::session::invalidate_session_cache(state, user_id).await;
    tracing::info!(connector = %connector.name, %user_id, verified = outcome.verified, "registered user credential");
    Ok(outcome)
}

/// The caller's credential status for `connector` (never the value): the auth
/// type when a credential is stored, else `None`.
pub async fn credential_status(
    state: &McpState,
    connector_id: Uuid,
    user_id: Uuid,
) -> Result<Option<String>> {
    let conn = repo::get_user_connection(&state.db, user_id, connector_id).await?;
    let has_cred = conn
        .as_ref()
        .and_then(|c| c.encrypted_credential.as_ref())
        .is_some();
    if !has_cred {
        return Ok(None);
    }
    let connector = repo::get_connector_by_id(&state.db, connector_id).await?;
    Ok(connector.and_then(|c| c.auth_type))
}

/// Remove the caller's credential/connection for `connector`.
pub async fn delete_credential(
    state: &McpState,
    connector: &McpConnector,
    user_id: Uuid,
) -> Result<()> {
    if !repo::delete_user_connection(&state.db, user_id, connector.id).await? {
        return Err(McpError::NotFound("no credential to delete".into()));
    }
    crate::session::invalidate_session_cache(state, user_id).await;
    tracing::info!(connector = %connector.name, %user_id, "deleted user credential");
    Ok(())
}

#[cfg(test)]
mod tests {
    use base64::Engine;
    use chrono::Utc;
    use uuid::Uuid;

    use super::{B64, normalize_for};
    use crate::repo::McpConnector;

    fn connector(auth_type: &str, credential_header_name: Option<&str>) -> McpConnector {
        McpConnector {
            id: Uuid::new_v4(),
            provider_type: "mcp_server".into(),
            owner_id: Some(Uuid::new_v4()),
            name: "test".into(),
            display_name: None,
            logo_url: None,
            description: None,
            auth_config_id: None,
            auth_scheme: None,
            use_composio_managed: None,
            url: Some("https://example.com".into()),
            transport: Some("streamable_http".into()),
            auth_type: Some(auth_type.into()),
            url_param_name: None,
            credential_header_name: credential_header_name.map(str::to_string),
            headers: None,
            is_active: Some(true),
            oauth_authorization_endpoint: None,
            oauth_token_endpoint: None,
            oauth_client_id: None,
            oauth_client_secret: None,
            source_kind: crate::repo::SourceKind::ExternalUrl,
            build_status: None,
            container_image_tag: None,
            setup_status: None,
            setup_error: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    #[test]
    fn normalize_bearer_adds_and_preserves_prefix() {
        let c = connector("bearer", None);
        assert_eq!(normalize_for(&c, "abc123"), "Bearer abc123");
        assert_eq!(normalize_for(&c, "Bearer abc123"), "Bearer abc123");
    }

    #[test]
    fn normalize_bearer_custom_header_is_raw() {
        let c = connector("bearer", Some("X-Api-Key"));
        assert_eq!(normalize_for(&c, "abc123"), "abc123");
    }

    #[test]
    fn normalize_basic_variants() {
        let c = connector("basic", None);
        assert_eq!(
            normalize_for(&c, "user:pass"),
            format!("Basic {}", B64.encode("user:pass"))
        );
        let enc = B64.encode("user:pass");
        assert_eq!(normalize_for(&c, &enc), format!("Basic {enc}"));
        assert_eq!(normalize_for(&c, "Basic xyz"), "Basic xyz");
    }

    #[test]
    fn normalize_url_param_and_none_are_raw() {
        assert_eq!(normalize_for(&connector("url_param", None), "abc"), "abc");
        assert_eq!(normalize_for(&connector("none", None), "abc"), "abc");
    }
}

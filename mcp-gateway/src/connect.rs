//! Unified connect / disconnect + connection listing, and the Composio OAuth
//! browser callback — pure logic behind `/api/mcp/connect*` and the redirect
//! target `/oauth/callback`.
//!
//! One entry point ([`connect_service`]) handles every service type: Composio
//! toolkit (OAuth), generic MCP bearer/basic/url_param (store credential), MCP
//! OAuth 2.1 (return authorization_url), and no-auth (immediately connected).

use nasiko_secrets::SecretsCrypto;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::error::{McpError, Result};
use crate::oauth::CallbackOutcome;
use crate::provider::normalize_connection_status;
use crate::repo::{self, McpConnection};
use crate::state::McpState;
use crate::{credentials, oauth, servers, session};

fn conn_dto(c: &McpConnection) -> Value {
    json!({
        "id": c.id,
        "toolkit": c.toolkit,
        "status": c.status,
        "connected_account_id": c.connected_account_id,
        "oauth_url": c.oauth_url,
        "created_at": c.created_at,
    })
}

/// Inputs for [`connect_service`].
pub struct ConnectInput {
    /// Toolkit or MCP server name (already resolved from the request's
    /// `service`/`toolkit` fields and lower-cased by the caller).
    pub service: String,
    /// Explicit service type, when the caller already knows it.
    pub kind: Option<String>,
    /// A custom MCP server URL, for auto-registration when `service` doesn't
    /// resolve to an existing platform/user server.
    pub url: Option<String>,
    pub credential_value: Option<String>,
    pub redirect_url: Option<String>,
}

/// Outcome of [`connect_service`] for the server to shape into an HTTP
/// response.
pub enum ConnectOutcome {
    /// 200 — already connected / no-auth server.
    Connected { service: String },
    /// 201 — Composio OAuth initiated (existing or newly-created).
    Initiated { service: String, oauth_url: Option<String> },
    /// 200 — generic MCP OAuth 2.1 authorization required.
    OAuthRequired { service: String, authorization_url: String },
}

/// `POST /api/mcp/connect` — connect any service type.
pub async fn connect_service(state: &McpState, user_id: Uuid, input: ConnectInput) -> Result<ConnectOutcome> {
    if input.service.is_empty() && input.url.is_none() {
        return Err(McpError::BadRequest("either 'service' or 'url' is required".into()));
    }

    // Detect the service type when not explicit.
    let mut kind = input.kind.clone();
    if kind.is_none() && !input.service.is_empty() {
        if repo::get_platform_auth_config_by_toolkit(&state.db, &input.service).await?.is_some() {
            kind = Some("composio".into());
        } else if repo::get_platform_mcp_server_by_name(&state.db, &input.service).await?.is_some() {
            kind = Some("mcp".into());
        }
    }

    // ── Composio toolkit ────────────────────────────────────────────────────
    if kind.as_deref() == Some("composio") || (kind.is_none() && !input.service.is_empty()) {
        match composio_connect(state, user_id, &input.service, input.redirect_url.as_deref()).await {
            Ok(outcome) => return Ok(outcome),
            Err(McpError::NotFound(_)) if kind.is_none() => {
                // Not a Composio toolkit — fall through to generic MCP.
            }
            Err(e) => return Err(e),
        }
    }

    generic_connect(state, user_id, &input).await
}

/// Composio OAuth connect: initiate the Tool Router link and record an
/// INITIATED connection. Errors `NotFound` when the toolkit has no platform
/// auth config, so the caller can fall through to generic MCP.
async fn composio_connect(
    state: &McpState,
    user_id: Uuid,
    toolkit: &str,
    redirect_url: Option<&str>,
) -> Result<ConnectOutcome> {
    let auth_config = repo::get_platform_auth_config_by_toolkit(&state.db, toolkit)
        .await?
        .ok_or_else(|| McpError::NotFound(format!("no platform auth config for toolkit '{toolkit}'")))?;

    // Reuse an existing ACTIVE / recent-INITIATED connection.
    if let Some(existing) = repo::get_active_or_pending_connection(&state.db, user_id, toolkit).await? {
        if existing.status == "ACTIVE" {
            return Ok(ConnectOutcome::Connected { service: toolkit.to_string() });
        }
        // INITIATED and still fresh → hand back the same OAuth URL.
        return Ok(ConnectOutcome::Initiated { service: toolkit.to_string(), oauth_url: existing.oauth_url });
    }
    let _ = repo::delete_orphan_expired_connections(&state.db, user_id, toolkit).await?;

    let provider = state.providers.require_composio()?;
    // Land the browser on our callback so we can verify + activate.
    let callback_url = composio_callback_url(state, user_id, toolkit, redirect_url);
    let initiated = provider
        .initiate_connection(&user_id.to_string(), &auth_config.auth_config_id, callback_url.as_deref())
        .await?;

    let oauth_url =
        initiated.redirect_url.ok_or_else(|| McpError::Composio("Composio did not return an OAuth URL".into()))?;

    let connection = repo::create_connection(
        &state.db,
        user_id,
        &auth_config.auth_config_id,
        toolkit,
        Some(&oauth_url),
        callback_url.as_deref(),
        None,
    )
    .await?;

    tracing::info!(%user_id, toolkit, "composio oauth initiated");
    Ok(ConnectOutcome::Initiated { service: toolkit.to_string(), oauth_url: connection.oauth_url })
}

/// Generic MCP connect: no-auth / credential / oauth2.
async fn generic_connect(state: &McpState, user_id: Uuid, input: &ConnectInput) -> Result<ConnectOutcome> {
    let service = input.service.as_str();

    // Resolve the server: platform catalog, the user's own, or auto-register a
    // custom URL as a user-scoped server (auth type probed).
    let mut server = if service.is_empty() {
        None
    } else {
        match repo::get_platform_mcp_server_by_name(&state.db, service).await? {
            Some(s) => Some(s),
            None => repo::get_user_mcp_server_by_name(&state.db, user_id, service).await?,
        }
    };

    if server.is_none()
        && let Some(url) = &input.url
    {
        // SSRF guard before probing/registering a user-supplied custom URL.
        crate::net::validate_public_url(url).await?;
        let detected = probe_auth_type(state, url).await;
        let name = if service.is_empty() {
            url.split("//").last().and_then(|h| h.split('/').next()).unwrap_or(url).to_string()
        } else {
            service.to_string()
        };
        let new = repo::NewMcpServer {
            name,
            url: url.clone(),
            transport: "streamable_http".into(),
            auth_type: detected,
            url_param_name: None,
            credential_header_name: None,
            headers: None,
            description: None,
            display_name: None,
            logo_url: None,
            is_platform: false,
            user_id: Some(user_id),
            is_active: true,
        };
        server = Some(repo::create_mcp_server(&state.db, &new).await?);
    }

    let server = server.ok_or_else(|| {
        McpError::NotFound(format!("service '{service}' not found — check /catalog or provide a url"))
    })?;

    match server.auth_type.as_str() {
        "none" => Ok(ConnectOutcome::Connected { service: server.name }),

        "bearer" | "basic" | "url_param" => {
            let value = input
                .credential_value
                .as_ref()
                .ok_or_else(|| McpError::BadRequest(format!("'{}' requires credentials.value", server.name)))?;
            let normalized = credentials::normalize_for(&server, value);
            let encrypted = SecretsCrypto::for_user(user_id)
                .encrypt(&normalized)
                .map_err(|e| McpError::Internal(format!("encrypt credential: {e}")))?;
            repo::upsert_user_credential(&state.db, server.id, user_id, &server.auth_type, &encrypted).await?;
            session::invalidate_session_cache(state, user_id).await;
            Ok(ConnectOutcome::Connected { service: server.name })
        }

        "oauth2" => {
            let url =
                oauth::begin_authorization(state, user_id, server, input.redirect_url.clone(), None).await?;
            Ok(ConnectOutcome::OAuthRequired { service: service.to_string(), authorization_url: url })
        }

        other => Err(McpError::BadRequest(format!("unsupported auth_type '{other}'"))),
    }
}

/// `GET /api/mcp/connections` view: the caller's connections, syncing any
/// pending ones with Composio.
pub async fn list_connections_view(state: &McpState, user_id: Uuid) -> Result<Value> {
    let connections = repo::list_connections_by_user(&state.db, user_id, None).await?;

    // Best-effort sync of non-terminal connections.
    if let Some(provider) = &state.providers.composio {
        for conn in &connections {
            if conn.status == "ACTIVE" || conn.status == "EXPIRED" {
                continue;
            }
            if let Ok(check) = provider.check_connection_status(&user_id.to_string(), &conn.auth_config_id).await
                && !matches!(check.status.as_str(), "NOT_FOUND" | "UNKNOWN")
            {
                let normalized = normalize_connection_status(&check.status);
                if normalized != conn.status {
                    let _ = repo::update_connection_status(&state.db, conn.id, normalized).await;
                }
                if let Some(account_id) = check.account_id.as_deref()
                    && conn.connected_account_id.is_none()
                {
                    let _ = repo::update_connection_account_id(&state.db, conn.id, account_id).await;
                }
            }
        }
    }

    let fresh = repo::list_connections_by_user(&state.db, user_id, None).await?;
    let data: Vec<Value> = fresh.iter().map(conn_dto).collect();
    let total = data.len();
    Ok(json!({ "data": data, "total": total }))
}

/// Outcome of [`disconnect_toolkit`].
pub struct DisconnectOutcome {
    pub message: String,
    pub toolkit: String,
    pub composio_revoked: bool,
}

/// `DELETE /api/mcp/connections/{toolkit}` — revoke a Composio connection.
pub async fn disconnect_toolkit(state: &McpState, user_id: Uuid, toolkit: &str) -> Result<DisconnectOutcome> {
    let toolkit = toolkit.to_lowercase();

    let connection = repo::get_connection_by_user_and_toolkit(&state.db, user_id, &toolkit, Some("ACTIVE"))
        .await?
        .ok_or_else(|| McpError::NotFound(format!("no active '{toolkit}' connection")))?;

    // Revoke the Composio token (best-effort).
    let mut composio_revoked = false;
    if let (Some(provider), Some(account_id)) =
        (&state.providers.composio, connection.connected_account_id.as_deref())
    {
        composio_revoked = provider.revoke_connection(account_id).await.unwrap_or(false);
    }

    repo::update_connection_status(&state.db, connection.id, "EXPIRED").await?;
    session::invalidate_session_cache(state, user_id).await;

    tracing::info!(%user_id, toolkit, composio_revoked, "disconnected toolkit");
    Ok(DisconnectOutcome { message: format!("Disconnected '{toolkit}'."), toolkit, composio_revoked })
}

/// Build our `/oauth/callback` URL (browser-reachable) from the gateway's public
/// origin, so Composio redirects back for verification.
fn composio_callback_url(state: &McpState, user_id: Uuid, toolkit: &str, success_url: Option<&str>) -> Option<String> {
    let base = state.config.gateway_public_url.as_ref()?;
    let mut origin = reqwest::Url::parse(base).ok()?;
    origin.set_path("/oauth/callback");
    origin.set_query(None);
    origin.query_pairs_mut().append_pair("user_id", &user_id.to_string()).append_pair("toolkit", toolkit);
    if let Some(s) = success_url {
        origin.query_pairs_mut().append_pair("success_url", s);
    }
    Some(origin.to_string())
}

/// Probe a URL's auth type (used when auto-registering a custom server).
/// Best-effort: any network failure defaults to `"none"` so registration can
/// proceed (matches the PoC).
async fn probe_auth_type(state: &McpState, url: &str) -> String {
    match servers::probe_initialize(&state.http_client, url).await {
        Ok((detected, _status)) => detected.as_str().to_string(),
        Err(_) => "none".to_string(),
    }
}

/// `GET /oauth/callback` core: the public Composio redirect target. Verifies
/// the connection became ACTIVE, records the account id, invalidates the
/// session cache, and reports where to redirect.
pub async fn handle_composio_callback(
    state: &McpState,
    user_id: Option<Uuid>,
    toolkit: Option<String>,
    success_url: Option<String>,
) -> CallbackOutcome {
    let (Some(user_id), Some(toolkit)) = (user_id, toolkit) else {
        return CallbackOutcome::Message("Missing user_id or toolkit.".to_string());
    };

    let connections = match repo::list_connections_by_user(&state.db, user_id, None).await {
        Ok(c) => c,
        Err(e) => return CallbackOutcome::Message(format!("Lookup failed: {e}")),
    };
    let pending = connections.into_iter().find(|c| c.toolkit == toolkit && c.status != "EXPIRED");
    let Some(connection) = pending else {
        return CallbackOutcome::Message(format!("No pending connection for '{toolkit}'."));
    };

    let Some(provider) = &state.providers.composio else {
        return CallbackOutcome::Message("Composio is not configured.".to_string());
    };
    match provider.check_connection_status(&user_id.to_string(), &connection.auth_config_id).await {
        Ok(check) if check.status.eq_ignore_ascii_case("ACTIVE") => {
            let _ = repo::update_connection_status(&state.db, connection.id, "ACTIVE").await;
            if let Some(account_id) = check.account_id.as_deref() {
                let _ = repo::update_connection_account_id(&state.db, connection.id, account_id).await;
            }
            session::invalidate_session_cache(state, user_id).await;
            CallbackOutcome::Redirect(success_url.unwrap_or_else(|| "/".to_string()))
        }
        _ => CallbackOutcome::Message(format!("'{toolkit}' authorization is still finalizing — refresh in a moment.")),
    }
}

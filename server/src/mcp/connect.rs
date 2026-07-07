//! Unified connect / disconnect + connection listing, and the Composio OAuth
//! browser callback.
//!
//! One endpoint (`POST /api/mcp/connect`) handles every service type: Composio
//! toolkit (OAuth), generic MCP bearer/basic/url_param (store credential), MCP
//! OAuth 2.1 (return authorization_url), and no-auth (immediately connected).

use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{Html, IntoResponse, Redirect, Response},
};
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use nasiko_mcp_gateway::provider::normalize_connection_status;
use nasiko_mcp_gateway::repo::{self, McpConnection};
use nasiko_mcp_gateway::{McpError, session};
use nasiko_secrets::SecretsCrypto;

use super::{ApiError, parse_user};
use crate::auth::Claims;
use crate::state::AppState;

#[derive(Debug, Deserialize)]
pub struct Credentials {
    pub value: String,
}

#[derive(Debug, Deserialize)]
pub struct ConnectRequest {
    pub service: Option<String>,
    pub toolkit: Option<String>,
    #[serde(rename = "type")]
    pub kind: Option<String>,
    pub url: Option<String>,
    pub credentials: Option<Credentials>,
    pub redirect_url: Option<String>,
}

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

/// `POST /api/mcp/connect` — connect any service type.
pub async fn connect_service(
    State(state): State<AppState>,
    claims: Claims,
    Json(body): Json<ConnectRequest>,
) -> Result<Response, ApiError> {
    let user_id = parse_user(&claims)?;
    let service = body
        .service
        .clone()
        .or_else(|| body.toolkit.clone())
        .map(|s| s.to_lowercase())
        .unwrap_or_default();

    if service.is_empty() && body.url.is_none() {
        return Err(ApiError(McpError::BadRequest("either 'service' or 'url' is required".into())));
    }

    // Detect the service type when not explicit.
    let mut kind = body.kind.clone();
    if kind.is_none() && !service.is_empty() {
        if repo::get_platform_auth_config_by_toolkit(&state.mcp.db, &service).await?.is_some() {
            kind = Some("composio".into());
        } else if repo::get_platform_mcp_server_by_name(&state.mcp.db, &service).await?.is_some() {
            kind = Some("mcp".into());
        }
    }

    // ── Composio toolkit ────────────────────────────────────────────────────
    if kind.as_deref() == Some("composio") || (kind.is_none() && !service.is_empty()) {
        match composio_connect(&state, user_id, &service, body.redirect_url.as_deref()).await {
            Ok(resp) => return Ok(resp),
            Err(ApiError(McpError::NotFound(_))) if kind.is_none() => {
                // Not a Composio toolkit — fall through to generic MCP.
            }
            Err(e) => return Err(e),
        }
    }

    generic_connect(&state, user_id, &service, &body).await
}

/// Composio OAuth connect: initiate the Tool Router link and record an
/// INITIATED connection. Returns 404 (via `McpError::NotFound`) when the toolkit
/// has no platform auth config, so the caller can fall through to generic MCP.
async fn composio_connect(
    state: &AppState,
    user_id: Uuid,
    toolkit: &str,
    redirect_url: Option<&str>,
) -> Result<Response, ApiError> {
    let auth_config = repo::get_platform_auth_config_by_toolkit(&state.mcp.db, toolkit)
        .await?
        .ok_or_else(|| McpError::NotFound(format!("no platform auth config for toolkit '{toolkit}'")))?;

    // Reuse an existing ACTIVE / recent-INITIATED connection.
    if let Some(existing) = repo::get_active_or_pending_connection(&state.mcp.db, user_id, toolkit).await? {
        if existing.status == "ACTIVE" {
            return Ok((StatusCode::OK, Json(json!({ "status": "connected", "service": toolkit }))).into_response());
        }
        // INITIATED and still fresh → hand back the same OAuth URL.
        return Ok((
            StatusCode::CREATED,
            Json(json!({ "status": "initiated", "service": toolkit, "oauth_url": existing.oauth_url })),
        )
            .into_response());
    }
    let _ = repo::delete_orphan_expired_connections(&state.mcp.db, user_id, toolkit).await?;

    let provider = state.mcp.providers.require_composio()?;
    // Land the browser on our callback so we can verify + activate.
    let callback_url = composio_callback_url(state, user_id, toolkit, redirect_url);
    let initiated = provider
        .initiate_connection(&user_id.to_string(), &auth_config.auth_config_id, callback_url.as_deref())
        .await?;

    let oauth_url = initiated.redirect_url.ok_or_else(|| {
        McpError::Composio("Composio did not return an OAuth URL".into())
    })?;

    let connection = repo::create_connection(
        &state.mcp.db,
        user_id,
        &auth_config.auth_config_id,
        toolkit,
        Some(&oauth_url),
        callback_url.as_deref(),
        None,
    )
    .await?;

    tracing::info!(%user_id, toolkit, "composio oauth initiated");
    Ok((
        StatusCode::CREATED,
        Json(json!({ "status": "initiated", "service": toolkit, "oauth_url": connection.oauth_url })),
    )
        .into_response())
}

/// Generic MCP connect: no-auth / credential / oauth2.
async fn generic_connect(
    state: &AppState,
    user_id: Uuid,
    service: &str,
    body: &ConnectRequest,
) -> Result<Response, ApiError> {
    // Resolve the server: platform catalog, the user's own, or auto-register a
    // custom URL as a user-scoped server (auth type probed).
    let mut server = if service.is_empty() {
        None
    } else {
        match repo::get_platform_mcp_server_by_name(&state.mcp.db, service).await? {
            Some(s) => Some(s),
            None => repo::get_user_mcp_server_by_name(&state.mcp.db, user_id, service).await?,
        }
    };

    if server.is_none()
        && let Some(url) = &body.url
    {
        // SSRF guard before probing/registering a user-supplied custom URL.
        nasiko_mcp_gateway::net::validate_public_url(url).await?;
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
        server = Some(repo::create_mcp_server(&state.mcp.db, &new).await?);
    }

    let server = server.ok_or_else(|| {
        McpError::NotFound(format!("service '{service}' not found — check /catalog or provide a url"))
    })?;

    match server.auth_type.as_str() {
        "none" => Ok(Json(json!({ "status": "connected", "service": server.name })).into_response()),

        "bearer" | "basic" | "url_param" => {
            let creds = body.credentials.as_ref().ok_or_else(|| {
                McpError::BadRequest(format!("'{}' requires credentials.value", server.name))
            })?;
            let value = super::credentials::normalize_for(&server, &creds.value);
            let encrypted = SecretsCrypto::for_user(user_id)
                .encrypt(&value)
                .map_err(|e| McpError::Internal(format!("encrypt credential: {e}")))?;
            repo::upsert_user_credential(&state.mcp.db, server.id, user_id, &server.auth_type, &encrypted).await?;
            session::invalidate_session_cache(&state.mcp, user_id).await;
            Ok(Json(json!({ "status": "connected", "service": server.name })).into_response())
        }

        "oauth2" => {
            let url = super::oauth::begin_authorization(
                state,
                user_id,
                server,
                body.redirect_url.clone(),
                None,
            )
            .await?;
            Ok(Json(json!({ "status": "oauth_required", "service": service, "authorization_url": url }))
                .into_response())
        }

        other => Err(ApiError(McpError::BadRequest(format!("unsupported auth_type '{other}'")))),
    }
}

/// `GET /api/mcp/connections` — list the caller's connections, syncing any
/// pending ones with Composio.
pub async fn list_connections(
    State(state): State<AppState>,
    claims: Claims,
) -> Result<Json<Value>, ApiError> {
    let user_id = parse_user(&claims)?;
    let connections = repo::list_connections_by_user(&state.mcp.db, user_id, None).await?;

    // Best-effort sync of non-terminal connections.
    if let Some(provider) = &state.mcp.providers.composio {
        for conn in &connections {
            if conn.status == "ACTIVE" || conn.status == "EXPIRED" {
                continue;
            }
            if let Ok(check) = provider
                .check_connection_status(&user_id.to_string(), &conn.auth_config_id)
                .await
                && !matches!(check.status.as_str(), "NOT_FOUND" | "UNKNOWN")
            {
                let normalized = normalize_connection_status(&check.status);
                if normalized != conn.status {
                    let _ = repo::update_connection_status(&state.mcp.db, conn.id, normalized).await;
                }
                if let Some(account_id) = check.account_id.as_deref()
                    && conn.connected_account_id.is_none()
                {
                    let _ = repo::update_connection_account_id(&state.mcp.db, conn.id, account_id).await;
                }
            }
        }
    }

    let fresh = repo::list_connections_by_user(&state.mcp.db, user_id, None).await?;
    let data: Vec<Value> = fresh.iter().map(conn_dto).collect();
    let total = data.len();
    Ok(Json(json!({ "data": data, "total": total })))
}

/// `DELETE /api/mcp/connections/{toolkit}` — revoke a Composio connection.
pub async fn disconnect_toolkit(
    State(state): State<AppState>,
    claims: Claims,
    Path(toolkit): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let user_id = parse_user(&claims)?;
    let toolkit = toolkit.to_lowercase();

    let connection = repo::get_connection_by_user_and_toolkit(&state.mcp.db, user_id, &toolkit, Some("ACTIVE"))
        .await?
        .ok_or_else(|| McpError::NotFound(format!("no active '{toolkit}' connection")))?;

    // Revoke the Composio token (best-effort).
    let mut composio_revoked = false;
    if let (Some(provider), Some(account_id)) =
        (&state.mcp.providers.composio, connection.connected_account_id.as_deref())
    {
        composio_revoked = provider.revoke_connection(account_id).await.unwrap_or(false);
    }

    repo::update_connection_status(&state.mcp.db, connection.id, "EXPIRED").await?;
    session::invalidate_session_cache(&state.mcp, user_id).await;

    tracing::info!(%user_id, toolkit, composio_revoked, "disconnected toolkit");
    Ok(Json(json!({
        "message": format!("Disconnected '{toolkit}'."),
        "toolkit": toolkit,
        "composio_revoked": composio_revoked,
    })))
}

#[derive(Debug, Deserialize)]
pub struct ComposioCallbackQuery {
    pub user_id: Option<Uuid>,
    pub toolkit: Option<String>,
    pub success_url: Option<String>,
}

/// `GET /oauth/callback` — public Composio redirect target. Verifies the
/// connection became ACTIVE, records the account id, invalidates the session
/// cache, and redirects.
pub async fn oauth_callback(
    State(state): State<AppState>,
    Query(q): Query<ComposioCallbackQuery>,
) -> Response {
    let (Some(user_id), Some(toolkit)) = (q.user_id, q.toolkit) else {
        return Html(callback_page("Missing user_id or toolkit.")).into_response();
    };

    // Find the pending connection.
    let connections = match repo::list_connections_by_user(&state.mcp.db, user_id, None).await {
        Ok(c) => c,
        Err(e) => return Html(callback_page(&format!("Lookup failed: {e}"))).into_response(),
    };
    let pending = connections
        .into_iter()
        .find(|c| c.toolkit == toolkit && c.status != "EXPIRED");
    let Some(connection) = pending else {
        return Html(callback_page(&format!("No pending connection for '{toolkit}'."))).into_response();
    };

    let Some(provider) = &state.mcp.providers.composio else {
        return Html(callback_page("Composio is not configured.")).into_response();
    };
    match provider.check_connection_status(&user_id.to_string(), &connection.auth_config_id).await {
        Ok(check) if check.status.eq_ignore_ascii_case("ACTIVE") => {
            let _ = repo::update_connection_status(&state.mcp.db, connection.id, "ACTIVE").await;
            if let Some(account_id) = check.account_id.as_deref() {
                let _ = repo::update_connection_account_id(&state.mcp.db, connection.id, account_id).await;
            }
            session::invalidate_session_cache(&state.mcp, user_id).await;
            let dest = q.success_url.unwrap_or_else(|| "/".to_string());
            Redirect::to(&dest).into_response()
        }
        _ => Html(callback_page(&format!(
            "'{toolkit}' authorization is still finalizing — refresh in a moment."
        )))
        .into_response(),
    }
}

/// Build our `/oauth/callback` URL (browser-reachable) from the gateway's public
/// origin, so Composio redirects back for verification.
fn composio_callback_url(
    state: &AppState,
    user_id: Uuid,
    toolkit: &str,
    success_url: Option<&str>,
) -> Option<String> {
    let base = state.mcp.config.gateway_public_url.as_ref()?;
    let mut origin = reqwest::Url::parse(base).ok()?;
    origin.set_path("/oauth/callback");
    origin.set_query(None);
    origin
        .query_pairs_mut()
        .append_pair("user_id", &user_id.to_string())
        .append_pair("toolkit", toolkit);
    if let Some(s) = success_url {
        origin.query_pairs_mut().append_pair("success_url", s);
    }
    Some(origin.to_string())
}

/// Probe a URL's auth type (used when auto-registering a custom server).
async fn probe_auth_type(state: &AppState, url: &str) -> String {
    let resp = state
        .mcp
        .http_client
        .post(url)
        .timeout(std::time::Duration::from_secs(10))
        .header("Content-Type", "application/json")
        .body(
            json!({"jsonrpc":"2.0","id":1,"method":"initialize",
                   "params":{"protocolVersion":"2024-11-05","capabilities":{},
                             "clientInfo":{"name":"mcp-gateway-probe","version":"1.0"}}})
            .to_string(),
        )
        .send()
        .await;
    match resp {
        Ok(r) if r.status().is_success() => "none".into(),
        Ok(r) if r.status() == reqwest::StatusCode::UNAUTHORIZED => {
            let www = r
                .headers()
                .get(reqwest::header::WWW_AUTHENTICATE)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("");
            if www.contains("resource_metadata") { "oauth2".into() } else { "bearer".into() }
        }
        _ => "none".into(),
    }
}

fn callback_page(message: &str) -> String {
    format!(
        "<!DOCTYPE html><html><head><meta charset=\"utf-8\"><title>Connecting…</title></head>\
         <body style=\"font-family:sans-serif;text-align:center;padding:48px\">\
         <p style=\"color:#666\">{}</p></body></html>",
        message.replace('<', "&lt;")
    )
}

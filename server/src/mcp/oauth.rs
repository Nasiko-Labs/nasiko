//! MCP OAuth 2.1 per-server flow: authorize / callback / status / revoke.
//!
//! Building blocks (discovery, PKCE, signed state, token exchange) live in the
//! crate's `oauth` module; these handlers orchestrate them against `AppState`.

use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{Html, IntoResponse, Redirect, Response},
};
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

use nasiko_mcp_gateway::{McpError, oauth, repo, session};
use nasiko_secrets::SecretsCrypto;

use super::{ApiError, parse_user};
use crate::auth::Claims;
use crate::state::AppState;

#[derive(Debug, Deserialize, Default)]
pub struct AuthorizeRequest {
    /// Pre-registered client_id (skips dynamic client registration).
    pub client_id: Option<String>,
    /// Where to send the user after OAuth completes.
    pub redirect_url: Option<String>,
}

async fn load_owned_oauth_server(
    state: &AppState,
    user_id: Uuid,
    server_id: Uuid,
) -> Result<repo::McpServer, ApiError> {
    let server = repo::get_mcp_server_by_id(&state.mcp.db, server_id)
        .await?
        .ok_or_else(|| ApiError(McpError::NotFound(format!("MCP server '{server_id}' not found"))))?;
    if !server.is_platform && server.user_id != Some(user_id) {
        return Err(ApiError(McpError::Forbidden("this server does not belong to you".into())));
    }
    if server.auth_type != "oauth2" {
        return Err(ApiError(McpError::BadRequest(format!(
            "OAuth is only for auth_type='oauth2' servers, not '{}'",
            server.auth_type
        ))));
    }
    Ok(server)
}

/// `POST /api/mcp/servers/{id}/oauth/authorize` — start the OAuth 2.1 flow.
pub async fn authorize(
    State(state): State<AppState>,
    claims: Claims,
    Path(server_id): Path<Uuid>,
    body: Option<Json<AuthorizeRequest>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let user_id = parse_user(&claims)?;
    let body = body.map(|Json(b)| b).unwrap_or_default();
    let server = load_owned_oauth_server(&state, user_id, server_id).await?;
    let (server_id, server_name) = (server.id, server.name.clone());
    let authorization_url =
        begin_authorization(&state, user_id, server, body.redirect_url, body.client_id).await?;
    Ok(Json(json!({
        "server_id": server_id,
        "server_name": server_name,
        "authorization_url": authorization_url,
    })))
}

/// Shared OAuth-authorize core: discover + register a client if needed, then
/// build the signed authorization URL. Used by both the authorize endpoint and
/// the unified connect flow.
pub(crate) async fn begin_authorization(
    state: &AppState,
    user_id: Uuid,
    mut server: repo::McpServer,
    redirect_url: Option<String>,
    pre_client_id: Option<String>,
) -> Result<String, ApiError> {
    let redirect_uri = state
        .mcp
        .config
        .oauth_redirect_uri()
        .ok_or_else(|| ApiError(McpError::NotConfigured("MCP_GATEWAY_PUBLIC_URL is not set".into())))?;

    if !server.oauth_configured() {
        let discovered = oauth::discover_oauth_config(
            &state.mcp.http_client,
            &server.url,
            &redirect_uri,
            pre_client_id.as_deref(),
        )
        .await?;
        repo::update_mcp_server_oauth_config(
            &state.mcp.db,
            server.id,
            &discovered.authorization_endpoint,
            &discovered.token_endpoint,
            discovered.client_id.as_deref(),
            discovered.client_secret.as_deref(),
        )
        .await?;
        server = repo::get_mcp_server_by_id(&state.mcp.db, server.id)
            .await?
            .ok_or_else(|| ApiError(McpError::Internal("server vanished after oauth config".into())))?;
    }

    let (Some(auth_endpoint), Some(client_id)) =
        (server.oauth_authorization_endpoint.as_ref(), server.oauth_client_id.as_ref())
    else {
        return Err(ApiError(McpError::Oauth(
            "dynamic client registration unavailable — supply a client_id".into(),
        )));
    };

    let (verifier, challenge) = oauth::pkce_pair();
    let oauth_state = oauth::OAuthState::new(user_id, server.id, verifier, redirect_url);
    let signed = oauth::sign_state(&oauth_state, &state.mcp.config.oauth_state_signing_key);
    let url = oauth::build_authorize_url(
        auth_endpoint,
        client_id,
        &redirect_uri,
        &signed,
        &challenge,
        &server.url,
    )?;
    Ok(url)
}

#[derive(Debug, Deserialize)]
pub struct CallbackQuery {
    pub code: Option<String>,
    pub state: Option<String>,
    pub error: Option<String>,
    pub error_description: Option<String>,
}

/// `GET /api/mcp/oauth/callback` — public browser redirect target. Exchanges the
/// code for tokens (encrypted), then redirects to the caller's success URL.
pub async fn callback(
    State(state): State<AppState>,
    Query(q): Query<CallbackQuery>,
) -> Response {
    if let Some(err) = q.error {
        let desc = q.error_description.unwrap_or_default();
        return Html(error_page(&format!("Authorization failed: {err} {desc}"))).into_response();
    }
    let (Some(code), Some(signed_state)) = (q.code, q.state) else {
        return Html(error_page("Missing code or state.")).into_response();
    };

    let Some(oauth_state) =
        oauth::verify_state(&signed_state, &state.mcp.config.oauth_state_signing_key)
    else {
        return Html(error_page("Invalid or expired state — restart the authorization flow."))
            .into_response();
    };

    let server = match repo::get_mcp_server_by_id(&state.mcp.db, oauth_state.server_id).await {
        Ok(Some(s)) => s,
        _ => return Html(error_page("Server configuration not found.")).into_response(),
    };
    let Some(token_endpoint) = server.oauth_token_endpoint.as_ref() else {
        return Html(error_page("Server has no token endpoint configured.")).into_response();
    };
    let redirect_uri = match state.mcp.config.oauth_redirect_uri() {
        Some(u) => u,
        None => return Html(error_page("Gateway public URL not configured.")).into_response(),
    };

    let tokens = match oauth::exchange_code(
        &state.mcp.http_client,
        token_endpoint,
        server.oauth_client_id.as_deref(),
        server.oauth_client_secret.as_deref(),
        &code,
        &oauth_state.code_verifier,
        &redirect_uri,
    )
    .await
    {
        Ok(t) => t,
        Err(e) => return Html(error_page(&format!("Token exchange failed: {e}"))).into_response(),
    };

    // Persist encrypted with the delegated user's key.
    let crypto = SecretsCrypto::for_user(oauth_state.user_id);
    let expires_at = tokens
        .expires_in
        .map(|secs| chrono::Utc::now() + chrono::Duration::seconds(secs));
    let access_enc = match crypto.encrypt(&tokens.access_token) {
        Ok(enc) => enc,
        Err(e) => return Html(error_page(&format!("Failed to encrypt token: {e}"))).into_response(),
    };
    let refresh_enc = match tokens.refresh_token.as_ref().map(|r| crypto.encrypt(r)).transpose() {
        Ok(enc) => enc,
        Err(e) => return Html(error_page(&format!("Failed to encrypt token: {e}"))).into_response(),
    };
    if let Err(e) = repo::upsert_mcp_oauth_token(
        &state.mcp.db,
        server.id,
        oauth_state.user_id,
        &access_enc,
        refresh_enc.as_deref(),
        expires_at,
        tokens.scope.as_deref(),
    )
    .await
    {
        return Html(error_page(&format!("Failed to store token: {e}"))).into_response();
    }

    session::invalidate_session_cache(&state.mcp, oauth_state.user_id).await;
    tracing::info!(server = %server.name, user_id = %oauth_state.user_id, "stored mcp oauth token");

    let dest = oauth_state.redirect_url.unwrap_or_else(|| "/".to_string());
    Redirect::to(&dest).into_response()
}

/// `GET /api/mcp/servers/{id}/oauth/status` — token presence + expiry.
pub async fn status(
    State(state): State<AppState>,
    claims: Claims,
    Path(server_id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let user_id = parse_user(&claims)?;
    let server = load_owned_oauth_server(&state, user_id, server_id).await?;
    let token = repo::get_mcp_oauth_token(&state.mcp.db, server_id, user_id).await?;
    Ok(Json(json!({
        "server_id": server.id,
        "server_name": server.name,
        "authorized": token.is_some(),
        "expires_at": token.as_ref().and_then(|t| t.expires_at),
        "scope": token.and_then(|t| t.scope),
    })))
}

/// `DELETE /api/mcp/servers/{id}/oauth/token` — remove the caller's token.
pub async fn revoke(
    State(state): State<AppState>,
    claims: Claims,
    Path(server_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = parse_user(&claims)?;
    load_owned_oauth_server(&state, user_id, server_id).await?;
    if !repo::delete_mcp_oauth_token(&state.mcp.db, server_id, user_id).await? {
        return Err(ApiError(McpError::NotFound("no token to revoke".into())));
    }
    session::invalidate_session_cache(&state.mcp, user_id).await;
    Ok(StatusCode::NO_CONTENT)
}

fn error_page(message: &str) -> String {
    format!(
        "<!DOCTYPE html><html><head><meta charset=\"utf-8\"><title>Connection Error</title></head>\
         <body style=\"font-family:sans-serif;text-align:center;padding:48px\">\
         <h2 style=\"color:#dc2626\">Connection failed</h2><p style=\"color:#666\">{}</p></body></html>",
        html_escape(message)
    )
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

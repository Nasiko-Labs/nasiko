//! MCP OAuth 2.1 — token handling.
//!
//! This step implements the **refresh** half that the session resolver needs to
//! inject `auth_type = 'oauth2'` credentials: decrypt the stored access token,
//! and when it is within 5 minutes of expiry, exchange the refresh token for a
//! new one (re-encrypting + persisting).
//!
//! The **authorization** half (RFC 9728/8414 discovery, RFC 7591 dynamic client
//! registration, PKCE, HMAC-signed `state`, the authorize/callback flow) is added
//! in a later step to this same module.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64URL;
use chrono::{Duration as ChronoDuration, Utc};
use hmac::{Hmac, Mac};
use nasiko_secrets::SecretsCrypto;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::error::{McpError, Result};
use crate::provider::first_str;
use crate::repo::{self, McpOAuthToken, McpServer};
use crate::state::McpState;

type HmacSha256 = Hmac<Sha256>;

const REFRESH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);
/// Refresh when the token is within this window of expiring.
const REFRESH_SKEW_MINUTES: i64 = 5;

/// Return the access token to inject for an `oauth2` server, refreshing first if
/// it is near expiry.
///
/// * `Ok(Some(token))` — a usable access token (decrypted plaintext).
/// * `Ok(None)`        — no usable token (decrypt failed, near-expiry with no
///   refresh path, or refresh failed) → the caller **skips** this server, exactly
///   like the PoC's `build_servers_list`.
pub async fn access_token_for(
    state: &McpState,
    crypto: &SecretsCrypto,
    user_id: Uuid,
    server: &McpServer,
    token: &McpOAuthToken,
) -> Result<Option<String>> {
    let access_plain = match crypto.decrypt(&token.access_token) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(server = %server.name, error = %e, "failed to decrypt oauth access token — skipping server");
            return Ok(None);
        }
    };

    let near_expiry = token
        .expires_at
        .map(|exp| exp <= Utc::now() + ChronoDuration::minutes(REFRESH_SKEW_MINUTES))
        .unwrap_or(false);

    if !near_expiry {
        return Ok(Some(access_plain));
    }

    // Near expiry → must refresh. If we can't, skip the server.
    match refresh(state, crypto, user_id, server, token).await? {
        Some(new_access) => Ok(Some(new_access)),
        None => Ok(None),
    }
}

/// Exchange the refresh token for a new access token, persist it (encrypted),
/// and return the new access token plaintext. Returns `None` if refresh is not
/// possible or fails.
async fn refresh(
    state: &McpState,
    crypto: &SecretsCrypto,
    user_id: Uuid,
    server: &McpServer,
    token: &McpOAuthToken,
) -> Result<Option<String>> {
    let (Some(refresh_enc), Some(token_endpoint)) =
        (token.refresh_token.as_ref(), server.oauth_token_endpoint.as_ref())
    else {
        tracing::warn!(server = %server.name, "oauth token near expiry but no refresh token / token endpoint — skipping server");
        return Ok(None);
    };

    let refresh_plain = match crypto.decrypt(refresh_enc) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(server = %server.name, error = %e, "failed to decrypt refresh token — skipping server");
            return Ok(None);
        }
    };

    let mut form: Vec<(&str, String)> = vec![
        ("grant_type", "refresh_token".to_string()),
        ("refresh_token", refresh_plain.clone()),
    ];
    if let Some(cid) = &server.oauth_client_id {
        form.push(("client_id", cid.clone()));
    }
    if let Some(secret) = &server.oauth_client_secret {
        form.push(("client_secret", secret.clone()));
    }

    let resp = state
        .http_client
        .post(token_endpoint)
        .timeout(REFRESH_TIMEOUT)
        .form(&form)
        .send()
        .await;

    let body: Value = match resp {
        Ok(r) if r.status().is_success() => match r.json().await {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(server = %server.name, error = %e, "oauth refresh: bad token response body");
                return Ok(None);
            }
        },
        Ok(r) => {
            tracing::warn!(server = %server.name, status = r.status().as_u16(), "oauth refresh rejected by token endpoint");
            return Ok(None);
        }
        Err(e) => {
            tracing::warn!(server = %server.name, error = %e, "oauth refresh request failed");
            return Ok(None);
        }
    };

    let Some(new_access) = first_str(&body, &["access_token"]) else {
        tracing::warn!(server = %server.name, "oauth refresh response missing access_token");
        return Ok(None);
    };
    // Some providers rotate the refresh token; reuse the old one if absent.
    let new_refresh = first_str(&body, &["refresh_token"]).unwrap_or(refresh_plain);
    let scope = first_str(&body, &["scope"]).or_else(|| token.scope.clone());
    let expires_at = body
        .get("expires_in")
        .and_then(|v| v.as_i64())
        .map(|secs| Utc::now() + ChronoDuration::seconds(secs));

    // Persist encrypted with the user-scoped key.
    let access_enc = crypto
        .encrypt(&new_access)
        .map_err(|e| McpError::Internal(format!("encrypt access token: {e}")))?;
    let refresh_enc = crypto
        .encrypt(&new_refresh)
        .map_err(|e| McpError::Internal(format!("encrypt refresh token: {e}")))?;
    repo::upsert_mcp_oauth_token(
        &state.db,
        server.id,
        user_id,
        &access_enc,
        Some(&refresh_enc),
        expires_at,
        scope.as_deref(),
    )
    .await
    .map_err(|e| McpError::Internal(format!("failed to persist refreshed oauth token: {e}")))?;

    tracing::info!(server = %server.name, "refreshed oauth token");
    Ok(Some(new_access))
}

// ═══════════════════════════════════════════════════════════════════════════
// Authorization flow — PKCE, signed state, RFC 9728/8414 discovery, RFC 7591 DCR
// ═══════════════════════════════════════════════════════════════════════════

const DISCOVERY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
/// How long a signed `state` (and thus the authorize flow) stays valid.
const STATE_TTL_MINUTES: i64 = 10;

/// Result of OAuth 2.1 discovery + optional dynamic client registration.
#[derive(Debug, Clone)]
pub struct DiscoveredOAuth {
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    pub client_id: Option<String>,
    pub client_secret: Option<String>,
}

/// Tokens returned by an authorization-code exchange.
#[derive(Debug, Clone)]
pub struct ExchangedTokens {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_in: Option<i64>,
    pub scope: Option<String>,
}

/// The signed, tamper-proof `state` carried through the OAuth redirect. Bound to
/// the initiating user + server and the PKCE verifier, with an expiry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthState {
    pub user_id: Uuid,
    pub server_id: Uuid,
    pub code_verifier: String,
    pub redirect_url: Option<String>,
    /// Unix seconds expiry.
    pub exp: i64,
}

impl OAuthState {
    /// Build a state for `(user, server)` with a fresh expiry.
    pub fn new(user_id: Uuid, server_id: Uuid, code_verifier: String, redirect_url: Option<String>) -> Self {
        Self {
            user_id,
            server_id,
            code_verifier,
            redirect_url,
            exp: (Utc::now() + ChronoDuration::minutes(STATE_TTL_MINUTES)).timestamp(),
        }
    }
}

/// Generate a PKCE `(code_verifier, code_challenge)` pair (S256).
pub fn pkce_pair() -> (String, String) {
    let mut buf = [0u8; 48];
    rand::rng().fill_bytes(&mut buf);
    let verifier = B64URL.encode(buf);
    let challenge = B64URL.encode(Sha256::digest(verifier.as_bytes()));
    (verifier, challenge)
}

/// Sign a state into a URL-safe, tamper-proof token: `base64url({b, s})` where
/// `b` is the compact-JSON payload and `s` is its HMAC-SHA256 (hex).
pub fn sign_state(state: &OAuthState, key: &str) -> String {
    let body = serde_json::to_string(state).unwrap_or_default();
    let mut mac = HmacSha256::new_from_slice(key.as_bytes()).expect("HMAC accepts any key length");
    mac.update(body.as_bytes());
    let sig = hex::encode(mac.finalize().into_bytes());
    let wrapped = json!({ "b": body, "s": sig });
    B64URL.encode(serde_json::to_string(&wrapped).unwrap_or_default())
}

/// Verify a signed state: checks the HMAC (constant-time) and expiry. Returns the
/// payload only if both pass.
pub fn verify_state(token: &str, key: &str) -> Option<OAuthState> {
    let decoded = B64URL.decode(token).ok()?;
    let wrapped: Value = serde_json::from_slice(&decoded).ok()?;
    let body = wrapped.get("b")?.as_str()?;
    let sig_hex = wrapped.get("s")?.as_str()?;
    let expected_sig = hex::decode(sig_hex).ok()?;

    let mut mac = HmacSha256::new_from_slice(key.as_bytes()).ok()?;
    mac.update(body.as_bytes());
    mac.verify_slice(&expected_sig).ok()?; // constant-time

    let state: OAuthState = serde_json::from_str(body).ok()?;
    if state.exp < Utc::now().timestamp() {
        return None;
    }
    Some(state)
}

/// RFC 9728 protected-resource discovery + RFC 8414 AS metadata + RFC 7591
/// dynamic client registration.
///
/// 1. Probe the MCP server → expect 401, parse `resource_metadata` from
///    `WWW-Authenticate`.
/// 2. Fetch Protected Resource Metadata → `authorization_servers[0]`.
/// 3. Fetch Authorization Server Metadata (`.well-known/oauth-authorization-server`).
/// 4. Register a client (DCR) unless `pre_client_id` is supplied or there is no
///    `registration_endpoint`.
pub async fn discover_oauth_config(
    http: &reqwest::Client,
    mcp_url: &str,
    redirect_uri: &str,
    pre_client_id: Option<&str>,
) -> Result<DiscoveredOAuth> {
    // Step 1 — probe.
    let probe = http
        .post(mcp_url)
        .timeout(DISCOVERY_TIMEOUT)
        .header("Content-Type", "application/json")
        .body(
            json!({
                "jsonrpc": "2.0", "id": 1, "method": "initialize",
                "params": {"protocolVersion": "2024-11-05", "capabilities": {},
                           "clientInfo": {"name": "mcp-gateway", "version": "1.0"}},
            })
            .to_string(),
        )
        .send()
        .await?;

    if probe.status() != reqwest::StatusCode::UNAUTHORIZED {
        return Err(McpError::Oauth(format!(
            "expected 401 from OAuth server, got {}. This server may not require OAuth — try auth_type='none' or 'bearer'.",
            probe.status().as_u16()
        )));
    }

    // Step 2 — resource_metadata URL from WWW-Authenticate.
    let www_auth = probe
        .headers()
        .get(reqwest::header::WWW_AUTHENTICATE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let rm_url = extract_resource_metadata(&www_auth).ok_or_else(|| {
        McpError::Oauth(
            "server returned 401 but WWW-Authenticate has no resource_metadata URL".to_string(),
        )
    })?;

    // Step 3 — Protected Resource Metadata (RFC 9728).
    let rm: Value = http.get(&rm_url).timeout(DISCOVERY_TIMEOUT).send().await?.json().await?;
    let as_url = rm
        .get("authorization_servers")
        .and_then(|v| v.as_array())
        .and_then(|a| a.first())
        .and_then(|v| v.as_str())
        .ok_or_else(|| McpError::Oauth(format!("no authorization_servers in resource metadata at {rm_url}")))?;

    // Step 4 — Authorization Server Metadata (RFC 8414).
    let as_meta_url = format!("{}/.well-known/oauth-authorization-server", as_url.trim_end_matches('/'));
    let as_meta: Value = http.get(&as_meta_url).timeout(DISCOVERY_TIMEOUT).send().await?.json().await?;
    let authorization_endpoint = first_str(&as_meta, &["authorization_endpoint"])
        .ok_or_else(|| McpError::Oauth(format!("AS metadata at {as_meta_url} missing authorization_endpoint")))?;
    let token_endpoint = first_str(&as_meta, &["token_endpoint"])
        .ok_or_else(|| McpError::Oauth(format!("AS metadata at {as_meta_url} missing token_endpoint")))?;
    let registration_endpoint = first_str(&as_meta, &["registration_endpoint"]);

    // Step 5 — Dynamic Client Registration (RFC 7591), unless a client_id was supplied.
    let mut client_id = pre_client_id.map(str::to_string);
    let mut client_secret = None;
    if client_id.is_none()
        && let Some(reg_endpoint) = registration_endpoint
    {
        let reg = http
            .post(&reg_endpoint)
            .timeout(DISCOVERY_TIMEOUT)
            .json(&json!({
                "client_name": "MCP Gateway",
                "redirect_uris": [redirect_uri],
                "grant_types": ["authorization_code", "refresh_token"],
                "response_types": ["code"],
                "token_endpoint_auth_method": "none",
            }))
            .send()
            .await?;
        if reg.status().is_success() {
            let reg_data: Value = reg.json().await?;
            client_id = first_str(&reg_data, &["client_id"]);
            client_secret = first_str(&reg_data, &["client_secret"]);
        } else {
            tracing::warn!(
                status = reg.status().as_u16(),
                "dynamic client registration failed — a client_id must be supplied manually"
            );
        }
    }

    Ok(DiscoveredOAuth { authorization_endpoint, token_endpoint, client_id, client_secret })
}

/// Build the authorization URL the user opens in their browser.
pub fn build_authorize_url(
    authorization_endpoint: &str,
    client_id: &str,
    redirect_uri: &str,
    state: &str,
    code_challenge: &str,
    resource: &str,
) -> Result<String> {
    let mut url = reqwest::Url::parse(authorization_endpoint)
        .map_err(|e| McpError::Oauth(format!("invalid authorization_endpoint: {e}")))?;
    url.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", client_id)
        .append_pair("redirect_uri", redirect_uri)
        .append_pair("state", state)
        .append_pair("code_challenge", code_challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("resource", resource);
    Ok(url.to_string())
}

/// Exchange an authorization code for tokens (the callback step).
pub async fn exchange_code(
    http: &reqwest::Client,
    token_endpoint: &str,
    client_id: Option<&str>,
    client_secret: Option<&str>,
    code: &str,
    code_verifier: &str,
    redirect_uri: &str,
) -> Result<ExchangedTokens> {
    let mut form: Vec<(&str, String)> = vec![
        ("grant_type", "authorization_code".to_string()),
        ("code", code.to_string()),
        ("redirect_uri", redirect_uri.to_string()),
        ("code_verifier", code_verifier.to_string()),
    ];
    if let Some(cid) = client_id {
        form.push(("client_id", cid.to_string()));
    }
    if let Some(secret) = client_secret {
        form.push(("client_secret", secret.to_string()));
    }

    let resp = http
        .post(token_endpoint)
        .timeout(DISCOVERY_TIMEOUT)
        .form(&form)
        .send()
        .await?;
    if !resp.status().is_success() {
        let code = resp.status().as_u16();
        let body = resp.text().await.unwrap_or_default();
        // Char-boundary-safe truncation — byte-slicing `&body[..300]` would panic
        // if a multibyte UTF-8 char straddles byte 300 (e.g. a non-ASCII error body).
        let snippet: String = body.chars().take(300).collect();
        return Err(McpError::Oauth(format!("token exchange failed (HTTP {code}): {snippet}")));
    }

    let body: Value = resp.json().await?;
    let access_token = first_str(&body, &["access_token"])
        .ok_or_else(|| McpError::Oauth("token response missing access_token".to_string()))?;
    Ok(ExchangedTokens {
        access_token,
        refresh_token: first_str(&body, &["refresh_token"]),
        expires_in: body.get("expires_in").and_then(|v| v.as_i64()),
        scope: first_str(&body, &["scope"]),
    })
}

// ═══════════════════════════════════════════════════════════════════════════
// Management — per-server authorize / callback / status orchestration behind
// `/api/mcp/servers/{id}/oauth/*` and the public callback.
// ═══════════════════════════════════════════════════════════════════════════

/// Load an `oauth2`-type server and confirm `user_id` may manage it (platform:
/// any authed user; user-scoped: the owner only).
pub async fn load_owned_oauth_server(state: &McpState, user_id: Uuid, server_id: Uuid) -> Result<McpServer> {
    let server = repo::get_mcp_server_by_id(&state.db, server_id)
        .await?
        .ok_or_else(|| McpError::NotFound(format!("MCP server '{server_id}' not found")))?;
    if !server.is_platform && server.user_id != Some(user_id) {
        return Err(McpError::Forbidden("this server does not belong to you".into()));
    }
    if server.auth_type != "oauth2" {
        return Err(McpError::BadRequest(format!(
            "OAuth is only for auth_type='oauth2' servers, not '{}'",
            server.auth_type
        )));
    }
    Ok(server)
}

/// Shared OAuth-authorize core: discover + register a client if needed, then
/// build the signed authorization URL. Used by both the authorize endpoint and
/// the unified connect flow.
pub async fn begin_authorization(
    state: &McpState,
    user_id: Uuid,
    mut server: McpServer,
    redirect_url: Option<String>,
    pre_client_id: Option<String>,
) -> Result<String> {
    let redirect_uri = state
        .config
        .oauth_redirect_uri()
        .ok_or_else(|| McpError::NotConfigured("MCP_GATEWAY_PUBLIC_URL is not set".into()))?;

    if !server.oauth_configured() {
        let discovered =
            discover_oauth_config(&state.http_client, &server.url, &redirect_uri, pre_client_id.as_deref()).await?;
        repo::update_mcp_server_oauth_config(
            &state.db,
            server.id,
            &discovered.authorization_endpoint,
            &discovered.token_endpoint,
            discovered.client_id.as_deref(),
            discovered.client_secret.as_deref(),
        )
        .await?;
        server = repo::get_mcp_server_by_id(&state.db, server.id)
            .await?
            .ok_or_else(|| McpError::Internal("server vanished after oauth config".into()))?;
    }

    let (Some(auth_endpoint), Some(client_id)) =
        (server.oauth_authorization_endpoint.as_ref(), server.oauth_client_id.as_ref())
    else {
        return Err(McpError::Oauth("dynamic client registration unavailable — supply a client_id".into()));
    };

    let (verifier, challenge) = pkce_pair();
    let oauth_state = OAuthState::new(user_id, server.id, verifier, redirect_url);
    let signed = sign_state(&oauth_state, &state.config.oauth_state_signing_key);
    build_authorize_url(auth_endpoint, client_id, &redirect_uri, &signed, &challenge, &server.url)
}

/// Outcome of the public OAuth callback for the server to render.
pub enum CallbackOutcome {
    /// Success — redirect the browser here.
    Redirect(String),
    /// Something went wrong — show this message as an HTML error page.
    Message(String),
}

/// `GET /api/mcp/oauth/callback` core: verify state, exchange the code for
/// tokens, persist them (encrypted), invalidate the session cache, and report
/// where to redirect.
pub async fn handle_callback(
    state: &McpState,
    code: Option<String>,
    signed_state: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
) -> CallbackOutcome {
    if let Some(err) = error {
        let desc = error_description.unwrap_or_default();
        return CallbackOutcome::Message(format!("Authorization failed: {err} {desc}"));
    }
    let (Some(code), Some(signed_state)) = (code, signed_state) else {
        return CallbackOutcome::Message("Missing code or state.".to_string());
    };

    let Some(oauth_state) = verify_state(&signed_state, &state.config.oauth_state_signing_key) else {
        return CallbackOutcome::Message("Invalid or expired state — restart the authorization flow.".to_string());
    };

    let server = match repo::get_mcp_server_by_id(&state.db, oauth_state.server_id).await {
        Ok(Some(s)) => s,
        _ => return CallbackOutcome::Message("Server configuration not found.".to_string()),
    };
    let Some(token_endpoint) = server.oauth_token_endpoint.as_ref() else {
        return CallbackOutcome::Message("Server has no token endpoint configured.".to_string());
    };
    let redirect_uri = match state.config.oauth_redirect_uri() {
        Some(u) => u,
        None => return CallbackOutcome::Message("Gateway public URL not configured.".to_string()),
    };

    let tokens = match exchange_code(
        &state.http_client,
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
        Err(e) => return CallbackOutcome::Message(format!("Token exchange failed: {e}")),
    };

    // Persist encrypted with the delegated user's key.
    let crypto = SecretsCrypto::for_user(oauth_state.user_id);
    let expires_at = tokens.expires_in.map(|secs| Utc::now() + ChronoDuration::seconds(secs));
    let access_enc = match crypto.encrypt(&tokens.access_token) {
        Ok(enc) => enc,
        Err(e) => return CallbackOutcome::Message(format!("Failed to encrypt token: {e}")),
    };
    let refresh_enc = match tokens.refresh_token.as_ref().map(|r| crypto.encrypt(r)).transpose() {
        Ok(enc) => enc,
        Err(e) => return CallbackOutcome::Message(format!("Failed to encrypt token: {e}")),
    };
    if let Err(e) = repo::upsert_mcp_oauth_token(
        &state.db,
        server.id,
        oauth_state.user_id,
        &access_enc,
        refresh_enc.as_deref(),
        expires_at,
        tokens.scope.as_deref(),
    )
    .await
    {
        return CallbackOutcome::Message(format!("Failed to store token: {e}"));
    }

    crate::session::invalidate_session_cache(state, oauth_state.user_id).await;
    tracing::info!(server = %server.name, user_id = %oauth_state.user_id, "stored mcp oauth token");

    CallbackOutcome::Redirect(oauth_state.redirect_url.unwrap_or_else(|| "/".to_string()))
}

/// Parse `resource_metadata="<url>"` out of a `WWW-Authenticate` header value.
fn extract_resource_metadata(www_authenticate: &str) -> Option<String> {
    let marker = "resource_metadata=";
    let start = www_authenticate.find(marker)? + marker.len();
    let rest = &www_authenticate[start..];
    let rest = rest.trim_start_matches('"');
    let end = rest.find('"').unwrap_or(rest.len());
    Some(rest[..end].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_round_trips_and_rejects_tamper() {
        let key = "test-signing-key";
        let state = OAuthState::new(Uuid::nil(), Uuid::nil(), "verifier123".into(), None);
        let signed = sign_state(&state, key);
        assert!(verify_state(&signed, key).is_some());
        // Wrong key → rejected.
        assert!(verify_state(&signed, "other-key").is_none());
        // Tampered token → rejected.
        assert!(verify_state(&format!("{signed}x"), key).is_none());
    }

    #[test]
    fn pkce_challenge_is_s256_of_verifier() {
        let (verifier, challenge) = pkce_pair();
        let expected = B64URL.encode(Sha256::digest(verifier.as_bytes()));
        assert_eq!(challenge, expected);
    }

    #[test]
    fn extracts_resource_metadata_url() {
        let h = r#"Bearer resource_metadata="https://as.example.com/.well-known/x", error="x""#;
        assert_eq!(
            extract_resource_metadata(h).as_deref(),
            Some("https://as.example.com/.well-known/x")
        );
    }
}

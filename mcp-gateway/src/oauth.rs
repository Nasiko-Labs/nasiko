//! MCP OAuth 2.1 — token refresh + the authorization flow.
//!
//! Tokens live on `mcp_user_connections` (one row per user+connector). OAuth
//! endpoint/client config lives on the connector. Pure crypto/HTTP helpers
//! (PKCE, signed state, RFC 9728/8414 discovery, RFC 7591 DCR, code exchange)
//! are backend-agnostic.

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
use crate::provider::generic::MCP_PROTOCOL_VERSION;
use crate::repo::{self, McpConnector, McpUserConnection};
use crate::state::McpState;

type HmacSha256 = Hmac<Sha256>;

const REFRESH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);
const REFRESH_SKEW_MINUTES: i64 = 5;

/// Return the access token to inject for an `oauth2` connector, refreshing first
/// if near expiry. `Ok(None)` → caller skips this connector.
pub async fn access_token_for(
    state: &McpState,
    crypto: &SecretsCrypto,
    user_id: Uuid,
    connector: &McpConnector,
    conn: &McpUserConnection,
) -> Result<Option<String>> {
    let Some(access_enc) = conn.encrypted_credential.as_deref() else {
        return Ok(None);
    };
    let access_plain = match crypto.decrypt(access_enc) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(connector = %connector.name, error = %e, "failed to decrypt oauth access token — skipping");
            return Ok(None);
        }
    };

    let near_expiry = conn
        .token_expires_at
        .map(|exp| exp <= Utc::now() + ChronoDuration::minutes(REFRESH_SKEW_MINUTES))
        .unwrap_or(false);

    if !near_expiry {
        return Ok(Some(access_plain));
    }
    refresh(state, crypto, user_id, connector, conn).await
}

/// Exchange the refresh token for a new access token, persist encrypted, return it.
async fn refresh(
    state: &McpState,
    crypto: &SecretsCrypto,
    user_id: Uuid,
    connector: &McpConnector,
    conn: &McpUserConnection,
) -> Result<Option<String>> {
    let (Some(refresh_enc), Some(token_endpoint)) = (
        conn.encrypted_refresh_token.as_ref(),
        connector.oauth_token_endpoint.as_ref(),
    ) else {
        tracing::warn!(connector = %connector.name, "oauth near expiry but no refresh token / endpoint — skipping");
        return Ok(None);
    };

    let refresh_plain = match crypto.decrypt(refresh_enc) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(connector = %connector.name, error = %e, "failed to decrypt refresh token — skipping");
            return Ok(None);
        }
    };

    let mut form: Vec<(&str, String)> = vec![
        ("grant_type", "refresh_token".to_string()),
        ("refresh_token", refresh_plain.clone()),
    ];
    if let Some(cid) = &connector.oauth_client_id {
        form.push(("client_id", cid.clone()));
    }
    if let Some(secret) = client_secret_plain(connector) {
        form.push(("client_secret", secret));
    }

    // Guarded client: the token endpoint was discovered from the target server's
    // response, so it must be SSRF-checked, not fetched with the internal client.
    let mut req = state
        .guarded_http_client
        .post(token_endpoint)
        .timeout(REFRESH_TIMEOUT)
        .form(&form);
    if let (Some(cid), Some(secret)) = (&connector.oauth_client_id, client_secret_plain(connector))
    {
        req = req.basic_auth(cid, Some(secret));
    }
    let resp = req.send().await;
    let body: Value = match resp {
        Ok(r) if r.status().is_success() => match r.json().await {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(connector = %connector.name, error = %e, "oauth refresh: bad token response");
                return Ok(None);
            }
        },
        Ok(r) => {
            tracing::warn!(connector = %connector.name, status = r.status().as_u16(), "oauth refresh rejected");
            return Ok(None);
        }
        Err(e) => {
            tracing::warn!(connector = %connector.name, error = %e, "oauth refresh request failed");
            return Ok(None);
        }
    };

    let Some(new_access) = first_str(&body, &["access_token"]) else {
        tracing::warn!(connector = %connector.name, "oauth refresh response missing access_token");
        return Ok(None);
    };
    let new_refresh = first_str(&body, &["refresh_token"]).unwrap_or(refresh_plain);
    let scope = first_str(&body, &["scope"]).or_else(|| conn.scope.clone());
    let expires_at = body
        .get("expires_in")
        .and_then(|v| v.as_i64())
        .map(|s| Utc::now() + ChronoDuration::seconds(s));

    let access_enc = crypto.encrypt(&new_access);
    let refresh_enc = crypto.encrypt(&new_refresh);
    repo::upsert_connection_oauth_token(
        &state.db,
        user_id,
        connector.id,
        &access_enc,
        Some(&refresh_enc),
        expires_at,
        scope.as_deref(),
    )
    .await
    .map_err(|e| McpError::Internal(format!("failed to persist refreshed oauth token: {e}")))?;

    tracing::info!(connector = %connector.name, "refreshed oauth token");
    Ok(Some(new_access))
}

// ═══════════════════════════════════════════════════════════════════════════
// Authorization flow — PKCE, signed state, discovery, DCR (backend-agnostic).
// ═══════════════════════════════════════════════════════════════════════════

const DISCOVERY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
const STATE_TTL_MINUTES: i64 = 10;

#[derive(Debug, Clone)]
pub struct DiscoveredOAuth {
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    pub client_id: Option<String>,
    pub client_secret: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ExchangedTokens {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_in: Option<i64>,
    pub scope: Option<String>,
}

/// Signed, tamper-proof `state` carried through the OAuth redirect.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthState {
    pub user_id: Uuid,
    pub connector_id: Uuid,
    pub code_verifier: String,
    pub redirect_url: Option<String>,
    pub exp: i64,
}

impl OAuthState {
    pub fn new(
        user_id: Uuid,
        connector_id: Uuid,
        code_verifier: String,
        redirect_url: Option<String>,
    ) -> Self {
        Self {
            user_id,
            connector_id,
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

/// Sign a state into a URL-safe, tamper-proof token.
pub fn sign_state(state: &OAuthState, key: &str) -> String {
    let body = serde_json::to_string(state).unwrap_or_default();
    let mut mac = HmacSha256::new_from_slice(key.as_bytes()).expect("HMAC accepts any key length");
    mac.update(body.as_bytes());
    let sig = hex::encode(mac.finalize().into_bytes());
    let wrapped = json!({ "b": body, "s": sig });
    B64URL.encode(serde_json::to_string(&wrapped).unwrap_or_default())
}

/// Verify a signed state (constant-time HMAC + expiry).
pub fn verify_state(token: &str, key: &str) -> Option<OAuthState> {
    let decoded = B64URL.decode(token).ok()?;
    let wrapped: Value = serde_json::from_slice(&decoded).ok()?;
    let body = wrapped.get("b")?.as_str()?;
    let sig_hex = wrapped.get("s")?.as_str()?;
    let expected_sig = hex::decode(sig_hex).ok()?;

    let mut mac = HmacSha256::new_from_slice(key.as_bytes()).ok()?;
    mac.update(body.as_bytes());
    mac.verify_slice(&expected_sig).ok()?;

    let state: OAuthState = serde_json::from_str(body).ok()?;
    if state.exp < Utc::now().timestamp() {
        return None;
    }
    Some(state)
}

/// RFC 9728 + RFC 8414 discovery + RFC 7591 dynamic client registration.
pub async fn discover_oauth_config(
    http: &reqwest::Client,
    mcp_url: &str,
    redirect_uri: &str,
    pre_client_id: Option<&str>,
) -> Result<DiscoveredOAuth> {
    let probe = http
        .post(mcp_url)
        .timeout(DISCOVERY_TIMEOUT)
        .header("Content-Type", "application/json")
        .body(
            json!({
                "jsonrpc": "2.0", "id": 1, "method": "initialize",
                "params": {"protocolVersion": MCP_PROTOCOL_VERSION, "capabilities": {},
                           "clientInfo": {"name": "mcp-gateway", "version": "1.0"}},
            })
            .to_string(),
        )
        .send()
        .await?;

    if probe.status() != reqwest::StatusCode::UNAUTHORIZED {
        return Err(McpError::Oauth(format!(
            "expected 401 from OAuth server, got {}. Try auth_type='none' or 'bearer'.",
            probe.status().as_u16()
        )));
    }

    let www_auth = probe
        .headers()
        .get(reqwest::header::WWW_AUTHENTICATE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();

    // Primary: the header explicitly points at the resource metadata document.
    // Fallback: some real OAuth-capable servers (confirmed: Atlassian) never
    // include the resource_metadata pointer at all — try the well-known
    // endpoint directly instead of giving up, since that's the spec's own
    // primary discovery method, not really a fallback in principle.
    let rm: Value = if let Some(rm_url) = extract_resource_metadata(&www_auth) {
        http.get(&rm_url)
            .timeout(DISCOVERY_TIMEOUT)
            .send()
            .await?
            .json()
            .await?
    } else {
        fetch_protected_resource_metadata(http, mcp_url)
            .await
            .ok_or_else(|| {
                McpError::Oauth(
                    "401 has no resource_metadata URL in WWW-Authenticate, and no RFC 9728 \
                 well-known endpoint was found either — this server may not actually \
                 support OAuth 2.1, or doesn't publish discovery metadata"
                        .to_string(),
                )
            })?
    };
    let as_url = rm
        .get("authorization_servers")
        .and_then(|v| v.as_array())
        .and_then(|a| a.first())
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            McpError::Oauth("no authorization_servers in resource metadata document".to_string())
        })?;

    let as_meta_url = format!(
        "{}/.well-known/oauth-authorization-server",
        as_url.trim_end_matches('/')
    );
    let as_meta: Value = http
        .get(&as_meta_url)
        .timeout(DISCOVERY_TIMEOUT)
        .send()
        .await?
        .json()
        .await?;
    let authorization_endpoint =
        first_str(&as_meta, &["authorization_endpoint"]).ok_or_else(|| {
            McpError::Oauth(format!(
                "AS metadata at {as_meta_url} missing authorization_endpoint"
            ))
        })?;
    let token_endpoint = first_str(&as_meta, &["token_endpoint"]).ok_or_else(|| {
        McpError::Oauth(format!(
            "AS metadata at {as_meta_url} missing token_endpoint"
        ))
    })?;
    let registration_endpoint = first_str(&as_meta, &["registration_endpoint"]);

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
                "dynamic client registration failed"
            );
        }
    }

    Ok(DiscoveredOAuth {
        authorization_endpoint,
        token_endpoint,
        client_id,
        client_secret,
    })
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

    let mut req = http
        .post(token_endpoint)
        .timeout(DISCOVERY_TIMEOUT)
        .form(&form);
    // Some providers (e.g. Notion) require client credentials as a Basic auth
    // header rather than form fields. Send both — providers that use form
    // fields ignore the header, and those that require Basic auth need it.
    if let (Some(cid), Some(secret)) = (client_id, client_secret) {
        req = req.basic_auth(cid, Some(secret));
    }
    let resp = req.send().await?;
    if !resp.status().is_success() {
        let code = resp.status().as_u16();
        let body = resp.text().await.unwrap_or_default();
        let snippet: String = body.chars().take(300).collect();
        return Err(McpError::Oauth(format!(
            "token exchange failed (HTTP {code}): {snippet}"
        )));
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
// Management — authorize / callback orchestration behind the connector routes.
// ═══════════════════════════════════════════════════════════════════════════

/// Decrypt a connector's stored OAuth client secret (encrypted at rest with the
/// owner's key, mirroring the other credential columns). `None` if absent or on
/// decrypt failure.
fn client_secret_plain(connector: &McpConnector) -> Option<String> {
    let enc = connector.oauth_client_secret.as_deref()?;
    let owner = connector.owner_id?;
    match SecretsCrypto::for_user(owner).decrypt(enc) {
        Ok(v) => Some(v),
        Err(e) => {
            tracing::warn!(connector = %connector.name, error = %e, "failed to decrypt oauth client secret");
            None
        }
    }
}

/// Load an `oauth2` connector and confirm `user_id` may reach it (owner/grant).
pub async fn load_accessible_oauth_connector(
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
    if connector.auth_type.as_deref() != Some("oauth2") {
        return Err(McpError::BadRequest(
            "OAuth is only for auth_type='oauth2' connectors".into(),
        ));
    }
    Ok(connector)
}

/// Discover + register a client if needed, then build the signed authorization URL.
pub async fn begin_authorization(
    state: &McpState,
    user_id: Uuid,
    mut connector: McpConnector,
    redirect_url: Option<String>,
    pre_client_id: Option<String>,
) -> Result<String> {
    let redirect_uri = state
        .config
        .oauth_redirect_uri()
        .ok_or_else(|| McpError::NotConfigured("MCP_GATEWAY_PUBLIC_URL is not set".into()))?;
    let server_url = connector
        .url
        .clone()
        .ok_or_else(|| McpError::BadRequest("connector has no url".into()))?;

    if !connector.oauth_configured() {
        // Guarded client: discovery follows URLs from the target server's own
        // response (resource_metadata, authorization_servers, endpoints), which
        // are attacker-influenced and must be SSRF-checked.
        let discovered = discover_oauth_config(
            &state.guarded_http_client,
            &server_url,
            &redirect_uri,
            pre_client_id
                .as_deref()
                .or(connector.oauth_client_id.as_deref()),
        )
        .await?;
        // Encrypt the DCR client secret at rest with the owner's key.
        let client_secret_enc = match (discovered.client_secret.as_deref(), connector.owner_id) {
            (Some(sec), Some(owner)) => Some(SecretsCrypto::for_user(owner).encrypt(sec)),
            _ => None,
        };
        repo::update_connector_oauth_config(
            &state.db,
            connector.id,
            &discovered.authorization_endpoint,
            &discovered.token_endpoint,
            discovered.client_id.as_deref(),
            client_secret_enc.as_deref(),
        )
        .await?;
        connector = repo::get_connector_by_id(&state.db, connector.id)
            .await?
            .ok_or_else(|| McpError::Internal("connector vanished after oauth config".into()))?;
    }

    let (Some(auth_endpoint), Some(client_id)) = (
        connector.oauth_authorization_endpoint.as_ref(),
        connector.oauth_client_id.as_ref(),
    ) else {
        return Err(McpError::Oauth(
            "dynamic client registration unavailable — supply a client_id".into(),
        ));
    };

    let (verifier, challenge) = pkce_pair();
    let oauth_state = OAuthState::new(user_id, connector.id, verifier, redirect_url);
    let signed = sign_state(&oauth_state, &state.config.oauth_state_signing_key);
    build_authorize_url(
        auth_endpoint,
        client_id,
        &redirect_uri,
        &signed,
        &challenge,
        &server_url,
    )
}

/// Outcome of the public OAuth callback for the server to render.
pub enum CallbackOutcome {
    Redirect(String),
    Message(String),
}

/// OAuth callback core: verify state, exchange code, persist tokens, invalidate cache.
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

    let Some(oauth_state) = verify_state(&signed_state, &state.config.oauth_state_signing_key)
    else {
        return CallbackOutcome::Message(
            "Invalid or expired state — restart the authorization flow.".to_string(),
        );
    };

    let connector = match repo::get_connector_by_id(&state.db, oauth_state.connector_id).await {
        Ok(Some(c)) => c,
        _ => return CallbackOutcome::Message("Connector configuration not found.".to_string()),
    };
    let Some(token_endpoint) = connector.oauth_token_endpoint.as_ref() else {
        return CallbackOutcome::Message("Connector has no token endpoint configured.".to_string());
    };
    let redirect_uri = match state.config.oauth_redirect_uri() {
        Some(u) => u,
        None => return CallbackOutcome::Message("Gateway public URL not configured.".to_string()),
    };

    let client_secret = client_secret_plain(&connector);
    let tokens = match exchange_code(
        &state.guarded_http_client,
        token_endpoint,
        connector.oauth_client_id.as_deref(),
        client_secret.as_deref(),
        &code,
        &oauth_state.code_verifier,
        &redirect_uri,
    )
    .await
    {
        Ok(t) => t,
        // Do not echo the token-endpoint response body back to the browser.
        Err(e) => {
            tracing::warn!(error = %e, "oauth token exchange failed");
            let _ = repo::set_connector_setup_status(
                &state.db,
                connector.id,
                "failed",
                Some("token exchange failed"),
            )
            .await;
            return CallbackOutcome::Message(
                "Token exchange failed — please restart the authorization flow.".to_string(),
            );
        }
    };

    let crypto = SecretsCrypto::for_user(oauth_state.user_id);
    let expires_at = tokens
        .expires_in
        .map(|secs| Utc::now() + ChronoDuration::seconds(secs));
    let access_enc = crypto.encrypt(&tokens.access_token);
    let refresh_enc = tokens.refresh_token.as_ref().map(|r| crypto.encrypt(r));
    if let Err(e) = repo::upsert_connection_oauth_token(
        &state.db,
        oauth_state.user_id,
        connector.id,
        &access_enc,
        refresh_enc.as_deref(),
        expires_at,
        tokens.scope.as_deref(),
    )
    .await
    {
        let _ = repo::set_connector_setup_status(
            &state.db,
            connector.id,
            "failed",
            Some("failed to store token"),
        )
        .await;
        return CallbackOutcome::Message(format!("Failed to store token: {e}"));
    }

    // A successful token exchange proves the OAuth handshake worked, not that
    // the resulting access token actually authorizes MCP calls against this
    // server — prove that too before calling the connector active.
    let outcome =
        crate::credentials::verify_connector_live(state, oauth_state.user_id, &connector).await;
    let (status, status_error) = if outcome.verified {
        ("active", None)
    } else {
        ("failed", outcome.error)
    };
    let _ =
        repo::set_connector_setup_status(&state.db, connector.id, status, status_error.as_deref())
            .await;
    if outcome.verified {
        crate::connect::grant_user_agents_access(&state.db, oauth_state.user_id, connector.id)
            .await;
    }
    crate::session::invalidate_session_cache(state, oauth_state.user_id).await;
    tracing::info!(
        connector = %connector.name, user_id = %oauth_state.user_id, verified = outcome.verified,
        "stored mcp oauth token"
    );
    let dest = oauth_state.redirect_url.unwrap_or_else(|| "/".to_string());
    CallbackOutcome::Redirect(crate::net::safe_redirect(
        &dest,
        state.config.gateway_public_url.as_deref(),
    ))
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

/// RFC 9728 direct discovery: `GET /.well-known/oauth-protected-resource` at
/// the resource's origin — tried path-aware first (e.g.
/// `/.well-known/oauth-protected-resource/mcp` for a resource at `/mcp`, per
/// RFC 9728 §3.1), then root-only as a fallback for servers that publish it
/// there regardless of path. This is the MCP spec's own PRIMARY discovery
/// method — unlike inferring from a `WWW-Authenticate: resource_metadata=...`
/// header on a bare 401 (`extract_resource_metadata` above), which some real
/// servers omit even though they support OAuth. Confirmed live: Atlassian's
/// hosted MCP server does neither — `/.well-known/oauth-protected-resource`
/// 404s there entirely, a real, currently-open gap on their side
/// (atlassian/atlassian-mcp-server#148), not something this can work around.
/// Check whether the authorization server for an OAuth-protected resource
/// advertises a `registration_endpoint` (RFC 7591 DCR). Used by the probe
/// endpoint so the frontend can decide whether to show client-credential fields.
pub async fn as_supports_dcr(http: &reqwest::Client, resource_metadata: &Value) -> bool {
    let Some(as_url) = resource_metadata
        .get("authorization_servers")
        .and_then(|v| v.as_array())
        .and_then(|a| a.first())
        .and_then(|v| v.as_str())
    else {
        return false;
    };
    let as_meta_url = format!(
        "{}/.well-known/oauth-authorization-server",
        as_url.trim_end_matches('/')
    );
    let Ok(resp) = http.get(&as_meta_url).timeout(DISCOVERY_TIMEOUT).send().await else {
        return false;
    };
    let Ok(doc) = resp.json::<Value>().await else {
        return false;
    };
    doc.get("registration_endpoint")
        .and_then(|v| v.as_str())
        .is_some_and(|s| !s.is_empty())
}

/// Returns the parsed resource metadata document, or `None` if no candidate
/// URL returned a document with a non-empty `authorization_servers` array.
pub async fn fetch_protected_resource_metadata(
    http: &reqwest::Client,
    mcp_url: &str,
) -> Option<Value> {
    let parsed = reqwest::Url::parse(mcp_url).ok()?;
    let origin = format!(
        "{}://{}{}",
        parsed.scheme(),
        parsed.host_str()?,
        parsed.port().map(|p| format!(":{p}")).unwrap_or_default()
    );
    let path = parsed.path().trim_matches('/');

    let mut candidates = Vec::new();
    if !path.is_empty() {
        candidates.push(format!(
            "{origin}/.well-known/oauth-protected-resource/{path}"
        ));
    }
    candidates.push(format!("{origin}/.well-known/oauth-protected-resource"));

    for candidate in candidates {
        let Ok(resp) = http.get(&candidate).timeout(DISCOVERY_TIMEOUT).send().await else {
            continue;
        };
        if !resp.status().is_success() {
            continue;
        }
        let Ok(doc) = resp.json::<Value>().await else {
            continue;
        };
        let has_auth_servers = doc
            .get("authorization_servers")
            .and_then(|v| v.as_array())
            .is_some_and(|a| !a.is_empty());
        if has_auth_servers {
            return Some(doc);
        }
    }
    None
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
        assert!(verify_state(&signed, "other-key").is_none());
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

    #[test]
    fn state_rejects_single_byte_payload_tamper() {
        // Flip one byte inside the signed JSON body (`b`) post-signature,
        // leaving the signature (`s`) untouched — the HMAC must catch this.
        let key = "test-signing-key";
        let state = OAuthState::new(Uuid::new_v4(), Uuid::new_v4(), "verifier123".into(), None);
        let signed = sign_state(&state, key);

        let decoded = B64URL.decode(&signed).unwrap();
        let mut wrapped: Value = serde_json::from_slice(&decoded).unwrap();
        let body = wrapped.get("b").unwrap().as_str().unwrap().to_string();
        let mut bytes = body.into_bytes();
        let idx = bytes.len() / 2;
        bytes[idx] ^= 0x01;
        wrapped["b"] = json!(String::from_utf8_lossy(&bytes).into_owned());
        let tampered = B64URL.encode(serde_json::to_string(&wrapped).unwrap());

        assert!(verify_state(&tampered, key).is_none());
    }

    #[test]
    fn expired_state_is_rejected_even_with_correct_key_and_signature() {
        let key = "test-signing-key";
        let mut state = OAuthState::new(Uuid::nil(), Uuid::nil(), "v".into(), None);
        state.exp = (Utc::now() - ChronoDuration::minutes(1)).timestamp();
        let signed = sign_state(&state, key);
        assert!(verify_state(&signed, key).is_none());
    }

    #[test]
    fn state_body_missing_required_fields_fails_deserialization() {
        // A well-signed body that doesn't deserialize into `OAuthState` (missing
        // `code_verifier` / `exp`) must be rejected, not panic or partially-populate.
        let key = "test-signing-key";
        let body = json!({ "user_id": Uuid::nil(), "connector_id": Uuid::nil() }).to_string();
        let mut mac = HmacSha256::new_from_slice(key.as_bytes()).unwrap();
        mac.update(body.as_bytes());
        let sig = hex::encode(mac.finalize().into_bytes());
        let wrapped = json!({ "b": body, "s": sig });
        let token = B64URL.encode(serde_json::to_string(&wrapped).unwrap());
        assert!(verify_state(&token, key).is_none());
    }

    #[test]
    fn state_wrapper_missing_b_or_s_field_fails_gracefully() {
        let token_missing_sig = B64URL.encode(json!({ "b": "{}" }).to_string());
        assert!(verify_state(&token_missing_sig, "any-key").is_none());
        let token_missing_body = B64URL.encode(json!({ "s": "deadbeef" }).to_string());
        assert!(verify_state(&token_missing_body, "any-key").is_none());
        // Not even valid base64.
        assert!(verify_state("not-valid-base64!!", "any-key").is_none());
    }

    // ─── fetch_protected_resource_metadata (RFC 9728 direct discovery) ─────────

    #[tokio::test]
    async fn discovers_via_path_aware_well_known_endpoint() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("GET", "/.well-known/oauth-protected-resource/mcp")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"resource":"x","authorization_servers":["https://as.example.com"]}"#)
            .create_async()
            .await;

        let client = reqwest::Client::new();
        let doc =
            fetch_protected_resource_metadata(&client, &format!("{}/mcp", server.url())).await;

        mock.assert_async().await;
        assert!(doc.is_some());
        assert_eq!(
            doc.unwrap()["authorization_servers"][0].as_str(),
            Some("https://as.example.com")
        );
    }

    #[tokio::test]
    async fn falls_back_to_root_well_known_endpoint_when_path_aware_404s() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/.well-known/oauth-protected-resource/mcp")
            .with_status(404)
            .create_async()
            .await;
        let root_mock = server
            .mock("GET", "/.well-known/oauth-protected-resource")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"resource":"x","authorization_servers":["https://as.example.com"]}"#)
            .create_async()
            .await;

        let client = reqwest::Client::new();
        let doc =
            fetch_protected_resource_metadata(&client, &format!("{}/mcp", server.url())).await;

        root_mock.assert_async().await;
        assert!(doc.is_some());
    }

    #[tokio::test]
    async fn returns_none_when_neither_well_known_endpoint_exists() {
        // Confirmed live behavior for a real server (Atlassian): both candidates 404.
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/.well-known/oauth-protected-resource/mcp")
            .with_status(404)
            .create_async()
            .await;
        server
            .mock("GET", "/.well-known/oauth-protected-resource")
            .with_status(404)
            .create_async()
            .await;

        let client = reqwest::Client::new();
        let doc =
            fetch_protected_resource_metadata(&client, &format!("{}/mcp", server.url())).await;

        assert!(doc.is_none());
    }

    #[tokio::test]
    async fn returns_none_when_document_has_no_authorization_servers() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/.well-known/oauth-protected-resource/mcp")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"resource":"x","authorization_servers":[]}"#)
            .create_async()
            .await;
        server
            .mock("GET", "/.well-known/oauth-protected-resource")
            .with_status(404)
            .create_async()
            .await;

        let client = reqwest::Client::new();
        let doc =
            fetch_protected_resource_metadata(&client, &format!("{}/mcp", server.url())).await;

        assert!(
            doc.is_none(),
            "an empty authorization_servers array must not count as discovery"
        );
    }
}

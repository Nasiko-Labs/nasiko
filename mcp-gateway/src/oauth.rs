//! MCP OAuth 2.1 — token refresh + the authorization flow.
//!
//! Tokens live on `mcp_user_connections` (one row per user+connector). OAuth
//! endpoint/client config lives on the connector. Pure crypto/HTTP helpers
//! (PKCE, signed state, RFC 9728 + RFC 8414 / OpenID Connect discovery,
//! RFC 7591 DCR, code exchange) are backend-agnostic.

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
use crate::types::ConnectorUnusable;

type HmacSha256 = Hmac<Sha256>;

const REFRESH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);
const REFRESH_SKEW_MINUTES: i64 = 5;

/// Return the access token to inject for an `oauth2` connector, refreshing first
/// if near expiry. `Err(reason)` → caller skips this connector, but now knows
/// *why* instead of just that it's unusable.
pub async fn access_token_for(
    state: &McpState,
    crypto: &SecretsCrypto,
    user_id: Uuid,
    connector: &McpConnector,
    conn: &McpUserConnection,
) -> Result<std::result::Result<String, ConnectorUnusable>> {
    let Some(access_enc) = conn.encrypted_credential.as_deref() else {
        return Ok(Err(ConnectorUnusable::MissingCredential));
    };
    let access_plain = match crypto.decrypt(access_enc) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(connector = %connector.name, error = %e, "failed to decrypt oauth access token — skipping");
            return Ok(Err(ConnectorUnusable::AuthRequired));
        }
    };

    let near_expiry = conn
        .token_expires_at
        .map(|exp| exp <= Utc::now() + ChronoDuration::minutes(REFRESH_SKEW_MINUTES))
        .unwrap_or(false);

    if !near_expiry {
        return Ok(Ok(access_plain));
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
) -> Result<std::result::Result<String, ConnectorUnusable>> {
    let (Some(refresh_enc), Some(token_endpoint)) = (
        conn.encrypted_refresh_token.as_ref(),
        connector.oauth_token_endpoint.as_ref(),
    ) else {
        tracing::warn!(connector = %connector.name, "oauth near expiry but no refresh token / endpoint — skipping");
        return Ok(Err(ConnectorUnusable::AuthRequired));
    };

    let refresh_plain = match crypto.decrypt(refresh_enc) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(connector = %connector.name, error = %e, "failed to decrypt refresh token — skipping");
            return Ok(Err(ConnectorUnusable::AuthRequired));
        }
    };

    let request = RefreshRequest {
        token_endpoint,
        client_id: connector.oauth_client_id.as_deref(),
        client_secret: client_secret_plain(connector),
        refresh_token: &refresh_plain,
    };
    let granted_scope = conn.scope.as_deref().filter(|s| !s.trim().is_empty());
    // Guarded client: the token endpoint was discovered from the target server's
    // response, so it must be SSRF-checked, not fetched with the internal client.
    let body = match send_refresh(
        &state.guarded_http_client,
        &request,
        granted_scope,
        &connector.name,
    )
    .await
    {
        RefreshOutcome::Granted(body) => body,
        RefreshOutcome::Rejected {
            status,
            error,
            error_description,
        } => {
            tracing::warn!(
                connector = %connector.name, status, error, error_description,
                "oauth refresh rejected"
            );
            return Ok(Err(ConnectorUnusable::AuthRequired));
        }
        RefreshOutcome::Failed(reason) => {
            tracing::warn!(connector = %connector.name, reason, "oauth refresh failed");
            return Ok(Err(ConnectorUnusable::AuthRequired));
        }
    };

    let Some(new_access) = first_str(&body, &["access_token"]) else {
        tracing::warn!(connector = %connector.name, "oauth refresh response missing access_token");
        return Ok(Err(ConnectorUnusable::AuthRequired));
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
    Ok(Ok(new_access))
}

/// Refresh exactly as before a scope was ever sent, so every provider that
/// already refreshed gets the same request; only a 400 rejection is retried,
/// once, with the granted scope (RFC 6749 §6 allows it). A rejected grant does
/// not consume the refresh token, so the retry is safe with rotation. Entra ID
/// needs it: without a scope it rejects an app registration that is both the
/// client and the API (AADSTS90009), the simplest way to put an
/// Entra-protected MCP server behind this gateway.
async fn send_refresh(
    http: &reqwest::Client,
    request: &RefreshRequest<'_>,
    granted_scope: Option<&str>,
    connector_name: &str,
) -> RefreshOutcome {
    let outcome = request.send(http, None).await;
    let retry_scope = match &outcome {
        RefreshOutcome::Rejected {
            status: 400,
            error,
            error_description,
        } => granted_scope.inspect(|_| {
            tracing::info!(
                connector = %connector_name, error, error_description,
                "oauth refresh without scope rejected; retrying with the granted scope"
            );
        }),
        _ => None,
    };
    match retry_scope {
        Some(scope) => request.send(http, Some(scope)).await,
        None => outcome,
    }
}

/// One connection's refresh-token grant, with the client secret decrypted
/// once for both the form and the Basic header.
struct RefreshRequest<'a> {
    token_endpoint: &'a str,
    client_id: Option<&'a str>,
    client_secret: Option<String>,
    refresh_token: &'a str,
}

enum RefreshOutcome {
    Granted(Value),
    /// The token endpoint answered with an error status.
    Rejected {
        status: u16,
        error: String,
        error_description: String,
    },
    /// No usable answer: transport failure or an undecodable success body.
    Failed(String),
}

impl RefreshRequest<'_> {
    fn form(&self, scope: Option<&str>) -> Vec<(&'static str, String)> {
        let mut form = vec![
            ("grant_type", "refresh_token".to_string()),
            ("refresh_token", self.refresh_token.to_string()),
        ];
        if let Some(scope) = scope {
            form.push(("scope", scope.to_string()));
        }
        if let Some(client_id) = self.client_id {
            form.push(("client_id", client_id.to_string()));
        }
        if let Some(secret) = &self.client_secret {
            form.push(("client_secret", secret.clone()));
        }
        form
    }

    async fn send(&self, http: &reqwest::Client, scope: Option<&str>) -> RefreshOutcome {
        let mut request = http
            .post(self.token_endpoint)
            .timeout(REFRESH_TIMEOUT)
            .form(&self.form(scope));
        if let (Some(client_id), Some(secret)) = (self.client_id, &self.client_secret) {
            request = request.basic_auth(client_id, Some(secret));
        }
        match request.send().await {
            Ok(response) if response.status().is_success() => match response.json().await {
                Ok(body) => RefreshOutcome::Granted(body),
                Err(e) => RefreshOutcome::Failed(format!("bad token response: {e}")),
            },
            Ok(response) => {
                let status = response.status().as_u16();
                let (error, error_description) =
                    token_error(&response.text().await.unwrap_or_default());
                RefreshOutcome::Rejected {
                    status,
                    error,
                    error_description,
                }
            }
            Err(e) => RefreshOutcome::Failed(format!("request failed: {e}")),
        }
    }
}

const TOKEN_ERROR_DESCRIPTION_MAX_CHARS: usize = 200;

/// RFC 6749 §5.2 `error` / `error_description` of a rejected token request,
/// for the log only: a bare status says nothing about why the provider
/// refused (an Entra ID description names its AADSTS code), and neither field
/// carries a credential. Truncated because providers append trace ids.
fn token_error(body: &str) -> (String, String) {
    let parsed: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let field = |name: &str| first_str(&parsed, &[name]).unwrap_or_default();
    let description = field("error_description")
        .chars()
        .take(TOKEN_ERROR_DESCRIPTION_MAX_CHARS)
        .collect();
    (field("error"), description)
}

// ═══════════════════════════════════════════════════════════════════════════
// Authorization flow — PKCE, signed state, discovery, DCR (backend-agnostic).
// ═══════════════════════════════════════════════════════════════════════════

const DISCOVERY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
const STATE_TTL_MINUTES: i64 = 10;

/// What an MCP server's discovery documents say its authorization server
/// needs: where to send the user, where to redeem the code, whether clients
/// can self-register, and what the authorize request must ask for.
#[derive(Debug, Clone)]
pub struct DiscoveredAuthorization {
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    pub registration_endpoint: Option<String>,
    pub requested_access: RequestedAccess,
}

/// What the authorize request asks for beyond the client's identity. `None`
/// omits the parameter rather than sending it empty.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestedAccess {
    pub scope: Option<String>,
    /// RFC 8707 resource indicator.
    pub resource: Option<String>,
}

impl RequestedAccess {
    /// The request every authorization sent before scope selection existed.
    fn without_scope(server_url: &str) -> Self {
        Self {
            scope: None,
            resource: Some(server_url.to_string()),
        }
    }
}

/// Which discovery document described the authorization server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MetadataSource {
    /// RFC 8414 OAuth 2.0 Authorization Server Metadata.
    OAuth,
    /// OpenID Connect Discovery 1.0 - the only document some identity
    /// providers publish (confirmed live: Entra ID 404s every RFC 8414 URL).
    OpenIdConnect,
}

/// An authorization server's metadata, already checked to name both
/// endpoints the flow cannot run without.
pub struct AuthorizationServerMetadata {
    authorization_endpoint: String,
    token_endpoint: String,
    document: Value,
    source: MetadataSource,
}

/// A client identity to persist on the connector; the secret is already
/// encrypted with the owner's key.
#[derive(Debug, Default)]
struct StoredClient {
    client_id: Option<String>,
    encrypted_secret: Option<String>,
}

/// A client issued by RFC 7591 dynamic registration.
struct RegisteredClient {
    client_id: String,
    client_secret: Option<String>,
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

/// RFC 9728 resource metadata, then the authorization server's RFC 8414 or
/// OpenID Connect metadata, in the MCP spec's order. A server that is down
/// is a `Backend` error; every malformed or missing document is an `Oauth`
/// error - they come from third-party servers, so neither is our 500.
pub async fn discover_authorization(
    http: &reqwest::Client,
    mcp_url: &str,
) -> Result<DiscoveredAuthorization> {
    let challenge = fetch_auth_challenge(http, mcp_url).await?;
    let resource_metadata = match fetch_resource_metadata_for(http, mcp_url, &challenge).await {
        Ok(document) => document,
        Err(no_resource_metadata) => {
            return discover_from_origin(http, mcp_url, &challenge)
                .await
                .ok_or(no_resource_metadata);
        }
    };
    let issuer = first_authorization_server(&resource_metadata).ok_or_else(|| {
        McpError::Oauth("no authorization_servers in resource metadata document".to_string())
    })?;
    let server_metadata = fetch_authorization_server_metadata(http, issuer)
        .await
        .ok_or_else(|| {
            McpError::Oauth(format!(
                "no authorization server metadata found for {issuer} at any RFC 8414 or \
                 OpenID Connect discovery URL"
            ))
        })?;
    let requested_access = RequestedAccess {
        scope: requested_scope(&challenge, &resource_metadata, &server_metadata.document),
        resource: requested_resource(mcp_url, server_metadata.source),
    };
    Ok(discovered_from(server_metadata, requested_access))
}

fn discovered_from(
    server_metadata: AuthorizationServerMetadata,
    requested_access: RequestedAccess,
) -> DiscoveredAuthorization {
    DiscoveredAuthorization {
        registration_endpoint: first_str(&server_metadata.document, &["registration_endpoint"]),
        authorization_endpoint: server_metadata.authorization_endpoint,
        token_endpoint: server_metadata.token_endpoint,
        requested_access,
    }
}

/// The MCP server's own origin as its authorization server, for a server that
/// names no usable resource metadata but publishes authorization server
/// metadata itself (Entra ID behind Azure API Management). `None` when the
/// origin publishes none either.
async fn discover_from_origin(
    http: &reqwest::Client,
    mcp_url: &str,
    challenge: &str,
) -> Option<DiscoveredAuthorization> {
    let server_metadata = fetch_origin_authorization_server_metadata(http, mcp_url).await?;
    let document = &server_metadata.document;
    let scope = match authorization_server_host(mcp_url, document) {
        // The MCP 2025-03-26 spec's clients send no scope unless the
        // challenge names one: a self-hosted server's `scopes_supported` lists
        // everything it can issue, not what this resource needs, and asking
        // for all of it can be rejected as invalid_scope or over-grant consent.
        AuthorizationServerHost::McpServer => {
            challenge_scope(challenge).map(|scope| with_offline_access(scope, document))
        }
        // A proxy publishing a third-party IdP's metadata (Entra ID behind
        // APIM): such IdPs reject a scope-less request, so fall back to the
        // advertised scopes.
        AuthorizationServerHost::ProxiedIdentityProvider => {
            requested_scope(challenge, document, document)
        }
    };
    // No resource metadata means no published resource identifier to assert,
    // the 2025-03-26 spec had no resource indicator at all, and the live
    // proxied case (Entra ID behind APIM) rejects one.
    let requested_access = RequestedAccess {
        scope,
        resource: None,
    };
    Some(discovered_from(server_metadata, requested_access))
}

/// Who issues tokens for metadata read from the MCP server's own origin.
enum AuthorizationServerHost {
    /// The document's `issuer` is the MCP server's origin, or names none.
    McpServer,
    /// The `issuer` is another origin: the server fronts a third-party IdP.
    ProxiedIdentityProvider,
}

fn authorization_server_host(mcp_url: &str, metadata: &Value) -> AuthorizationServerHost {
    let issuer_origin = first_str(metadata, &["issuer"])
        .and_then(|issuer| origin_and_path(&issuer))
        .map(|(origin, _)| origin);
    let mcp_origin = origin_and_path(mcp_url).map(|(origin, _)| origin);
    match issuer_origin {
        Some(issuer_origin) if Some(&issuer_origin) != mcp_origin.as_ref() => {
            AuthorizationServerHost::ProxiedIdentityProvider
        }
        _ => AuthorizationServerHost::McpServer,
    }
}

/// Authorization server metadata published at the MCP server's own origin.
pub async fn fetch_origin_authorization_server_metadata(
    http: &reqwest::Client,
    mcp_url: &str,
) -> Option<AuthorizationServerMetadata> {
    let (origin, _) = origin_and_path(mcp_url)?;
    fetch_authorization_server_metadata(http, &origin).await
}

/// Build the authorization URL the user opens in their browser. Parameters
/// are appended, so any query the endpoint already carries is preserved.
pub fn build_authorize_url(
    authorization_endpoint: &str,
    client_id: &str,
    redirect_uri: &str,
    state: &str,
    code_challenge: &str,
    requested_access: &RequestedAccess,
) -> Result<String> {
    let mut url = reqwest::Url::parse(authorization_endpoint)
        .map_err(|e| McpError::Oauth(format!("invalid authorization_endpoint: {e}")))?;
    {
        let mut query = url.query_pairs_mut();
        query
            .append_pair("response_type", "code")
            .append_pair("client_id", client_id)
            .append_pair("redirect_uri", redirect_uri)
            .append_pair("state", state)
            .append_pair("code_challenge", code_challenge)
            .append_pair("code_challenge_method", "S256");
        if let Some(scope) = &requested_access.scope {
            query.append_pair("scope", scope);
        }
        if let Some(resource) = &requested_access.resource {
            query.append_pair("resource", resource);
        }
    }
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

/// Discover the server's authorization requirements, store a client on first
/// use, then build the signed authorization URL.
pub async fn begin_authorization(
    state: &McpState,
    user_id: Uuid,
    connector: McpConnector,
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

    // Guarded client: discovery follows URLs from the target server's own
    // response (resource_metadata, authorization_servers, endpoints), which
    // are attacker-influenced and must be SSRF-checked.
    let discovery = discover_authorization(&state.guarded_http_client, &server_url).await;
    let (connector, requested_access) = if connector.oauth_configured() {
        let requested_access = requested_access_or_fallback(discovery, &connector, &server_url);
        let requested_access = apply_connector_overrides(requested_access, &connector, &server_url);
        (connector, requested_access)
    } else {
        let discovery = discovery?;
        let connector = store_client_config(
            state,
            connector,
            &discovery,
            &redirect_uri,
            pre_client_id.as_deref(),
        )
        .await?;
        let requested_access =
            apply_connector_overrides(discovery.requested_access, &connector, &server_url);
        (connector, requested_access)
    };

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
        &requested_access,
    )
}

/// A connector configured before scope selection existed authorized without
/// it, so a discovery failure there must not newly break re-authorization:
/// fall back to that original request, and record that a fallback happened.
fn requested_access_or_fallback(
    discovery: Result<DiscoveredAuthorization>,
    connector: &McpConnector,
    server_url: &str,
) -> RequestedAccess {
    match discovery {
        Ok(discovered) => discovered.requested_access,
        Err(e) => {
            tracing::warn!(
                connector = %connector.name, error = %e, fallback_used = true,
                "oauth discovery failed for a configured connector - authorizing without scope selection"
            );
            RequestedAccess::without_scope(server_url)
        }
    }
}

/// The owner's per-connector overrides beat discovery: identity providers
/// behind proxies (Entra ID behind API Management) cannot be classified from
/// metadata alone. An explicit scope replaces the discovered one, but keeps
/// `offline_access` when discovery wanted it, so the grant stays refreshable.
fn apply_connector_overrides(
    requested: RequestedAccess,
    connector: &McpConnector,
    server_url: &str,
) -> RequestedAccess {
    let scope = match connector.oauth_scopes.as_deref().map(str::trim) {
        Some(scopes) if !scopes.is_empty() => Some(keep_offline_access(scopes, &requested.scope)),
        _ => requested.scope,
    };
    let resource = match connector.oauth_send_resource {
        Some(true) => Some(server_url.to_string()),
        Some(false) => None,
        None => requested.resource,
    };
    RequestedAccess { scope, resource }
}

/// `scopes`, plus `offline_access` when the discovered scope carried it and
/// `scopes` does not.
fn keep_offline_access(scopes: &str, discovered: &Option<String>) -> String {
    let has_offline_access =
        |scope: &str| scope.split_whitespace().any(|s| s == OFFLINE_ACCESS_SCOPE);
    let discovery_wanted_it = discovered.as_deref().is_some_and(has_offline_access);
    if discovery_wanted_it && !has_offline_access(scopes) {
        format!("{scopes} {OFFLINE_ACCESS_SCOPE}")
    } else {
        scopes.to_string()
    }
}

/// First authorization only: persist the discovered endpoints and a client -
/// the user's own, else one registered dynamically. Returns the reloaded row.
async fn store_client_config(
    state: &McpState,
    connector: McpConnector,
    discovery: &DiscoveredAuthorization,
    redirect_uri: &str,
    pre_client_id: Option<&str>,
) -> Result<McpConnector> {
    let client = match pre_client_id.or(connector.oauth_client_id.as_deref()) {
        Some(client_id) => StoredClient {
            client_id: Some(client_id.to_string()),
            encrypted_secret: stored_secret_for(&connector, client_id),
        },
        None => dynamically_registered_client(state, &connector, discovery, redirect_uri).await?,
    };
    repo::update_connector_oauth_config(
        &state.db,
        connector.id,
        &discovery.authorization_endpoint,
        &discovery.token_endpoint,
        client.client_id.as_deref(),
        client.encrypted_secret.as_deref(),
    )
    .await?;
    repo::get_connector_by_id(&state.db, connector.id)
        .await?
        .ok_or_else(|| McpError::Internal("connector vanished after oauth config".into()))
}

/// The secret the user registered alongside `client_id`, still encrypted.
/// Persisting discovery must keep it: a confidential client (Entra ID
/// requires one) cannot redeem its authorization code without it.
fn stored_secret_for(connector: &McpConnector, client_id: &str) -> Option<String> {
    if connector.oauth_client_id.as_deref() == Some(client_id) {
        connector.oauth_client_secret.clone()
    } else {
        None
    }
}

/// An RFC 7591 client, when the server offers registration and accepts ours;
/// otherwise an empty client, which `begin_authorization` reports as needing
/// a user-supplied client id.
async fn dynamically_registered_client(
    state: &McpState,
    connector: &McpConnector,
    discovery: &DiscoveredAuthorization,
    redirect_uri: &str,
) -> Result<StoredClient> {
    let Some(endpoint) = discovery.registration_endpoint.as_deref() else {
        return Ok(StoredClient::default());
    };
    let Some(registered) =
        register_dynamic_client(&state.guarded_http_client, endpoint, redirect_uri).await?
    else {
        return Ok(StoredClient::default());
    };
    // Encrypt the DCR client secret at rest with the owner's key.
    let encrypted_secret = match (registered.client_secret, connector.owner_id) {
        (Some(secret), Some(owner)) => Some(SecretsCrypto::for_user(owner).encrypt(&secret)),
        _ => None,
    };
    Ok(StoredClient {
        client_id: Some(registered.client_id),
        encrypted_secret,
    })
}

/// Outcome of the public OAuth callback for the server to render.
pub enum CallbackOutcome {
    Redirect(String),
    /// Render an HTML page that closes the popup window, then falls back to a
    /// redirect if `window.close()` is blocked (e.g. the flow ran in a
    /// top-level tab instead of a popup).
    ClosePopup(String),
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
        // The credential now works — resolve any hitl_requests rows that were
        // waiting on exactly this fix so MCP's dispatcher (already running)
        // nudges every paused conversation to retry. A plain function call,
        // the same one the resolve API uses; no new store/dispatcher logic.
        match nasiko_hitl::repo::resolve_pending_auth_required_for_connector(
            &state.db,
            oauth_state.user_id,
            connector.id,
        )
        .await
        {
            Ok(resolved) if !resolved.is_empty() => {
                tracing::info!(
                    connector = %connector.name, user_id = %oauth_state.user_id,
                    resolved_count = resolved.len(),
                    "auto-resolved pending auth_required hitl requests after oauth callback"
                );
            }
            Ok(_) => {}
            Err(e) => {
                tracing::warn!(
                    error = %e, connector = %connector.name, user_id = %oauth_state.user_id,
                    "failed to auto-resolve pending auth_required hitl requests"
                );
            }
        }
    }
    crate::session::invalidate_session_cache(state, oauth_state.user_id).await;
    tracing::info!(
        connector = %connector.name, user_id = %oauth_state.user_id, verified = outcome.verified,
        "stored mcp oauth token"
    );
    // Sync tools eagerly so the frontend shows them immediately instead of
    // waiting for a lazy sync on the next `tools/list` request.
    if outcome.verified
        && let Err(e) =
            crate::permissions::sync_connector_tools_by_id(state, oauth_state.user_id, connector.id)
                .await
    {
        tracing::warn!(error = %e, "post-oauth tool sync failed (non-fatal)");
    }

    let dest = oauth_state.redirect_url.unwrap_or_else(|| "/".to_string());
    let safe_dest = crate::net::safe_redirect(&dest, state.config.gateway_public_url.as_deref());
    CallbackOutcome::ClosePopup(safe_dest)
}

/// What the probe learns about an authorization server's client onboarding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClientOnboarding {
    /// RFC 7591: the server advertises a `registration_endpoint`.
    pub supports_dcr: bool,
    /// Whether `token_endpoint_auth_methods_supported` contains `none`
    /// (public clients: PKCE, no secret); `None` when the document is silent.
    pub public_client: Option<bool>,
}

impl ClientOnboarding {
    pub fn from_metadata(metadata: &AuthorizationServerMetadata) -> Self {
        let document = &metadata.document;
        let public_client = document
            .get("token_endpoint_auth_methods_supported")
            .and_then(Value::as_array)
            .map(|methods| methods.iter().any(|m| m.as_str() == Some("none")));
        Self {
            supports_dcr: first_str(document, &["registration_endpoint"])
                .is_some_and(|endpoint| !endpoint.is_empty()),
            public_client,
        }
    }
}

/// Client onboarding of the authorization server an OAuth-protected
/// resource names, for the probe endpoint so the frontend can decide whether
/// to show client-credential fields. `None` when it cannot be discovered.
pub async fn as_client_onboarding(
    http: &reqwest::Client,
    resource_metadata: &Value,
) -> Option<ClientOnboarding> {
    let issuer = first_authorization_server(resource_metadata)?;
    let metadata = fetch_authorization_server_metadata(http, issuer).await?;
    Some(ClientOnboarding::from_metadata(&metadata))
}

/// Client onboarding of the MCP server's own origin, for servers that name no
/// resource metadata (see [`discover_from_origin`]).
pub async fn origin_client_onboarding(
    http: &reqwest::Client,
    mcp_url: &str,
) -> Option<ClientOnboarding> {
    let metadata = fetch_origin_authorization_server_metadata(http, mcp_url).await?;
    Some(ClientOnboarding::from_metadata(&metadata))
}

/// RFC 9728 direct discovery: `GET /.well-known/oauth-protected-resource` at
/// the resource's origin - tried path-aware first (e.g.
/// `/.well-known/oauth-protected-resource/mcp` for a resource at `/mcp`, per
/// RFC 9728 §3.1), then root-only as a fallback for servers that publish it
/// there regardless of path. This is the MCP spec's own PRIMARY discovery
/// method - unlike inferring from a `WWW-Authenticate: resource_metadata=...`
/// header on a bare 401, which some real servers omit even though they
/// support OAuth. Confirmed live: Atlassian's hosted MCP server does neither -
/// `/.well-known/oauth-protected-resource` 404s there entirely, a real,
/// currently-open gap on their side (atlassian/atlassian-mcp-server#148), not
/// something this can work around.
///
/// Returns the parsed resource metadata document, or `None` if no candidate
/// URL returned a document with a non-empty `authorization_servers` array.
pub async fn fetch_protected_resource_metadata(
    http: &reqwest::Client,
    mcp_url: &str,
) -> Option<Value> {
    let (origin, path) = origin_and_path(mcp_url)?;
    let mut candidates = Vec::new();
    if !path.is_empty() {
        candidates.push(format!(
            "{origin}/.well-known/oauth-protected-resource/{path}"
        ));
    }
    candidates.push(format!("{origin}/.well-known/oauth-protected-resource"));

    for candidate in candidates {
        if let Some(document) = fetch_resource_metadata_document(http, &candidate).await {
            return Some(document);
        }
    }
    None
}

const OAUTH_METADATA_PATH: &str = ".well-known/oauth-authorization-server";
const OIDC_METADATA_PATH: &str = ".well-known/openid-configuration";
const OFFLINE_ACCESS_SCOPE: &str = "offline_access";

/// POST a bare `initialize`. An OAuth-protected MCP server answers 401, and
/// its `WWW-Authenticate` value (returned here) may name the resource
/// metadata URL and the scope to request.
async fn fetch_auth_challenge(http: &reqwest::Client, mcp_url: &str) -> Result<String> {
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
        .await
        .map_err(|e| McpError::Backend(format!("could not reach MCP server: {e}")))?;

    if probe.status() != reqwest::StatusCode::UNAUTHORIZED {
        return Err(McpError::Oauth(format!(
            "expected 401 from OAuth server, got {}. Try auth_type='none' or 'bearer'.",
            probe.status().as_u16()
        )));
    }
    Ok(probe
        .headers()
        .get(reqwest::header::WWW_AUTHENTICATE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string())
}

/// The resource metadata document: from the URL the 401 challenge names,
/// else RFC 9728's well-known URLs - some real OAuth-capable servers
/// (confirmed: Atlassian) never include the `resource_metadata` pointer.
async fn fetch_resource_metadata_for(
    http: &reqwest::Client,
    mcp_url: &str,
    challenge: &str,
) -> Result<Value> {
    if let Some(url) = auth_param(challenge, "resource_metadata")
        && let Some(document) = fetch_resource_metadata_document(http, &url).await
    {
        return Ok(document);
    }
    fetch_protected_resource_metadata(http, mcp_url)
        .await
        .ok_or_else(|| {
            McpError::Oauth(
                "401 has no usable resource_metadata URL in WWW-Authenticate, and no RFC 9728 \
                 well-known endpoint was found either - this server may not actually \
                 support OAuth 2.1, or doesn't publish discovery metadata"
                    .to_string(),
            )
        })
}

/// The first authorization server a resource metadata document names.
fn first_authorization_server(resource_metadata: &Value) -> Option<&str> {
    resource_metadata
        .get("authorization_servers")?
        .as_array()?
        .first()?
        .as_str()
}

/// The first metadata document found for `issuer`, trying
/// [`authorization_server_metadata_urls`] in order.
async fn fetch_authorization_server_metadata(
    http: &reqwest::Client,
    issuer: &str,
) -> Option<AuthorizationServerMetadata> {
    for (url, source) in authorization_server_metadata_urls(issuer) {
        if let Some(metadata) = fetch_json_document(http, &url)
            .await
            .and_then(|document| parse_authorization_server_metadata(document, source))
        {
            return Some(metadata);
        }
    }
    None
}

/// Where an issuer's metadata may live. Every RFC 8414 URL comes before any
/// OpenID Connect one: RFC 8414's path insertion, then the RFC 8414 document
/// appended to the issuer (non-standard, but the only URL this gateway used to
/// try), then OpenID Connect Discovery's insertion and appending forms. A
/// server that publishes both documents (Keycloak, Okta) is therefore still
/// discovered as RFC 8414 and keeps receiving `resource`; only an issuer with
/// no RFC 8414 document at all (Entra ID) is treated as OpenID Connect.
fn authorization_server_metadata_urls(issuer: &str) -> Vec<(String, MetadataSource)> {
    let Some((origin, path)) = origin_and_path(issuer) else {
        return Vec::new();
    };
    if path.is_empty() {
        return vec![
            (
                format!("{origin}/{OAUTH_METADATA_PATH}"),
                MetadataSource::OAuth,
            ),
            (
                format!("{origin}/{OIDC_METADATA_PATH}"),
                MetadataSource::OpenIdConnect,
            ),
        ];
    }
    vec![
        (
            format!("{origin}/{OAUTH_METADATA_PATH}/{path}"),
            MetadataSource::OAuth,
        ),
        (
            format!("{origin}/{path}/{OAUTH_METADATA_PATH}"),
            MetadataSource::OAuth,
        ),
        (
            format!("{origin}/{OIDC_METADATA_PATH}/{path}"),
            MetadataSource::OpenIdConnect,
        ),
        (
            format!("{origin}/{path}/{OIDC_METADATA_PATH}"),
            MetadataSource::OpenIdConnect,
        ),
    ]
}

fn parse_authorization_server_metadata(
    document: Value,
    source: MetadataSource,
) -> Option<AuthorizationServerMetadata> {
    let authorization_endpoint = first_str(&document, &["authorization_endpoint"])?;
    let token_endpoint = first_str(&document, &["token_endpoint"])?;
    Some(AuthorizationServerMetadata {
        authorization_endpoint,
        token_endpoint,
        document,
        source,
    })
}

async fn fetch_resource_metadata_document(http: &reqwest::Client, url: &str) -> Option<Value> {
    fetch_json_document(http, url)
        .await
        .filter(|document| first_authorization_server(document).is_some())
}

/// A JSON document from a successful `GET`; `None` for any failure, since
/// every caller moves on to its next candidate URL.
async fn fetch_json_document(http: &reqwest::Client, url: &str) -> Option<Value> {
    let response = http.get(url).timeout(DISCOVERY_TIMEOUT).send().await.ok()?;
    if !response.status().is_success() {
        return None;
    }
    response.json().await.ok()
}

/// `(origin, path)` of a URL, the path without surrounding slashes.
fn origin_and_path(url: &str) -> Option<(String, String)> {
    let parsed = reqwest::Url::parse(url).ok()?;
    let origin = format!(
        "{}://{}{}",
        parsed.scheme(),
        parsed.host_str()?,
        parsed.port().map(|p| format!(":{p}")).unwrap_or_default()
    );
    Some((origin, parsed.path().trim_matches('/').to_string()))
}

/// The MCP spec's scope selection: the scope the 401 challenge asks for,
/// else every scope the resource advertises, else none. `offline_access` is
/// added when the authorization server offers it, because some (Entra ID)
/// only issue a refresh token when it is requested - and holding a
/// refreshable grant is the point of this flow.
fn requested_scope(
    challenge: &str,
    resource_metadata: &Value,
    server_metadata: &Value,
) -> Option<String> {
    let scope = challenge_scope(challenge).or_else(|| {
        let advertised = scopes_supported(resource_metadata);
        (!advertised.is_empty()).then(|| advertised.join(" "))
    })?;
    Some(with_offline_access(scope, server_metadata))
}

/// The scope the 401 challenge asks for, when it names one.
fn challenge_scope(challenge: &str) -> Option<String> {
    auth_param(challenge, "scope").filter(|scope| !scope.trim().is_empty())
}

/// `scope` plus `offline_access`, when the authorization server offers it and
/// the scope does not already ask for it.
fn with_offline_access(scope: String, server_metadata: &Value) -> String {
    let offline_access_offered = scopes_supported(server_metadata).contains(&OFFLINE_ACCESS_SCOPE);
    let offline_access_requested = scope.split_whitespace().any(|s| s == OFFLINE_ACCESS_SCOPE);
    if offline_access_offered && !offline_access_requested {
        format!("{scope} {OFFLINE_ACCESS_SCOPE}")
    } else {
        scope
    }
}

fn scopes_supported(metadata: &Value) -> Vec<&str> {
    metadata
        .get("scopes_supported")
        .and_then(Value::as_array)
        .map(|scopes| scopes.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default()
}

/// RFC 8707 `resource`: the connector URL, except for an authorization
/// server known only through OpenID Connect discovery. Entra ID - the case
/// that path exists for - rejects a `resource` that differs from the
/// requested scope's own resource (AADSTS9010010, confirmed live against
/// Azure DevOps' MCP server), and there the scope already fixes the token's
/// audience.
fn requested_resource(mcp_url: &str, source: MetadataSource) -> Option<String> {
    match source {
        MetadataSource::OAuth => Some(mcp_url.to_string()),
        MetadataSource::OpenIdConnect => None,
    }
}

/// One auth-param (`name="value"` or `name=value`) of a `WWW-Authenticate`
/// value. Matched only at a parameter boundary, so `scope` is never read out
/// of a longer name such as `xscope`.
fn auth_param(www_authenticate: &str, name: &str) -> Option<String> {
    let marker = format!("{name}=");
    let mut search_from = 0;
    while let Some(offset) = www_authenticate[search_from..].find(&marker) {
        let start = search_from + offset;
        let value_start = start + marker.len();
        let at_boundary = www_authenticate[..start]
            .chars()
            .next_back()
            .is_none_or(|c| c == ' ' || c == ',');
        if at_boundary {
            let rest = &www_authenticate[value_start..];
            let value = match rest.strip_prefix('"') {
                Some(quoted) => &quoted[..quoted.find('"').unwrap_or(quoted.len())],
                None => &rest[..rest.find([',', ' ']).unwrap_or(rest.len())],
            };
            return Some(value.to_string());
        }
        search_from = value_start;
    }
    None
}

/// RFC 7591 dynamic client registration. `None` when the server refuses, so
/// the caller can ask for a user-supplied client id instead.
async fn register_dynamic_client(
    http: &reqwest::Client,
    registration_endpoint: &str,
    redirect_uri: &str,
) -> Result<Option<RegisteredClient>> {
    let response = http
        .post(registration_endpoint)
        .timeout(DISCOVERY_TIMEOUT)
        .json(&json!({
            "client_name": "MCP Gateway",
            "redirect_uris": [redirect_uri],
            "grant_types": ["authorization_code", "refresh_token"],
            "response_types": ["code"],
            "token_endpoint_auth_method": "none",
        }))
        .send()
        .await
        .map_err(|e| McpError::Oauth(format!("dynamic client registration failed: {e}")))?;
    if !response.status().is_success() {
        tracing::warn!(
            status = response.status().as_u16(),
            "dynamic client registration failed"
        );
        return Ok(None);
    }
    let registration: Value = response.json().await.map_err(|e| {
        McpError::Oauth(format!(
            "dynamic client registration returned bad JSON: {e}"
        ))
    })?;
    Ok(
        first_str(&registration, &["client_id"]).map(|client_id| RegisteredClient {
            client_id,
            client_secret: first_str(&registration, &["client_secret"]),
        }),
    )
}

#[cfg(test)]
mod tests {
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD as B64Std;
    use std::sync::Arc;

    use super::*;
    use crate::config::{McpConfig, ToolSearchMode};
    use crate::provider::{GenericMcpProvider, Providers};

    /// Install a valid `SECRETS_ENCRYPTION_KEY` so `SecretsCrypto::for_user`
    /// doesn't panic — mirrors `oss/secrets/tests/crypto.rs`'s `install_valid_key`.
    fn install_valid_encryption_key() {
        let key = B64Std.encode([0x24u8; 32]);
        // SAFETY: all callers in this module install the same value, so a
        // concurrent set from another test is not observably different.
        unsafe { std::env::set_var("SECRETS_ENCRYPTION_KEY", &key) };
    }

    fn test_state() -> McpState {
        let db = sqlx::PgPool::connect_lazy("postgres://user:pass@127.0.0.1:1/db")
            .expect("lazy pool construction must not touch the network");
        let redis = redis::Client::open("redis://127.0.0.1:1/").expect("lazy redis client");
        McpState {
            db,
            redis,
            http_client: reqwest::Client::new(),
            guarded_http_client: reqwest::Client::new(),
            config: McpConfig {
                composio_api_key: None,
                composio_base_url: "http://localhost".to_string(),
                composio_webhook_secret: None,
                gateway_public_url: None,
                oauth_redirect_base_url: None,
                composio_callback_base_url: None,
                session_ttl_seconds: 60,
                perm_cache_ttl_seconds: 60,
                manifest_ttl_seconds: 60,
                toolcount_ttl_seconds: 3600,
                oauth_state_signing_key: "test".to_string(),
                description_model: "gpt-4o-mini".to_string(),
                hitl_request_ttl_days: 7,
                tool_search_mode: ToolSearchMode::Semantic,
                tool_search_tool_limit: 0,
                tool_search_meta_limit: 0,
                openai_api_key: None,
                embedding_model: "".to_string(),
                gateway_instructions: String::new(),
                identity_signing_key: b"test-identity-signing-key".to_vec(),
            },
            providers: Providers {
                composio: None,
                mcp: GenericMcpProvider::new(reqwest::Client::new(), reqwest::Client::new()),
            },
            authorizer: std::sync::Arc::new(crate::authorizer::OssConnectorAuthorizer),
            endpoint_refresher: std::sync::Arc::new(crate::endpoint_refresh::NoopEndpointRefresher),
            llm: nasiko_orchestrator::providers::LLMProvider::from_env(reqwest::Client::new()),
            search_index: Arc::new(crate::search::NoopSearchIndex),
        }
    }

    fn oauth_connector(token_endpoint: &str) -> McpConnector {
        McpConnector {
            id: Uuid::new_v4(),
            provider_type: "mcp_server".into(),
            owner_id: None,
            name: "test-oauth-connector".into(),
            display_name: None,
            logo_url: None,
            description: None,
            instructions: None,
            auth_config_id: None,
            auth_scheme: None,
            use_composio_managed: None,
            url: Some("https://example.com/mcp".into()),
            transport: Some("streamable_http".into()),
            auth_type: Some("oauth2".into()),
            url_param_name: None,
            credential_header_name: None,
            headers: None,
            is_active: Some(true),
            oauth_authorization_endpoint: None,
            oauth_token_endpoint: Some(token_endpoint.to_string()),
            oauth_client_id: Some("client123".into()),
            oauth_client_secret: None,
            oauth_scopes: None,
            oauth_send_resource: None,
            source_kind: crate::repo::SourceKind::ExternalUrl,
            auto_enable: false,
            build_status: None,
            container_image_tag: None,
            setup_status: None,
            setup_error: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    fn oauth_connection(
        user_id: Uuid,
        connector_id: Uuid,
        crypto: &SecretsCrypto,
    ) -> McpUserConnection {
        McpUserConnection {
            id: Uuid::new_v4(),
            user_id,
            connector_id,
            status: "active".into(),
            connected_account_id: None,
            redirect_url: None,
            oauth_url: None,
            encrypted_credential: Some(crypto.encrypt("old-access-token")),
            encrypted_refresh_token: Some(crypto.encrypt("refresh-token-value")),
            token_expires_at: None,
            scope: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    #[tokio::test]
    async fn refresh_flags_auth_required_when_token_endpoint_rejects_request() {
        install_valid_encryption_key();
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/token")
            .with_status(401)
            .create_async()
            .await;

        let user_id = Uuid::new_v4();
        let crypto = SecretsCrypto::for_user(user_id);
        let connector = oauth_connector(&format!("{}/token", server.url()));
        let conn = oauth_connection(user_id, connector.id, &crypto);
        let state = test_state();

        let result = refresh(&state, &crypto, user_id, &connector, &conn)
            .await
            .unwrap();

        mock.assert_async().await;
        assert_eq!(result, Err(ConnectorUnusable::AuthRequired));
    }

    #[tokio::test]
    async fn refresh_flags_auth_required_when_no_refresh_token_stored() {
        install_valid_encryption_key();
        let user_id = Uuid::new_v4();
        let crypto = SecretsCrypto::for_user(user_id);
        let connector = oauth_connector("https://unused.example.com/token");
        let mut conn = oauth_connection(user_id, connector.id, &crypto);
        conn.encrypted_refresh_token = None;
        let state = test_state();

        // No network call should happen — missing refresh token is caught before
        // any request is sent.
        let result = refresh(&state, &crypto, user_id, &connector, &conn)
            .await
            .unwrap();

        assert_eq!(result, Err(ConnectorUnusable::AuthRequired));
    }

    #[tokio::test]
    async fn access_token_for_flags_missing_credential_without_stored_access_token() {
        install_valid_encryption_key();
        let user_id = Uuid::new_v4();
        let crypto = SecretsCrypto::for_user(user_id);
        let connector = oauth_connector("https://unused.example.com/token");
        let mut conn = oauth_connection(user_id, connector.id, &crypto);
        conn.encrypted_credential = None;
        let state = test_state();

        let result = access_token_for(&state, &crypto, user_id, &connector, &conn)
            .await
            .unwrap();

        assert_eq!(result, Err(ConnectorUnusable::MissingCredential));
    }

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
            auth_param(h, "resource_metadata").as_deref(),
            Some("https://as.example.com/.well-known/x")
        );
    }

    #[test]
    fn auth_param_matches_only_at_a_parameter_boundary() {
        let h = r#"Bearer xscope="wrong", scope="files:read files:write", realm="r""#;
        assert_eq!(
            auth_param(h, "scope").as_deref(),
            Some("files:read files:write")
        );
        assert_eq!(auth_param(r#"Bearer xscope="wrong""#, "scope"), None);
        assert_eq!(
            auth_param("Bearer error=invalid_token, scope=read", "scope").as_deref(),
            Some("read")
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

    // ─── discover_authorization (RFC 9728 → RFC 8414 / OpenID Connect) ─────────

    fn json_body(body: Value) -> String {
        body.to_string()
    }

    /// `POST /mcp` answers 401 with a challenge pointing at resource metadata
    /// that names `issuer` and advertises `scopes`.
    async fn mount_protected_mcp(
        server: &mut mockito::ServerGuard,
        challenge_suffix: &str,
        issuer: &str,
        scopes: &[&str],
    ) {
        let metadata_url = format!("{}/.well-known/oauth-protected-resource/mcp", server.url());
        server
            .mock("POST", "/mcp")
            .with_status(401)
            .with_header(
                "www-authenticate",
                &format!(r#"Bearer resource_metadata="{metadata_url}"{challenge_suffix}"#),
            )
            .create_async()
            .await;
        server
            .mock("GET", "/.well-known/oauth-protected-resource/mcp")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(json_body(json!({
                "resource": format!("{}/mcp", server.url()),
                "authorization_servers": [issuer],
                "scopes_supported": scopes,
            })))
            .create_async()
            .await;
    }

    async fn mount_metadata(server: &mut mockito::ServerGuard, path: &str, document: Value) {
        server
            .mock("GET", path)
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(json_body(document))
            .create_async()
            .await;
    }

    #[tokio::test]
    async fn discovers_an_openid_connect_only_issuer_and_requests_scope_without_resource() {
        // Entra ID's shape: an issuer with a path, no RFC 8414 document at
        // all (unmounted paths answer 501 here, 404 there), OpenID Connect
        // discovery appended to the issuer.
        let mut server = mockito::Server::new_async().await;
        let issuer = format!("{}/tenant/v2.0", server.url());
        mount_protected_mcp(&mut server, "", &issuer, &["api://app/.default"]).await;
        mount_metadata(
            &mut server,
            "/tenant/v2.0/.well-known/openid-configuration",
            json!({
                "authorization_endpoint": "https://idp.example/authorize",
                "token_endpoint": "https://idp.example/token",
                "scopes_supported": ["openid", "offline_access"],
            }),
        )
        .await;

        let discovered =
            discover_authorization(&reqwest::Client::new(), &format!("{}/mcp", server.url()))
                .await
                .expect("OpenID Connect discovery succeeds");

        assert_eq!(
            discovered.authorization_endpoint,
            "https://idp.example/authorize"
        );
        assert_eq!(discovered.token_endpoint, "https://idp.example/token");
        assert_eq!(discovered.registration_endpoint, None);
        assert_eq!(
            discovered.requested_access,
            RequestedAccess {
                scope: Some("api://app/.default offline_access".to_string()),
                resource: None,
            }
        );
    }

    #[tokio::test]
    async fn prefers_rfc_8414_metadata_and_the_scope_the_challenge_asks_for() {
        let mut server = mockito::Server::new_async().await;
        let issuer = format!("{}/tenant", server.url());
        mount_protected_mcp(
            &mut server,
            r#", scope="files:read""#,
            &issuer,
            &["files:read", "files:write"],
        )
        .await;
        mount_metadata(
            &mut server,
            "/.well-known/oauth-authorization-server/tenant",
            json!({
                "authorization_endpoint": "https://as.example/authorize",
                "token_endpoint": "https://as.example/token",
                "registration_endpoint": "https://as.example/register",
            }),
        )
        .await;
        let oidc = server
            .mock("GET", "/.well-known/openid-configuration/tenant")
            .expect(0)
            .create_async()
            .await;
        let mcp_url = format!("{}/mcp", server.url());

        let discovered = discover_authorization(&reqwest::Client::new(), &mcp_url)
            .await
            .expect("RFC 8414 discovery succeeds");

        oidc.assert_async().await;
        assert_eq!(
            discovered.authorization_endpoint,
            "https://as.example/authorize"
        );
        assert_eq!(
            discovered.registration_endpoint.as_deref(),
            Some("https://as.example/register")
        );
        assert_eq!(
            discovered.requested_access,
            RequestedAccess {
                scope: Some("files:read".to_string()),
                resource: Some(mcp_url),
            }
        );
    }

    #[tokio::test]
    async fn prefers_appended_rfc_8414_metadata_over_openid_connect_and_keeps_resource() {
        let mut server = mockito::Server::new_async().await;
        let issuer = format!("{}/tenant", server.url());
        mount_protected_mcp(&mut server, "", &issuer, &[]).await;
        mount_metadata(
            &mut server,
            "/tenant/.well-known/oauth-authorization-server",
            json!({
                "authorization_endpoint": "https://as.example/authorize",
                "token_endpoint": "https://as.example/token",
            }),
        )
        .await;
        // Keycloak and Okta publish both appended documents; the OpenID
        // Connect one must not win, or `resource` would silently be dropped.
        mount_metadata(
            &mut server,
            "/tenant/.well-known/openid-configuration",
            json!({
                "authorization_endpoint": "https://oidc.example/authorize",
                "token_endpoint": "https://oidc.example/token",
            }),
        )
        .await;
        let mcp_url = format!("{}/mcp", server.url());

        let discovered = discover_authorization(&reqwest::Client::new(), &mcp_url)
            .await
            .expect("the original appended URL is still tried");

        assert_eq!(discovered.token_endpoint, "https://as.example/token");
        assert_eq!(
            discovered.requested_access,
            RequestedAccess::without_scope(&mcp_url)
        );
    }

    #[tokio::test]
    async fn missing_authorization_server_metadata_is_an_oauth_error_not_a_500() {
        let mut server = mockito::Server::new_async().await;
        let issuer = format!("{}/tenant/v2.0", server.url());
        mount_protected_mcp(&mut server, "", &issuer, &["api://app/.default"]).await;

        let err = discover_authorization(&reqwest::Client::new(), &format!("{}/mcp", server.url()))
            .await
            .expect_err("no metadata document anywhere");

        assert!(matches!(err, McpError::Oauth(_)), "got {err:?}");
        assert_eq!(err.http_status(), 502);
    }

    /// `POST /mcp` answers 401 pointing at a resource metadata URL that itself
    /// 401s, with no RFC 9728 document anywhere - Entra ID behind APIM.
    async fn mount_unusable_resource_metadata(
        server: &mut mockito::ServerGuard,
        challenge_suffix: &str,
    ) {
        let pointer = format!(
            "{}/p/.well-known/oauth-protected-resource/p/mcp",
            server.url()
        );
        server
            .mock("POST", "/mcp")
            .with_status(401)
            .with_header(
                "www-authenticate",
                &format!(r#"Bearer error="invalid_token", resource_metadata="{pointer}"{challenge_suffix}"#),
            )
            .create_async()
            .await;
        server
            .mock("GET", "/p/.well-known/oauth-protected-resource/p/mcp")
            .with_status(401)
            .create_async()
            .await;
        for path in [
            "/.well-known/oauth-protected-resource/mcp",
            "/.well-known/oauth-protected-resource",
        ] {
            server
                .mock("GET", path)
                .with_status(404)
                .create_async()
                .await;
        }
    }

    fn apim_origin_metadata() -> Value {
        json!({
            "issuer": "https://login.example/tenant/v2.0",
            "authorization_endpoint": "https://host.example/authorize",
            "token_endpoint": "https://login.example/tenant/oauth2/v2.0/token",
            "token_endpoint_auth_methods_supported": ["none", "client_secret_post"],
            "scopes_supported": ["api://x/mcp.access", "openid", "offline_access"],
        })
    }

    #[tokio::test]
    async fn falls_back_to_the_origin_as_authorization_server_without_resource() {
        let mut server = mockito::Server::new_async().await;
        mount_unusable_resource_metadata(&mut server, "").await;
        mount_metadata(
            &mut server,
            "/.well-known/oauth-authorization-server",
            apim_origin_metadata(),
        )
        .await;

        let discovered =
            discover_authorization(&reqwest::Client::new(), &format!("{}/mcp", server.url()))
                .await
                .expect("origin metadata is used");

        assert_eq!(
            discovered.authorization_endpoint,
            "https://host.example/authorize"
        );
        assert_eq!(
            discovered.token_endpoint,
            "https://login.example/tenant/oauth2/v2.0/token"
        );
        assert_eq!(discovered.registration_endpoint, None);
        assert_eq!(
            discovered.requested_access,
            RequestedAccess {
                scope: Some("api://x/mcp.access openid offline_access".to_string()),
                resource: None,
            }
        );
    }

    #[tokio::test]
    async fn origin_fallback_prefers_the_challenge_scope_and_adds_offline_access() {
        let mut server = mockito::Server::new_async().await;
        mount_unusable_resource_metadata(&mut server, r#", scope="a b""#).await;
        mount_metadata(
            &mut server,
            "/.well-known/oauth-authorization-server",
            apim_origin_metadata(),
        )
        .await;

        let discovered =
            discover_authorization(&reqwest::Client::new(), &format!("{}/mcp", server.url()))
                .await
                .expect("origin metadata is used");

        assert_eq!(
            discovered.requested_access.scope.as_deref(),
            Some("a b offline_access")
        );
    }

    /// `POST /mcp` answers a plain Bearer 401 with no `resource_metadata` and
    /// no RFC 9728 document exists - Atlassian's shape.
    async fn mount_plain_bearer_challenge(server: &mut mockito::ServerGuard, suffix: &str) {
        server
            .mock("POST", "/mcp")
            .with_status(401)
            .with_header(
                "www-authenticate",
                &format!(r#"Bearer realm="OAuth", error="invalid_token"{suffix}"#),
            )
            .create_async()
            .await;
        for path in [
            "/.well-known/oauth-protected-resource/mcp",
            "/.well-known/oauth-protected-resource",
        ] {
            server
                .mock("GET", path)
                .with_status(404)
                .create_async()
                .await;
        }
    }

    fn self_hosted_origin_metadata(issuer: &str) -> Value {
        json!({
            "issuer": issuer,
            "authorization_endpoint": format!("{issuer}/v1/authorize"),
            "token_endpoint": format!("{issuer}/v1/token"),
            "registration_endpoint": format!("{issuer}/v1/register"),
            "token_endpoint_auth_methods_supported": ["none", "client_secret_post"],
            "scopes_supported": ["read", "write", "offline_access"],
        })
    }

    #[tokio::test]
    async fn self_hosted_origin_as_requests_no_scope_and_keeps_registration() {
        let mut server = mockito::Server::new_async().await;
        mount_plain_bearer_challenge(&mut server, "").await;
        let issuer = server.url();
        mount_metadata(
            &mut server,
            "/.well-known/oauth-authorization-server",
            self_hosted_origin_metadata(&issuer),
        )
        .await;

        let discovered = discover_authorization(&reqwest::Client::new(), &format!("{issuer}/mcp"))
            .await
            .expect("origin metadata is used");

        assert_eq!(
            discovered.registration_endpoint.as_deref(),
            Some(format!("{issuer}/v1/register").as_str())
        );
        assert_eq!(
            discovered.requested_access,
            RequestedAccess {
                scope: None,
                resource: None,
            }
        );
    }

    #[tokio::test]
    async fn self_hosted_origin_as_uses_only_the_challenge_scope() {
        let mut server = mockito::Server::new_async().await;
        mount_plain_bearer_challenge(&mut server, r#", scope="a b""#).await;
        let issuer = server.url();
        mount_metadata(
            &mut server,
            "/.well-known/oauth-authorization-server",
            self_hosted_origin_metadata(&issuer),
        )
        .await;

        let discovered = discover_authorization(&reqwest::Client::new(), &format!("{issuer}/mcp"))
            .await
            .expect("origin metadata is used");

        assert_eq!(
            discovered.requested_access,
            RequestedAccess {
                scope: Some("a b offline_access".to_string()),
                resource: None,
            }
        );
    }

    #[tokio::test]
    async fn unusable_pointer_and_no_origin_metadata_keeps_the_original_error() {
        let mut server = mockito::Server::new_async().await;
        mount_unusable_resource_metadata(&mut server, "").await;

        let err = discover_authorization(&reqwest::Client::new(), &format!("{}/mcp", server.url()))
            .await
            .expect_err("nothing to discover");

        assert!(
            matches!(&err, McpError::Oauth(m) if m.contains("no usable resource_metadata URL")),
            "got {err:?}"
        );
    }

    #[tokio::test]
    async fn resource_metadata_path_still_sends_resource_for_rfc_8414() {
        let mut server = mockito::Server::new_async().await;
        let issuer = server.url();
        mount_protected_mcp(&mut server, "", &issuer, &["a"]).await;
        mount_metadata(
            &mut server,
            "/.well-known/oauth-authorization-server",
            json!({
                "authorization_endpoint": "https://idp.example/authorize",
                "token_endpoint": "https://idp.example/token",
            }),
        )
        .await;
        let mcp_url = format!("{}/mcp", server.url());

        let discovered = discover_authorization(&reqwest::Client::new(), &mcp_url)
            .await
            .expect("discovery succeeds");

        assert_eq!(discovered.requested_access.resource, Some(mcp_url));
    }

    #[test]
    fn connector_overrides_beat_discovery() {
        let discovered = RequestedAccess {
            scope: Some("auto offline_access".into()),
            resource: Some("https://example.com/mcp".into()),
        };
        let url = "https://example.com/mcp";
        let with = |scopes: Option<&str>, send: Option<bool>| {
            let mut connector = oauth_connector("https://idp.example/token");
            connector.oauth_scopes = scopes.map(str::to_string);
            connector.oauth_send_resource = send;
            apply_connector_overrides(discovered.clone(), &connector, url)
        };

        assert_eq!(with(None, None), discovered);
        assert_eq!(
            with(Some("api://x/.default"), None).scope.as_deref(),
            Some("api://x/.default offline_access"),
            "discovery wanted offline_access, so the explicit scope keeps it"
        );
        assert_eq!(
            with(Some("api://x/.default offline_access"), None)
                .scope
                .as_deref(),
            Some("api://x/.default offline_access"),
            "offline_access is not duplicated"
        );
        let mut without_offline_access = discovered.clone();
        without_offline_access.scope = Some("auto".into());
        let mut connector = oauth_connector("https://idp.example/token");
        connector.oauth_scopes = Some("api://x/.default".into());
        assert_eq!(
            apply_connector_overrides(without_offline_access, &connector, url)
                .scope
                .as_deref(),
            Some("api://x/.default"),
            "not added when discovery's scope lacked it"
        );
        assert_eq!(with(Some("  "), None).scope, discovered.scope);
        assert_eq!(with(None, Some(false)).resource, None);
        let mut no_resource = discovered.clone();
        no_resource.resource = None;
        let mut connector = oauth_connector("https://idp.example/token");
        connector.oauth_send_resource = Some(true);
        assert_eq!(
            apply_connector_overrides(no_resource, &connector, url)
                .resource
                .as_deref(),
            Some(url)
        );
    }

    // The empty-string-clears-`oauth_scopes` behaviour lives in the SQL
    // (`NULLIF(BTRIM(..), '')` in `repo::update_connector`), so it needs a
    // database and is left to the server integration tests.

    #[test]
    fn requested_scope_adds_offline_access_only_when_offered_and_not_yet_asked_for() {
        let offered = json!({"scopes_supported": ["openid", "offline_access"]});
        let advertised = json!({"scopes_supported": ["a", "b"]});

        assert_eq!(
            requested_scope("", &advertised, &offered).as_deref(),
            Some("a b offline_access")
        );
        assert_eq!(
            requested_scope(r#"Bearer scope="a offline_access""#, &advertised, &offered).as_deref(),
            Some("a offline_access")
        );
        assert_eq!(
            requested_scope("", &advertised, &json!({})).as_deref(),
            Some("a b")
        );
        assert_eq!(
            requested_scope("", &json!({}), &offered),
            None,
            "offline_access is never requested on its own"
        );
    }

    #[test]
    fn authorize_url_carries_requested_access_and_keeps_the_endpoints_own_query() {
        let query_of = |access: &RequestedAccess| -> std::collections::HashMap<String, String> {
            let url = build_authorize_url(
                "https://idp.example/authorize?audience=api",
                "client",
                "https://cp.example/api/mcp/oauth/callback",
                "state",
                "challenge",
                access,
            )
            .expect("valid endpoint");
            reqwest::Url::parse(&url)
                .expect("built URL parses")
                .query_pairs()
                .into_owned()
                .collect()
        };

        let entra = query_of(&RequestedAccess {
            scope: Some("api://app/.default offline_access".to_string()),
            resource: None,
        });
        assert_eq!(entra.get("audience").map(String::as_str), Some("api"));
        assert_eq!(
            entra.get("scope").map(String::as_str),
            Some("api://app/.default offline_access")
        );
        assert!(!entra.contains_key("resource"));

        let unscoped = query_of(&RequestedAccess::without_scope("https://mcp.example/mcp"));
        assert!(!unscoped.contains_key("scope"));
        assert_eq!(
            unscoped.get("resource").map(String::as_str),
            Some("https://mcp.example/mcp")
        );
    }

    #[test]
    fn first_authorization_keeps_the_secret_registered_with_the_client_id() {
        let mut connector = oauth_connector("https://as.example/token");
        connector.oauth_client_id = Some("entra-app".to_string());
        connector.oauth_client_secret = Some("encrypted-secret".to_string());

        assert_eq!(
            stored_secret_for(&connector, "entra-app").as_deref(),
            Some("encrypted-secret")
        );
        assert_eq!(
            stored_secret_for(&connector, "another-client"),
            None,
            "a stored secret never pairs with a different client id"
        );
    }

    #[test]
    fn token_error_reports_the_providers_reason_and_tolerates_non_json() {
        let entra = r#"{"error":"invalid_request","error_description":"AADSTS900144: The request body must contain the following parameter: 'scope'.","error_codes":[900144]}"#;
        let (error, description) = token_error(entra);
        assert_eq!(error, "invalid_request");
        assert!(description.starts_with("AADSTS900144"));

        let long = format!(
            r#"{{"error":"x","error_description":"{}"}}"#,
            "d".repeat(500)
        );
        assert_eq!(
            token_error(&long).1.len(),
            TOKEN_ERROR_DESCRIPTION_MAX_CHARS
        );

        assert_eq!(
            token_error("<html>bad gateway</html>"),
            (String::new(), String::new())
        );
    }

    const UNSCOPED_REFRESH: &str = "grant_type=refresh_token&refresh_token=rt&client_id=app";
    const SCOPED_REFRESH: &str = "grant_type=refresh_token&refresh_token=rt&scope=api%3A%2F%2Fapp%2Fmcp.access&client_id=app";

    async fn mount_refresh(
        server: &mut mockito::ServerGuard,
        body: &str,
        status: usize,
        response: Value,
        hits: usize,
    ) -> mockito::Mock {
        server
            .mock("POST", "/token")
            .match_body(mockito::Matcher::Exact(body.to_string()))
            .with_status(status)
            .with_header("content-type", "application/json")
            .with_body(response.to_string())
            .expect(hits)
            .create_async()
            .await
    }

    fn refresh_request(token_endpoint: &str) -> RefreshRequest<'_> {
        RefreshRequest {
            token_endpoint,
            client_id: Some("app"),
            client_secret: None,
            refresh_token: "rt",
        }
    }

    #[tokio::test]
    async fn refresh_retries_with_the_granted_scope_only_after_a_400() {
        let mut server = mockito::Server::new_async().await;
        let entra_rejection =
            json!({"error": "invalid_request", "error_description": "AADSTS90009: ..."});
        let unscoped = mount_refresh(&mut server, UNSCOPED_REFRESH, 400, entra_rejection, 1).await;
        let scoped = mount_refresh(
            &mut server,
            SCOPED_REFRESH,
            200,
            json!({"access_token": "new"}),
            1,
        )
        .await;
        let endpoint = format!("{}/token", server.url());

        let outcome = send_refresh(
            &reqwest::Client::new(),
            &refresh_request(&endpoint),
            Some("api://app/mcp.access"),
            "entra",
        )
        .await;

        unscoped.assert_async().await;
        scoped.assert_async().await;
        assert!(matches!(outcome, RefreshOutcome::Granted(body) if body["access_token"] == "new"));
    }

    #[tokio::test]
    async fn refresh_sends_no_scope_when_the_first_request_succeeds_or_none_was_granted() {
        let mut server = mockito::Server::new_async().await;
        let endpoint = format!("{}/token", server.url());
        let granted = mount_refresh(
            &mut server,
            UNSCOPED_REFRESH,
            200,
            json!({"access_token": "new"}),
            1,
        )
        .await;
        let never_scoped = mount_refresh(
            &mut server,
            SCOPED_REFRESH,
            200,
            json!({"access_token": "x"}),
            0,
        )
        .await;

        let outcome = send_refresh(
            &reqwest::Client::new(),
            &refresh_request(&endpoint),
            Some("api://app/mcp.access"),
            "working-provider",
        )
        .await;
        assert!(matches!(outcome, RefreshOutcome::Granted(_)));
        granted.assert_async().await;

        granted.remove_async().await;
        let rejected = mount_refresh(
            &mut server,
            UNSCOPED_REFRESH,
            400,
            json!({"error": "invalid_grant"}),
            1,
        )
        .await;
        let outcome = send_refresh(
            &reqwest::Client::new(),
            &refresh_request(&endpoint),
            None,
            "no-scope",
        )
        .await;
        assert!(matches!(
            outcome,
            RefreshOutcome::Rejected { status: 400, .. }
        ));
        rejected.assert_async().await;
        never_scoped.assert_async().await;
    }
}

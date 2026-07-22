//! Custom MCP connector registration, probe, sharing, and deletion — pure logic
//! behind `/api/mcp/connectors*`.

use std::collections::HashMap;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use reqwest::StatusCode;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::error::{McpError, Result};
use crate::permissions;
use crate::repo::{self, McpConnector, NewConnector};
use crate::state::McpState;
use crate::types::{GrantType, PUBLIC_GRANTEE};

pub const AUTH_TYPES: [&str; 5] = ["none", "bearer", "basic", "oauth2", "url_param"];

pub fn connector_dto(c: &McpConnector) -> Value {
    json!({
        "connector_id": c.id,
        "provider_type": c.provider_type,
        "owner_id": c.owner_id,
        "name": c.name,
        "url": c.url,
        "transport": c.transport,
        "auth_type": c.auth_type,
        "url_param_name": c.url_param_name,
        "credential_header_name": c.credential_header_name,
        "description": c.description,
        "display_name": c.display_name,
        "logo_url": c.logo_url,
        "is_active": c.active(),
        "oauth_configured": c.oauth_configured(),
        "created_at": c.created_at,
        "updated_at": c.updated_at,
    })
}

// ─── Probe (pure classification, unchanged from v1) ─────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetectedAuthType {
    None,
    OAuth2,
    Bearer,
}

impl DetectedAuthType {
    pub fn as_str(self) -> &'static str {
        match self {
            DetectedAuthType::None => "none",
            DetectedAuthType::OAuth2 => "oauth2",
            DetectedAuthType::Bearer => "bearer",
        }
    }
}

fn classify_response(status: StatusCode, www_authenticate: &str) -> DetectedAuthType {
    if status.is_success() {
        return DetectedAuthType::None;
    }
    if status == StatusCode::UNAUTHORIZED {
        return if www_authenticate.contains("resource_metadata") {
            DetectedAuthType::OAuth2
        } else {
            DetectedAuthType::Bearer
        };
    }
    DetectedAuthType::Bearer
}

/// POST a bare `initialize` and classify the response into an auth type + status.
pub async fn probe_initialize(
    http_client: &reqwest::Client,
    url: &str,
) -> std::result::Result<(DetectedAuthType, u16), reqwest::Error> {
    let resp = http_client
        .post(url)
        .timeout(std::time::Duration::from_secs(10))
        .header("Content-Type", "application/json")
        .body(
            json!({
                "jsonrpc": "2.0", "id": 1, "method": "initialize",
                "params": {"protocolVersion": "2024-11-05", "capabilities": {},
                           "clientInfo": {"name": "mcp-gateway-probe", "version": "1.0"}},
            })
            .to_string(),
        )
        .send()
        .await?;

    let status = resp.status();
    let www = resp
        .headers()
        .get(reqwest::header::WWW_AUTHENTICATE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    Ok((classify_response(status, &www), status.as_u16()))
}

/// `POST /connectors/probe` — detect a server's auth type without storing anything.
pub async fn probe_connector_view(state: &McpState, url: &str) -> Result<Value> {
    let url = url.trim_end_matches('/').to_string();
    crate::net::validate_public_url(&url).await?;

    // Guarded client: `validate_public_url` is a one-shot pre-check; the probe
    // itself must go through the SSRF/DNS-rebinding-guarded client so a rebinding
    // DNS can't point it at an internal address between the two resolutions.
    let (detected, status) = probe_initialize(&state.guarded_http_client, &url)
        .await
        .map_err(|e| McpError::Backend(format!("could not reach MCP server: {e}")))?;

    let (requires, hint) = match detected {
        DetectedAuthType::None => ("nothing", "This server requires no authentication.".to_string()),
        DetectedAuthType::OAuth2 => {
            ("oauth_flow", "This server uses OAuth 2.1. You will be redirected to authorize.".to_string())
        }
        DetectedAuthType::Bearer if status == StatusCode::UNAUTHORIZED.as_u16() => {
            ("api_key_input", "This server requires a Bearer token or API key.".to_string())
        }
        DetectedAuthType::Bearer => {
            ("api_key_input", format!("Server returned HTTP {status}. It may require an API key."))
        }
    };
    Ok(json!({ "url": url, "auth_type": detected.as_str(), "requires": requires, "hint": hint }))
}

// ─── Registration ─────────────────────────────────────────────────────────────

/// Inputs for registering a custom MCP connector (always owner-scoped).
pub struct NewConnectorInput {
    pub name: String,
    pub url: String,
    pub transport: String,
    pub auth_type: String,
    pub url_param_name: Option<String>,
    pub credential_header_name: Option<String>,
    pub headers: Option<HashMap<String, String>>,
    pub basic_username: Option<String>,
    pub basic_password: Option<String>,
    pub description: Option<String>,
    pub display_name: Option<String>,
    pub logo_url: Option<String>,
}

/// Register a custom MCP connector owned by `owner_id` (private until shared).
pub async fn register_connector(
    state: &McpState,
    owner_id: Uuid,
    input: NewConnectorInput,
) -> Result<McpConnector> {
    if !AUTH_TYPES.contains(&input.auth_type.as_str()) {
        return Err(McpError::BadRequest(format!("auth_type must be one of {AUTH_TYPES:?}")));
    }
    if input.auth_type == "url_param" && input.url_param_name.is_none() {
        return Err(McpError::BadRequest("url_param_name is required when auth_type='url_param'".into()));
    }
    crate::net::validate_public_url(&input.url).await?;

    if repo::get_owned_connector_by_name(&state.db, owner_id, &input.name).await?.is_some() {
        return Err(McpError::Conflict(format!("you already have a connector named '{}'", input.name)));
    }

    // For basic auth, precompute the Authorization: Basic header into static headers.
    let mut headers = input.headers.clone().unwrap_or_default();
    if input.auth_type == "basic"
        && let (Some(u), Some(p)) = (&input.basic_username, &input.basic_password)
    {
        let encoded = B64.encode(format!("{u}:{p}"));
        headers.insert("Authorization".to_string(), format!("Basic {encoded}"));
    }
    let headers_json = if headers.is_empty() { None } else { Some(serde_json::to_value(&headers).unwrap_or(Value::Null)) };

    let connector = repo::create_connector(
        &state.db,
        &NewConnector {
            provider_type: "mcp_server".to_string(),
            owner_id: Some(owner_id),
            name: input.name,
            display_name: input.display_name,
            logo_url: input.logo_url,
            description: input.description,
            url: Some(input.url),
            transport: Some(input.transport),
            auth_type: Some(input.auth_type),
            url_param_name: input.url_param_name,
            credential_header_name: input.credential_header_name,
            headers: headers_json,
            is_active: Some(true),
            ..Default::default()
        },
    )
    .await?;
    tracing::info!(name = %connector.name, %owner_id, "registered mcp connector");
    Ok(connector)
}

/// Partial-update input for [`update_connector`]. `None` fields are unchanged.
#[derive(Default)]
pub struct UpdateConnectorInput {
    pub name: Option<String>,
    pub url: Option<String>,
    pub transport: Option<String>,
    pub auth_type: Option<String>,
    pub url_param_name: Option<String>,
    pub credential_header_name: Option<String>,
    pub headers: Option<HashMap<String, String>>,
    pub description: Option<String>,
    pub display_name: Option<String>,
    pub logo_url: Option<String>,
    pub is_active: Option<bool>,
}

/// Update an owned custom connector (owner or admin). Avoids the destructive
/// delete+recreate that would CASCADE away every connected user's credentials.
pub async fn update_connector(
    state: &McpState,
    caller: Uuid,
    is_admin: bool,
    id: Uuid,
    input: UpdateConnectorInput,
) -> Result<McpConnector> {
    let connector = repo::get_connector_by_id(&state.db, id)
        .await?
        .ok_or_else(|| McpError::NotFound(format!("connector '{id}' not found")))?;
    if !connector.is_mcp_server() {
        return Err(McpError::BadRequest("only custom MCP connectors can be updated here".into()));
    }
    if !is_admin && connector.owner_id != Some(caller) {
        return Err(McpError::Forbidden("this connector does not belong to you".into()));
    }

    if let Some(at) = &input.auth_type
        && !AUTH_TYPES.contains(&at.as_str())
    {
        return Err(McpError::BadRequest(format!("auth_type must be one of {AUTH_TYPES:?}")));
    }
    // url_param needs a param name (using the post-update effective values).
    let effective_auth = input.auth_type.clone().or_else(|| connector.auth_type.clone()).unwrap_or_default();
    let effective_param = input.url_param_name.clone().or_else(|| connector.url_param_name.clone());
    if effective_auth == "url_param" && effective_param.is_none() {
        return Err(McpError::BadRequest("url_param_name is required when auth_type='url_param'".into()));
    }
    if let Some(url) = &input.url {
        crate::net::validate_public_url(url).await?;
    }
    // Name-collision check within the owner's scope.
    let owner = connector.owner_id.unwrap_or(caller);
    if let Some(name) = &input.name
        && name != &connector.name
        && let Some(existing) = repo::get_owned_connector_by_name(&state.db, owner, name).await?
        && existing.id != connector.id
    {
        return Err(McpError::Conflict(format!("you already have a connector named '{name}'")));
    }

    let headers_json = input.headers.as_ref().map(|h| serde_json::to_value(h).unwrap_or(Value::Null));
    let updated = repo::update_connector(
        &state.db,
        id,
        &repo::UpdateConnector {
            name: input.name,
            url: input.url,
            transport: input.transport,
            auth_type: input.auth_type,
            url_param_name: input.url_param_name,
            credential_header_name: input.credential_header_name,
            headers: headers_json,
            description: input.description,
            display_name: input.display_name,
            logo_url: input.logo_url,
            is_active: input.is_active,
        },
    )
    .await?;
    tracing::info!(name = %updated.name, "updated mcp connector");
    Ok(updated)
}

/// `GET /api/mcp/connectors` — custom connectors visible to the caller.
pub async fn list_connectors_view(state: &McpState, user_id: Uuid) -> Result<Value> {
    let connectors = state.authorizer.list_accessible_mcp_connectors(&state.db, user_id).await?;
    let data: Vec<Value> = connectors
        .iter()
        .map(|c| {
            let mut dto = connector_dto(c);
            dto["is_owner"] = json!(c.owner_id == Some(user_id));
            dto
        })
        .collect();
    let total = data.len();
    Ok(json!({ "data": data, "total": total }))
}

/// `GET /api/mcp/connectors/{id}` — a single connector. 404s (not 403) when the
/// caller can't reach it, so the response never leaks whether the id exists.
pub async fn get_connector_view(state: &McpState, user_id: Uuid, connector_id: Uuid) -> Result<Value> {
    if !state.authorizer.can_access_connector(&state.db, user_id, connector_id).await? {
        return Err(McpError::NotFound(format!("connector '{connector_id}' not found")));
    }
    let connector = repo::get_connector_by_id(&state.db, connector_id)
        .await?
        .ok_or_else(|| McpError::NotFound(format!("connector '{connector_id}' not found")))?;
    let mut dto = connector_dto(&connector);
    dto["is_owner"] = json!(connector.owner_id == Some(user_id));
    Ok(dto)
}

/// Load a connector for a pending delete (authorization done by the caller).
pub async fn get_connector_for_deletion(state: &McpState, id: Uuid) -> Result<McpConnector> {
    repo::get_connector_by_id(&state.db, id)
        .await?
        .ok_or_else(|| McpError::NotFound(format!("connector '{id}' not found")))
}

/// Delete a connector (already authorized) + invalidate affected permission caches.
/// CASCADE removes its grants, tools, connections, and access rows.
pub async fn delete_connector(state: &McpState, connector: &McpConnector) -> Result<()> {
    let agent_ids = repo::get_agents_for_connector(&state.db, connector.id).await?;
    repo::delete_connector(&state.db, connector.id).await?;
    for aid in agent_ids {
        permissions::invalidate_permission_cache(state, aid).await;
    }
    tracing::info!(name = %connector.name, "deleted mcp connector");
    Ok(())
}

// ─── Sharing ────────────────────────────────────────────────────────────────

/// Who a connector is being shared with.
pub enum ShareTarget {
    /// A specific username.
    User(String),
    /// Everyone (`'*'`).
    Public,
}

/// Load a connector and confirm `caller` may share it (owner or admin).
async fn owned_shareable(state: &McpState, caller: Uuid, is_admin: bool, connector_id: Uuid) -> Result<McpConnector> {
    let connector = repo::get_connector_by_id(&state.db, connector_id)
        .await?
        .ok_or_else(|| McpError::NotFound(format!("connector '{connector_id}' not found")))?;
    if !connector.is_mcp_server() {
        return Err(McpError::BadRequest("only custom MCP connectors can be shared".into()));
    }
    if !is_admin && connector.owner_id != Some(caller) {
        return Err(McpError::Forbidden("only the owner can share this connector".into()));
    }
    Ok(connector)
}

/// Owner/admin-gated grant write, generic over `grant_type`/`grantee_id` (raw
/// strings) so an edition can add grant kinds without this crate knowing them.
/// `grantee_id` must already be resolved (a target id, or '*' for public).
pub async fn create_share_grant(
    state: &McpState,
    caller: Uuid,
    is_admin: bool,
    connector_id: Uuid,
    grant_type: &str,
    grantee_id: &str,
) -> Result<Value> {
    let connector = owned_shareable(state, caller, is_admin, connector_id).await?;
    let grant = repo::create_grant(&state.db, connector.id, grant_type, grantee_id, caller).await?;
    tracing::info!(connector_id = %connector.id, grant_type, grantee = %grantee_id, "shared connector");
    Ok(json!({ "grant_id": grant.id, "grant_type": grant.grant_type, "grantee_id": grant.grantee_id }))
}

/// Owner/admin-gated grant revoke — deletes the grant AND the grantee's
/// connection (fix #2), then drops affected permission caches. Generic over
/// `grant_type`/`grantee_id`. Invalidates every pair referencing the connector
/// (a safe superset — covers multi-user grant kinds without resolving members;
/// access itself is a live DB read, so this only keeps caches fresh).
pub async fn revoke_share_grant(
    state: &McpState,
    caller: Uuid,
    is_admin: bool,
    connector_id: Uuid,
    grant_type: &str,
    grantee_id: &str,
) -> Result<()> {
    let connector = owned_shareable(state, caller, is_admin, connector_id).await?;
    let removed = repo::revoke_grant_and_connection(&state.db, connector.id, grant_type, grantee_id).await?;
    if !removed {
        return Err(McpError::NotFound("no matching share to revoke".into()));
    }
    for aid in repo::get_agents_for_connector(&state.db, connector.id).await? {
        permissions::invalidate_permission_cache(state, aid).await;
    }
    tracing::info!(connector_id = %connector.id, grant_type, grantee = %grantee_id, "revoked share");
    Ok(())
}

/// Resolve a [`ShareTarget`] to `(grant_type, grantee_id, grantee_user)`.
async fn resolve_target(state: &McpState, target: ShareTarget) -> Result<(&'static str, String, Option<Uuid>)> {
    match target {
        ShareTarget::Public => Ok((GrantType::Public.as_str(), PUBLIC_GRANTEE.to_string(), None)),
        ShareTarget::User(username) => {
            let uid = repo::resolve_username_to_user_id(&state.db, &username)
                .await?
                .ok_or_else(|| McpError::NotFound(format!("user '{username}' not found")))?;
            Ok((GrantType::User.as_str(), uid.to_string(), Some(uid)))
        }
    }
}

/// Share a connector with a user (by username) or everyone.
pub async fn share_connector(
    state: &McpState,
    caller: Uuid,
    is_admin: bool,
    connector_id: Uuid,
    target: ShareTarget,
) -> Result<Value> {
    let (grant_type, grantee_id, grantee_user) = resolve_target(state, target).await?;
    let view = create_share_grant(state, caller, is_admin, connector_id, grant_type, &grantee_id).await?;
    if let Some(uid) = grantee_user {
        crate::session::invalidate_session_cache(state, uid).await;
    }
    Ok(view)
}

/// Revoke a share — deletes the grant AND the grantee's connection (fix #2).
pub async fn revoke_share(
    state: &McpState,
    caller: Uuid,
    is_admin: bool,
    connector_id: Uuid,
    target: ShareTarget,
) -> Result<()> {
    let (grant_type, grantee_id, grantee_user) = resolve_target(state, target).await?;
    revoke_share_grant(state, caller, is_admin, connector_id, grant_type, &grantee_id).await?;
    if let Some(uid) = grantee_user {
        crate::session::invalidate_session_cache(state, uid).await;
    }
    Ok(())
}

/// `GET /api/mcp/connectors/{id}/share` — list a connector's grants.
pub async fn list_shares_view(state: &McpState, caller: Uuid, is_admin: bool, connector_id: Uuid) -> Result<Value> {
    let connector = owned_shareable(state, caller, is_admin, connector_id).await?;
    let grants = repo::list_grants_for_connector(&state.db, connector.id).await?;
    let data: Vec<Value> = grants
        .into_iter()
        .map(|g| {
            json!({
                "grant_id": g.id,
                "grant_type": g.grant_type,
                "grantee_id": g.grantee_id,
                "granted_by": g.granted_by,
                "created_at": g.created_at,
            })
        })
        .collect();
    Ok(json!({ "data": data }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_response_cases() {
        assert_eq!(classify_response(StatusCode::OK, ""), DetectedAuthType::None);
        assert_eq!(classify_response(StatusCode::CREATED, "Bearer"), DetectedAuthType::None);
        assert_eq!(
            classify_response(StatusCode::UNAUTHORIZED, "Bearer resource_metadata=\"x\""),
            DetectedAuthType::OAuth2
        );
        assert_eq!(classify_response(StatusCode::UNAUTHORIZED, ""), DetectedAuthType::Bearer);
        assert_eq!(classify_response(StatusCode::INTERNAL_SERVER_ERROR, ""), DetectedAuthType::Bearer);
        assert_eq!(classify_response(StatusCode::NOT_FOUND, ""), DetectedAuthType::Bearer);
    }

    #[test]
    fn detected_auth_type_as_str() {
        assert_eq!(DetectedAuthType::None.as_str(), "none");
        assert_eq!(DetectedAuthType::OAuth2.as_str(), "oauth2");
        assert_eq!(DetectedAuthType::Bearer.as_str(), "bearer");
    }

    #[test]
    fn auth_types_contains_all_five() {
        for kind in ["none", "bearer", "basic", "oauth2", "url_param"] {
            assert!(AUTH_TYPES.contains(&kind));
        }
        assert_eq!(AUTH_TYPES.len(), 5);
    }
}

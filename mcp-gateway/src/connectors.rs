//! Custom MCP connector registration, probe, sharing, and deletion — pure logic
//! behind `/api/mcp/connectors*`.

use std::collections::{HashMap, HashSet};

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
        "source_kind": c.source_kind,
        "build_status": c.build_status,
        "setup_status": c.setup_status,
        "setup_error": c.setup_error,
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
        // Required by the MCP Streamable HTTP transport spec — a spec-compliant
        // no-auth server (e.g. mcp.deepwiki.com) responds 406 without this and
        // gets misclassified as requiring a bearer token, for a reason that has
        // nothing to do with authentication.
        .header("Accept", "application/json, text/event-stream")
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

    // Primary: RFC 9728 direct discovery — the MCP spec's own recommended
    // method, and deterministic when a server implements it (unlike guessing
    // from a bare unauthenticated response, which is a real, verified source
    // of false classifications — see classify_response's doc comment).
    if crate::oauth::fetch_protected_resource_metadata(&state.guarded_http_client, &url)
        .await
        .is_some()
    {
        return Ok(json!({
            "url": url,
            "auth_type": "oauth2",
            "requires": "oauth_flow",
            "hint": "This server publishes OAuth 2.0 Protected Resource Metadata (RFC 9728) \
                     — it supports OAuth 2.1. You will be redirected to authorize."
                .to_string(),
        }));
    }

    // Fallback: no well-known metadata found (either the server doesn't
    // support OAuth, or — confirmed live for at least one real server,
    // Atlassian — it does but doesn't publish RFC 9728 metadata at all).
    // Guarded client: `validate_public_url` is a one-shot pre-check; the probe
    // itself must go through the SSRF/DNS-rebinding-guarded client so a rebinding
    // DNS can't point it at an internal address between the two resolutions.
    let (detected, status) = probe_initialize(&state.guarded_http_client, &url)
        .await
        .map_err(|e| McpError::Backend(format!("could not reach MCP server: {e}")))?;

    let (requires, hint) = match detected {
        DetectedAuthType::None => (
            "nothing",
            "This server requires no authentication.".to_string(),
        ),
        DetectedAuthType::OAuth2 => (
            "oauth_flow",
            "This server uses OAuth 2.1. You will be redirected to authorize.".to_string(),
        ),
        DetectedAuthType::Bearer if status == StatusCode::UNAUTHORIZED.as_u16() => (
            "api_key_input",
            "This server requires a Bearer token or API key.".to_string(),
        ),
        DetectedAuthType::Bearer => (
            "api_key_input",
            format!("Server returned HTTP {status}. It may require an API key."),
        ),
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
        return Err(McpError::BadRequest(format!(
            "auth_type must be one of {AUTH_TYPES:?}"
        )));
    }
    if input.auth_type == "url_param" && input.url_param_name.is_none() {
        return Err(McpError::BadRequest(
            "url_param_name is required when auth_type='url_param'".into(),
        ));
    }
    crate::net::validate_public_url(&input.url).await?;

    if repo::get_owned_connector_by_name(&state.db, owner_id, &input.name)
        .await?
        .is_some()
    {
        return Err(McpError::Conflict(format!(
            "you already have a connector named '{}'",
            input.name
        )));
    }

    // `none` needs nothing further; every other auth_type still needs a
    // credential registered (bearer/basic/url_param) or a browser OAuth
    // round-trip (oauth2) before the connector is actually usable. `basic`
    // supplied here at registration time is the one exception — it becomes
    // immediately verifiable, handled below once the connector row exists.
    let initial_setup_status = if input.auth_type == "none" {
        "active"
    } else {
        "pending"
    };

    // For basic auth, precompute the Authorization: Basic header into static headers.
    let mut headers = input.headers.clone().unwrap_or_default();
    let basic_ready = input.auth_type == "basic" && input.basic_username.is_some() && input.basic_password.is_some();
    if input.auth_type == "basic"
        && let (Some(u), Some(p)) = (&input.basic_username, &input.basic_password)
    {
        let encoded = B64.encode(format!("{u}:{p}"));
        headers.insert("Authorization".to_string(), format!("Basic {encoded}"));
    }
    let headers_json = if headers.is_empty() {
        None
    } else {
        Some(serde_json::to_value(&headers).unwrap_or(Value::Null))
    };

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
            source_kind: repo::SourceKind::ExternalUrl,
            build_status: None,
            ..Default::default()
        },
    )
    .await?;

    // `basic` supplied at registration is immediately verifiable — prove it
    // actually works instead of leaving it marked `pending` indefinitely.
    let (final_status, final_error) = if basic_ready {
        let outcome = crate::credentials::verify_connector_live(state, owner_id, &connector).await;
        if outcome.verified { ("active".to_string(), None) } else { ("failed".to_string(), outcome.error) }
    } else {
        (initial_setup_status.to_string(), None)
    };
    repo::set_connector_setup_status(&state.db, connector.id, &final_status, final_error.as_deref()).await?;
    tracing::info!(name = %connector.name, %owner_id, setup_status = %final_status, "registered mcp connector");
    Ok(McpConnector {
        setup_status: Some(final_status),
        setup_error: final_error,
        ..connector
    })
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
        return Err(McpError::BadRequest(
            "only custom MCP connectors can be updated here".into(),
        ));
    }
    if !is_admin && connector.owner_id != Some(caller) {
        return Err(McpError::Forbidden(
            "this connector does not belong to you".into(),
        ));
    }

    if let Some(at) = &input.auth_type
        && !AUTH_TYPES.contains(&at.as_str())
    {
        return Err(McpError::BadRequest(format!(
            "auth_type must be one of {AUTH_TYPES:?}"
        )));
    }
    // url_param needs a param name (using the post-update effective values).
    let effective_auth = input
        .auth_type
        .clone()
        .or_else(|| connector.auth_type.clone())
        .unwrap_or_default();
    let effective_param = input
        .url_param_name
        .clone()
        .or_else(|| connector.url_param_name.clone());
    if effective_auth == "url_param" && effective_param.is_none() {
        return Err(McpError::BadRequest(
            "url_param_name is required when auth_type='url_param'".into(),
        ));
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
        return Err(McpError::Conflict(format!(
            "you already have a connector named '{name}'"
        )));
    }

    let headers_json = input
        .headers
        .as_ref()
        .map(|h| serde_json::to_value(h).unwrap_or(Value::Null));
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

/// `GET /api/mcp/connectors` — custom connectors visible to the caller,
/// grouped into "created_by_you" and "shared_with_you" with per-card
/// metadata (tool count, connection status, version, author).
pub async fn list_connectors_view(state: &McpState, user_id: Uuid) -> Result<Value> {
    let connectors = state
        .authorizer
        .list_accessible_mcp_connectors(&state.db, user_id)
        .await?;

    let ids: Vec<Uuid> = connectors.iter().map(|c| c.id).collect();

    // Batch-fetch tool counts.
    let tool_counts: HashMap<Uuid, i64> = if ids.is_empty() {
        HashMap::new()
    } else {
        sqlx::query_as::<_, (Uuid, i64)>(
            "SELECT connector_id, COUNT(*) FROM mcp_connector_tools \
             WHERE connector_id = ANY($1) GROUP BY connector_id",
        )
        .bind(&ids)
        .fetch_all(&state.db)
        .await
        .unwrap_or_default()
        .into_iter()
        .collect()
    };

    // Batch-fetch caller's connection status.
    let connected_set: HashSet<Uuid> = if ids.is_empty() {
        HashSet::new()
    } else {
        sqlx::query_scalar::<_, Uuid>(
            "SELECT connector_id FROM mcp_user_connections \
             WHERE connector_id = ANY($1) AND user_id = $2 AND status = 'ACTIVE'",
        )
        .bind(&ids)
        .bind(user_id)
        .fetch_all(&state.db)
        .await
        .unwrap_or_default()
        .into_iter()
        .collect()
    };

    // Batch-fetch version tags for uploaded connectors.
    let version_map: HashMap<Uuid, String> = if ids.is_empty() {
        HashMap::new()
    } else {
        sqlx::query_as::<_, (Uuid, String)>(
            "SELECT DISTINCT ON (connector_id) connector_id, version_tag \
             FROM mcp_connector_builds WHERE connector_id = ANY($1) \
             ORDER BY connector_id, created_at DESC",
        )
        .bind(&ids)
        .fetch_all(&state.db)
        .await
        .unwrap_or_default()
        .into_iter()
        .collect()
    };

    // Batch-fetch owner usernames.
    let owner_ids: Vec<Uuid> = connectors
        .iter()
        .filter_map(|c| c.owner_id)
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    let owner_names: HashMap<Uuid, String> = if owner_ids.is_empty() {
        HashMap::new()
    } else {
        sqlx::query_as::<_, (Uuid, String)>(
            "SELECT id, username FROM users WHERE id = ANY($1)",
        )
        .bind(&owner_ids)
        .fetch_all(&state.db)
        .await
        .unwrap_or_default()
        .into_iter()
        .collect()
    };

    let mut created_by_you = Vec::new();
    let mut shared_with_you = Vec::new();

    for c in &connectors {
        let is_owner = c.owner_id == Some(user_id);
        let mut dto = connector_dto(c);
        dto["is_owner"] = json!(is_owner);
        dto["tool_count"] = json!(tool_counts.get(&c.id).copied().unwrap_or(0));
        dto["is_connected"] = json!(connected_set.contains(&c.id));
        dto["version"] = json!(version_map.get(&c.id));
        if let Some(oid) = c.owner_id {
            dto["owner_username"] = json!(owner_names.get(&oid));
        }

        if is_owner {
            created_by_you.push(dto);
        } else {
            shared_with_you.push(dto);
        }
    }

    let total = created_by_you.len() + shared_with_you.len();
    Ok(json!({
        "created_by_you": created_by_you,
        "shared_with_you": shared_with_you,
        "total": total,
    }))
}

/// `GET /api/mcp/connectors/{id}` — a single connector. 404s (not 403) when the
/// caller can't reach it, so the response never leaks whether the id exists.
pub async fn get_connector_view(
    state: &McpState,
    user_id: Uuid,
    connector_id: Uuid,
) -> Result<Value> {
    if !state
        .authorizer
        .can_access_connector(&state.db, user_id, connector_id)
        .await?
    {
        return Err(McpError::NotFound(format!(
            "connector '{connector_id}' not found"
        )));
    }
    let connector = repo::get_connector_by_id(&state.db, connector_id)
        .await?
        .ok_or_else(|| McpError::NotFound(format!("connector '{connector_id}' not found")))?;
    let mut dto = connector_dto(&connector);
    dto["is_owner"] = json!(connector.owner_id == Some(user_id));

    // Tool count
    let tool_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM mcp_connector_tools WHERE connector_id = $1",
    )
    .bind(connector_id)
    .fetch_one(&state.db)
    .await
    .unwrap_or(0);
    dto["tool_count"] = json!(tool_count);

    // Connection count
    let connection_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM mcp_user_connections WHERE connector_id = $1",
    )
    .bind(connector_id)
    .fetch_one(&state.db)
    .await
    .unwrap_or(0);
    dto["connection_count"] = json!(connection_count);

    // Is public
    let is_public: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM mcp_connector_grants WHERE connector_id = $1 AND grant_type = 'public')",
    )
    .bind(connector_id)
    .fetch_one(&state.db)
    .await
    .unwrap_or(false);
    dto["is_public"] = json!(is_public);

    // Version tag + image tag (uploaded builds only)
    if connector.source_kind == repo::SourceKind::UploadedBuild {
        let build_info: Option<(Option<String>, Option<String>)> = sqlx::query_as(
            "SELECT version_tag, image_reference FROM mcp_connector_builds \
             WHERE connector_id = $1 ORDER BY created_at DESC LIMIT 1",
        )
        .bind(connector_id)
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten();
        if let Some((version, image)) = build_info {
            dto["version"] = json!(version);
            dto["image_tag"] = json!(image);
        }
    }

    // Owner username
    if let Some(owner_id) = connector.owner_id {
        let owner_name: Option<String> = sqlx::query_scalar(
            "SELECT username FROM users WHERE id = $1",
        )
        .bind(owner_id)
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten();
        dto["owner_username"] = json!(owner_name);
    }

    // Caller's credential status
    let has_credential: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM mcp_user_connections \
         WHERE connector_id = $1 AND user_id = $2 AND encrypted_credential IS NOT NULL)",
    )
    .bind(connector_id)
    .bind(user_id)
    .fetch_one(&state.db)
    .await
    .unwrap_or(false);
    dto["has_credential"] = json!(has_credential);

    // Agent grant count (how many agents have access)
    let agent_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM mcp_connector_grants WHERE connector_id = $1 AND grant_type = 'agent'",
    )
    .bind(connector_id)
    .fetch_one(&state.db)
    .await
    .unwrap_or(0);
    dto["agent_count"] = json!(agent_count);

    Ok(dto)
}

/// Load a connector for a pending delete (authorization done by the caller).
pub async fn get_connector_for_deletion(state: &McpState, id: Uuid) -> Result<McpConnector> {
    repo::get_connector_by_id(&state.db, id)
        .await?
        .ok_or_else(|| McpError::NotFound(format!("connector '{id}' not found")))
}

/// `DELETE /api/mcp/connectors/{id}` — owner/admin-gated. Ownership is enforced
/// here (in the crate), but the delete itself is left to the caller: returns
/// the authorized connector so `oss/server`'s `service::connectors::delete` can
/// destroy an `uploaded_build` connector's container (needs `ContainerRuntime`,
/// which this crate deliberately never depends on) BEFORE removing the DB row
/// — so an interruption mid-delete leaves a retryable DB row, never an
/// orphaned container with no DB trace pointing at it.
pub async fn authorize_delete(
    state: &McpState,
    caller: Uuid,
    is_admin: bool,
    id: Uuid,
) -> Result<McpConnector> {
    let connector = get_connector_for_deletion(state, id).await?;
    if !is_admin && connector.owner_id != Some(caller) {
        return Err(McpError::Forbidden(
            "this connector does not belong to you".into(),
        ));
    }
    Ok(connector)
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
    /// A specific user by ID.
    User(Uuid),
    /// Everyone (`'*'`).
    Public,
}

/// Load a connector and confirm `caller` may share it (owner or admin).
async fn owned_shareable(
    state: &McpState,
    caller: Uuid,
    is_admin: bool,
    connector_id: Uuid,
) -> Result<McpConnector> {
    let connector = repo::get_connector_by_id(&state.db, connector_id)
        .await?
        .ok_or_else(|| McpError::NotFound(format!("connector '{connector_id}' not found")))?;
    if !connector.is_mcp_server() {
        return Err(McpError::BadRequest(
            "only custom MCP connectors can be shared".into(),
        ));
    }
    if !is_admin && connector.owner_id != Some(caller) {
        return Err(McpError::Forbidden(
            "only the owner can share this connector".into(),
        ));
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
    Ok(json!({ "id": grant.id, "grant_type": grant.grant_type, "grantee_id": grant.grantee_id }))
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
    let removed =
        repo::revoke_grant_and_connection(&state.db, connector.id, grant_type, grantee_id).await?;
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
async fn resolve_target(
    state: &McpState,
    target: ShareTarget,
) -> Result<(&'static str, String, Option<Uuid>)> {
    match target {
        ShareTarget::Public => Ok((GrantType::Public.as_str(), PUBLIC_GRANTEE.to_string(), None)),
        ShareTarget::User(user_id) => {
            // Verify the user exists.
            let exists: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM users WHERE id = $1 AND deleted_at IS NULL)",
            )
            .bind(user_id)
            .fetch_one(&state.db)
            .await
            .map_err(McpError::Database)?;
            if !exists {
                return Err(McpError::NotFound(format!("user '{user_id}' not found")));
            }
            Ok((GrantType::User.as_str(), user_id.to_string(), Some(user_id)))
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
    let view = create_share_grant(
        state,
        caller,
        is_admin,
        connector_id,
        grant_type,
        &grantee_id,
    )
    .await?;
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
    revoke_share_grant(
        state,
        caller,
        is_admin,
        connector_id,
        grant_type,
        &grantee_id,
    )
    .await?;
    if let Some(uid) = grantee_user {
        crate::session::invalidate_session_cache(state, uid).await;
    }
    Ok(())
}

/// `GET /api/mcp/connectors/{id}/share` — list a connector's grants, plus who
/// has access and why (`access_reasons`, EE-aware via the authorizer seam)
/// and whether it's public (a flag, not a per-person reason).
pub async fn list_shares_view(
    state: &McpState,
    caller: Uuid,
    is_admin: bool,
    connector_id: Uuid,
) -> Result<Value> {
    let connector = owned_shareable(state, caller, is_admin, connector_id).await?;
    let grants = repo::list_grants_for_connector(&state.db, connector.id).await?;
    let is_public = grants.iter().any(|g| g.grant_type == "public");
    let access_reasons = state
        .authorizer
        .list_access_reasons(&state.db, &connector)
        .await?;
    let data: Vec<Value> = grants
        .into_iter()
        .map(|g| {
            json!({
                "id": g.id,
                "grant_type": g.grant_type,
                "grantee_id": g.grantee_id,
                "granted_by": g.granted_by,
                "created_at": g.created_at,
            })
        })
        .collect();
    Ok(json!({ "grants": data, "is_public": is_public, "access_reasons": access_reasons }))
}

/// `GET /api/mcp/share-targets?q=` — search users to share a connector with.
/// Open to any authenticated user (any owner may need to share), NOT admin-gated
/// like the platform's general user directory — capped and username-only so
/// that openness doesn't become a directory-enumeration/email-leak risk.
///
/// `visible_ids` is the org-visibility allowlist resolved by the server via
/// `AuthService::org_visible_user_ids`: `None` = unscoped (OSS, or an EE role
/// that sees everyone), `Some(ids)` = restrict to those users (EE members only
/// find users in their own team/department). `Some(empty)` matches no one.
pub async fn search_share_targets_view(
    state: &McpState,
    q: &str,
    visible_ids: Option<Vec<Uuid>>,
) -> Result<Value> {
    let q = q.trim();
    if q.chars().count() < 2 {
        return Err(McpError::BadRequest(
            "q must be at least 2 characters".into(),
        ));
    }
    let rows = repo::search_users_for_share(&state.db, q, 20, visible_ids.as_deref()).await?;
    let data: Vec<Value> = rows
        .into_iter()
        .map(|(id, username, display_name)| json!({ "user_id": id, "username": username, "display_name": display_name }))
        .collect();
    Ok(json!({ "users": data }))
}

/// `GET /api/mcp/connectors/{id}/consumers` — which agents, users, teams, and
/// departments actually use this connector. Owner/admin-gated, same as
/// sharing — a management view.
///
/// Agents are read off `mcp_agent_connector_access` (an explicit per-agent
/// override row) — correct for public connectors too, since it keys off the
/// override row, not owner reachability, so a public-only user's configured
/// agent still shows up. Users/teams/departments are read off the grant rows
/// directly (entity-level, not exploded per team member — that's
/// `list_access_reasons`'s job for the Share tab); team/department grants are
/// always empty in OSS via the authorizer seam.
pub async fn list_consumers_view(
    state: &McpState,
    caller: Uuid,
    is_admin: bool,
    connector_id: Uuid,
) -> Result<Value> {
    let connector = owned_shareable(state, caller, is_admin, connector_id).await?;
    let agents = repo::list_configured_agent_consumers(&state.db, connector.id).await?;

    // Total tools for this connector (for "X of Y" display).
    let total_tools: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM mcp_connector_tools WHERE connector_id = $1",
    )
    .bind(connector_id)
    .fetch_one(&state.db)
    .await
    .unwrap_or(0);

    let agents_json: Vec<Value> = agents
        .into_iter()
        .map(|a| {
            // A connector disabled outright for this agent (Layer-2 toggle) makes
            // every tool unusable, regardless of per-tool block rules.
            let tools_used = if !a.enabled {
                0
            } else {
                let blocked_count = a
                    .tool_rules
                    .as_array()
                    .map(|rules| {
                        rules
                            .iter()
                            .filter(|r| r.get("stance").and_then(|s| s.as_str()) == Some("block"))
                            .count() as i64
                    })
                    .unwrap_or(0);
                (total_tools - blocked_count).max(0)
            };

            json!({
                "agent_id": a.agent_id,
                "agent_name": a.agent_name,
                "agent_owner_id": a.agent_owner_id,
                "enabled": a.enabled,
                "tools_used": tools_used,
                "total_tools": total_tools,
                "tool_rules": a.tool_rules,
            })
        })
        .collect();

    let grants = repo::list_grants_for_connector(&state.db, connector.id).await?;
    let direct_user_ids: Vec<Uuid> = grants
        .iter()
        .filter(|g| g.grant_type == "user")
        .filter_map(|g| Uuid::parse_str(&g.grantee_id).ok())
        .collect();
    let labels = repo::resolve_user_labels(&state.db, &direct_user_ids).await?;
    let users_json: Vec<Value> = grants
        .iter()
        .filter(|g| g.grant_type == "user")
        .filter_map(|g| {
            let user_id = Uuid::parse_str(&g.grantee_id).ok()?;
            let (username, display_name) = labels.get(&user_id)?;
            Some(json!({
                "user_id": user_id,
                "username": username,
                "display_name": display_name,
                "granted_by": g.granted_by,
                "created_at": g.created_at,
            }))
        })
        .collect();

    let (teams, departments) = state
        .authorizer
        .list_org_grant_consumers(&state.db, connector.id)
        .await?;

    Ok(
        json!({ "agents": agents_json, "users": users_json, "teams": teams, "departments": departments }),
    )
}

/// `POST /api/mcp/connectors/{id}/pin` — pin a connector for quick access.
/// Requires the connector be reachable (Layer 1) — pinning something you
/// can't use would be a pointless, confusing shortlist entry.
pub async fn pin_connector_view(state: &McpState, user_id: Uuid, connector_id: Uuid) -> Result<()> {
    if !state
        .authorizer
        .can_access_connector(&state.db, user_id, connector_id)
        .await?
    {
        return Err(McpError::NotFound(format!(
            "connector '{connector_id}' not found"
        )));
    }
    repo::pin_connector(&state.db, user_id, connector_id).await
}

/// `DELETE /api/mcp/connectors/{id}/pin` — unpin.
pub async fn unpin_connector_view(
    state: &McpState,
    user_id: Uuid,
    connector_id: Uuid,
) -> Result<()> {
    repo::unpin_connector(&state.db, user_id, connector_id).await?;
    Ok(())
}

/// `GET /api/mcp/connectors/pinned` — the caller's pinned connectors, filtered
/// to ones still reachable (a stale pin on a since-revoked connector must not
/// leak it), most recently pinned first.
pub async fn list_pinned_view(state: &McpState, user_id: Uuid) -> Result<Value> {
    let pinned_ids = repo::list_pinned_connector_ids(&state.db, user_id).await?;
    Ok(json!({ "connectors": connectors_reachable_in_order(state, user_id, &pinned_ids).await? }))
}

/// `GET /api/mcp/connectors/recent` — the caller's recently-used connectors
/// (derived from real connect activity, not a page-view tracker), filtered to
/// ones still reachable, most recent first.
pub async fn list_recent_view(state: &McpState, user_id: Uuid) -> Result<Value> {
    let recent_ids = repo::list_recent_connector_ids(&state.db, user_id, 10).await?;
    Ok(json!({ "connectors": connectors_reachable_in_order(state, user_id, &recent_ids).await? }))
}

/// Resolve `ids` to full connector DTOs, preserving order, dropping any the
/// caller can no longer reach.
async fn connectors_reachable_in_order(
    state: &McpState,
    user_id: Uuid,
    ids: &[Uuid],
) -> Result<Vec<Value>> {
    let mut out = Vec::with_capacity(ids.len());
    for &id in ids {
        if !state
            .authorizer
            .can_access_connector(&state.db, user_id, id)
            .await?
        {
            continue;
        }
        if let Some(c) = repo::get_connector_by_id(&state.db, id).await? {
            let mut dto = connector_dto(&c);
            dto["is_owner"] = json!(c.owner_id == Some(user_id));
            out.push(dto);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_response_cases() {
        assert_eq!(
            classify_response(StatusCode::OK, ""),
            DetectedAuthType::None
        );
        assert_eq!(
            classify_response(StatusCode::CREATED, "Bearer"),
            DetectedAuthType::None
        );
        assert_eq!(
            classify_response(StatusCode::UNAUTHORIZED, "Bearer resource_metadata=\"x\""),
            DetectedAuthType::OAuth2
        );
        assert_eq!(
            classify_response(StatusCode::UNAUTHORIZED, ""),
            DetectedAuthType::Bearer
        );
        assert_eq!(
            classify_response(StatusCode::INTERNAL_SERVER_ERROR, ""),
            DetectedAuthType::Bearer
        );
        assert_eq!(
            classify_response(StatusCode::NOT_FOUND, ""),
            DetectedAuthType::Bearer
        );
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

    /// A spec-compliant no-auth Streamable HTTP server (e.g. mcp.deepwiki.com)
    /// 406s a bare `initialize` unless the client sends this — without it,
    /// probe misclassified every such server as requiring a bearer token,
    /// for a reason unrelated to authentication.
    #[tokio::test]
    async fn probe_initialize_sends_the_streamable_http_accept_header() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/mcp")
            .match_header("accept", "application/json, text/event-stream")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"jsonrpc":"2.0","id":1,"result":{}}"#)
            .create_async()
            .await;

        let client = reqwest::Client::new();
        let (detected, status) = probe_initialize(&client, &format!("{}/mcp", server.url()))
            .await
            .unwrap();

        mock.assert_async().await;
        assert_eq!(detected, DetectedAuthType::None);
        assert_eq!(status, 200);
    }
}

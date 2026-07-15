//! Unified connect / disconnect + connection listing, and the Composio OAuth
//! browser callback — pure logic behind `/api/mcp/connect*` and `/oauth/callback`.
//!
//! One entry point ([`connect_service`]) handles every connector: Composio
//! (OAuth), custom bearer/basic/url_param (store credential), custom OAuth 2.1
//! (return authorization_url), and no-auth (immediately connected).

use std::collections::HashMap;

use serde_json::{Value, json};
use uuid::Uuid;

use crate::error::{McpError, Result};
use crate::oauth::CallbackOutcome;
use crate::provider::normalize_connection_status;
use crate::repo::{self, McpConnector};
use crate::state::McpState;
use crate::{connectors, credentials, oauth, session};

/// Inputs for [`connect_service`] — one of `connector_id` / `service` / `url`.
#[derive(Default)]
pub struct ConnectInput {
    /// An existing connector id.
    pub connector_id: Option<Uuid>,
    /// A service name — a Composio toolkit or one of the caller's custom connectors.
    pub service: Option<String>,
    /// A custom MCP server URL to auto-register (owned by the caller).
    pub url: Option<String>,
    pub credential_value: Option<String>,
    pub redirect_url: Option<String>,
}

/// Outcome of [`connect_service`] for the server to shape into an HTTP response.
pub enum ConnectOutcome {
    Connected { connector_id: Uuid, name: String },
    Initiated { connector_id: Uuid, name: String, oauth_url: Option<String> },
    OAuthRequired { connector_id: Uuid, name: String, authorization_url: String },
}

/// `POST /api/mcp/connect` — connect any connector type.
pub async fn connect_service(state: &McpState, user_id: Uuid, input: ConnectInput) -> Result<ConnectOutcome> {
    let connector = resolve_target(state, user_id, &input).await?;

    if !state.authorizer.can_access_connector(&state.db, user_id, connector.id).await? {
        return Err(McpError::Forbidden("you do not have access to this connector".into()));
    }

    if connector.is_composio() {
        return composio_connect(state, user_id, &connector, input.redirect_url.as_deref()).await;
    }

    match connector.auth_type.as_deref().unwrap_or("none") {
        "none" => Ok(ConnectOutcome::Connected { connector_id: connector.id, name: connector.name }),
        "bearer" | "basic" | "url_param" => {
            let value = input
                .credential_value
                .as_deref()
                .ok_or_else(|| McpError::BadRequest(format!("'{}' requires credentials.value", connector.name)))?;
            credentials::register_credential(state, user_id, &connector, value).await?;
            Ok(ConnectOutcome::Connected { connector_id: connector.id, name: connector.name })
        }
        "oauth2" => {
            let url = oauth::begin_authorization(state, user_id, connector.clone(), input.redirect_url, None).await?;
            Ok(ConnectOutcome::OAuthRequired { connector_id: connector.id, name: connector.name, authorization_url: url })
        }
        other => Err(McpError::BadRequest(format!("unsupported auth_type '{other}'"))),
    }
}

/// Resolve the connector being connected: by id, by service name, or auto-register a URL.
async fn resolve_target(state: &McpState, user_id: Uuid, input: &ConnectInput) -> Result<McpConnector> {
    if let Some(id) = input.connector_id {
        return repo::get_connector_by_id(&state.db, id)
            .await?
            .ok_or_else(|| McpError::NotFound(format!("connector '{id}' not found")));
    }

    if let Some(service) = input.service.as_deref().filter(|s| !s.is_empty()) {
        let lower = service.to_lowercase();
        if let Some(c) = repo::get_composio_connector_by_name(&state.db, &lower).await? {
            return Ok(c);
        }
        if let Some(c) = repo::get_owned_connector_by_name(&state.db, user_id, service).await? {
            return Ok(c);
        }
        // Fall through to URL auto-register if a url was also supplied.
    }

    if let Some(url) = input.url.as_deref() {
        crate::net::validate_public_url(url).await?;
        // Guarded client (SSRF/DNS-rebinding): same reasoning as the /probe route.
        let detected = match connectors::probe_initialize(&state.guarded_http_client, url).await {
            Ok((d, _)) => d.as_str().to_string(),
            Err(_) => "none".to_string(),
        };
        let name = input
            .service
            .clone()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| url.rsplit("//").next().and_then(|h| h.split('/').next()).unwrap_or(url).to_string());
        return connectors::register_connector(
            state,
            user_id,
            connectors::NewConnectorInput {
                name,
                url: url.to_string(),
                transport: "streamable_http".into(),
                auth_type: detected,
                url_param_name: None,
                credential_header_name: None,
                headers: None,
                basic_username: None,
                basic_password: None,
                description: None,
                display_name: None,
                logo_url: None,
            },
        )
        .await;
    }

    Err(McpError::BadRequest("one of 'connector_id', 'service', or 'url' is required".into()))
}

/// Composio OAuth connect: reuse an active/pending connection or initiate a new one.
async fn composio_connect(
    state: &McpState,
    user_id: Uuid,
    connector: &McpConnector,
    redirect_url: Option<&str>,
) -> Result<ConnectOutcome> {
    let auth_config_id = connector
        .auth_config_id
        .as_deref()
        .ok_or_else(|| McpError::Internal("composio connector missing auth_config_id".into()))?;

    if let Some(existing) = repo::get_user_connection(&state.db, user_id, connector.id).await? {
        match existing.status.as_str() {
            "ACTIVE" => return Ok(ConnectOutcome::Connected { connector_id: connector.id, name: connector.name.clone() }),
            "INITIATED" => {
                return Ok(ConnectOutcome::Initiated {
                    connector_id: connector.id,
                    name: connector.name.clone(),
                    oauth_url: existing.oauth_url,
                });
            }
            _ => {}
        }
    }

    let provider = state.providers.require_composio()?;
    let callback_url = composio_callback_url(state, user_id, connector.id, redirect_url);
    let initiated = provider
        .initiate_connection(&user_id.to_string(), auth_config_id, callback_url.as_deref())
        .await?;
    let oauth_url = initiated
        .redirect_url
        .ok_or_else(|| McpError::Composio("Composio did not return an OAuth URL".into()))?;

    let connection =
        repo::upsert_composio_connection(&state.db, user_id, connector.id, Some(&oauth_url), callback_url.as_deref())
            .await?;

    tracing::info!(%user_id, connector = %connector.name, "composio oauth initiated");
    Ok(ConnectOutcome::Initiated { connector_id: connector.id, name: connector.name.clone(), oauth_url: connection.oauth_url })
}

/// `GET /api/mcp/connections` — the caller's connections, syncing pending ones.
pub async fn list_connections_view(state: &McpState, user_id: Uuid) -> Result<Value> {
    // Map connector id → connector for names / auth_config_id.
    let connectors: HashMap<Uuid, McpConnector> = state.authorizer.list_accessible_connectors(&state.db, user_id)
        .await?
        .into_iter()
        .map(|c| (c.id, c))
        .collect();

    // Best-effort sync of pending composio connections.
    if let Some(provider) = &state.providers.composio {
        for conn in repo::list_user_connections(&state.db, user_id, None).await? {
            if conn.status != "INITIATED" {
                continue;
            }
            let Some(connector) = connectors.get(&conn.connector_id) else { continue };
            let Some(auth_config_id) = connector.auth_config_id.as_deref() else { continue };
            if let Ok(check) = provider.check_connection_status(&user_id.to_string(), auth_config_id).await
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

    let fresh = repo::list_user_connections(&state.db, user_id, None).await?;
    let data: Vec<Value> = fresh
        .iter()
        .map(|c| {
            let name = connectors.get(&c.connector_id).map(|k| k.name.clone());
            json!({
                "connector_id": c.connector_id,
                "name": name,
                "status": c.status,
                "connected_account_id": c.connected_account_id,
                "oauth_url": c.oauth_url,
                "created_at": c.created_at,
            })
        })
        .collect();
    let total = data.len();
    Ok(json!({ "data": data, "total": total }))
}

/// Outcome of [`disconnect`].
pub struct DisconnectOutcome {
    pub message: String,
    pub connector_id: Uuid,
    pub composio_revoked: bool,
}

/// `DELETE /api/mcp/connections/{connector_id}` — disconnect the caller's connection.
pub async fn disconnect(state: &McpState, user_id: Uuid, connector_id: Uuid) -> Result<DisconnectOutcome> {
    let connection = repo::get_user_connection(&state.db, user_id, connector_id)
        .await?
        .ok_or_else(|| McpError::NotFound("no connection for this connector".into()))?;

    // Best-effort Composio token revoke.
    let mut composio_revoked = false;
    if let (Some(provider), Some(account_id)) =
        (&state.providers.composio, connection.connected_account_id.as_deref())
    {
        composio_revoked = provider.revoke_connection(account_id).await.unwrap_or(false);
    }

    repo::delete_user_connection(&state.db, user_id, connector_id).await?;
    session::invalidate_session_cache(state, user_id).await;

    tracing::info!(%user_id, %connector_id, composio_revoked, "disconnected connector");
    Ok(DisconnectOutcome { message: "Disconnected.".to_string(), connector_id, composio_revoked })
}

/// Build our `/oauth/callback` URL carrying the user + connector for verification.
fn composio_callback_url(state: &McpState, user_id: Uuid, connector_id: Uuid, success_url: Option<&str>) -> Option<String> {
    let base = state.config.gateway_public_url.as_ref()?;
    let mut origin = reqwest::Url::parse(base).ok()?;
    origin.set_path("/oauth/callback");
    origin.set_query(None);
    origin
        .query_pairs_mut()
        .append_pair("user_id", &user_id.to_string())
        .append_pair("connector_id", &connector_id.to_string());
    if let Some(s) = success_url {
        origin.query_pairs_mut().append_pair("success_url", s);
    }
    Some(origin.to_string())
}

/// `GET /oauth/callback` core (Composio redirect target): verify ACTIVE, record
/// the account id, invalidate the session cache, report where to redirect.
pub async fn handle_composio_callback(
    state: &McpState,
    user_id: Option<Uuid>,
    connector_id: Option<Uuid>,
    success_url: Option<String>,
) -> CallbackOutcome {
    let (Some(user_id), Some(connector_id)) = (user_id, connector_id) else {
        return CallbackOutcome::Message("Missing user_id or connector_id.".to_string());
    };

    let connection = match repo::get_user_connection(&state.db, user_id, connector_id).await {
        Ok(Some(c)) if c.status != "EXPIRED" => c,
        Ok(_) => return CallbackOutcome::Message("No pending connection for this connector.".to_string()),
        Err(e) => return CallbackOutcome::Message(format!("Lookup failed: {e}")),
    };

    let connector = match repo::get_connector_by_id(&state.db, connector_id).await {
        Ok(Some(c)) => c,
        _ => return CallbackOutcome::Message("Connector not found.".to_string()),
    };
    let Some(auth_config_id) = connector.auth_config_id.as_deref() else {
        return CallbackOutcome::Message("Connector is not a Composio connector.".to_string());
    };
    let Some(provider) = &state.providers.composio else {
        return CallbackOutcome::Message("Composio is not configured.".to_string());
    };

    match provider.check_connection_status(&user_id.to_string(), auth_config_id).await {
        Ok(check) if check.status.eq_ignore_ascii_case("ACTIVE") => {
            let _ = repo::update_connection_status(&state.db, connection.id, "ACTIVE").await;
            if let Some(account_id) = check.account_id.as_deref() {
                let _ = repo::update_connection_account_id(&state.db, connection.id, account_id).await;
            }
            session::invalidate_session_cache(state, user_id).await;
            // success_url is an unauthenticated query param — never redirect off-origin.
            let dest = success_url.unwrap_or_else(|| "/".to_string());
            CallbackOutcome::Redirect(crate::net::safe_redirect(&dest, state.config.gateway_public_url.as_deref()))
        }
        _ => CallbackOutcome::Message("Authorization is still finalizing — refresh in a moment.".to_string()),
    }
}

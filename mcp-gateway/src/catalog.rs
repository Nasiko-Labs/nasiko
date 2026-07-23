//! Service catalog + platform Composio connector registration — pure logic
//! behind `/api/mcp/catalog` and `/api/mcp/auth-configs`.

use serde_json::{Value, json};
use uuid::Uuid;

use crate::cache;
use crate::error::{McpError, Result};
use crate::repo::{self, NewConnector};
use crate::state::McpState;

/// Capitalize the first character (default display names).
pub fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

fn auth_flow_for(connector: &repo::McpConnector) -> &'static str {
    if connector.is_composio() {
        return "oauth";
    }
    match connector.auth_type.as_deref() {
        Some("oauth2") => "oauth",
        Some("bearer") | Some("basic") | Some("url_param") => "api_key",
        _ => "none",
    }
}

/// `GET /api/mcp/catalog` — connectable services (composio ∪ accessible custom).
pub async fn get_catalog_view(state: &McpState, user_id: Uuid) -> Result<Value> {
    let connectors = state
        .authorizer
        .list_accessible_connectors(&state.db, user_id)
        .await?;
    let mut services: Vec<Value> = Vec::with_capacity(connectors.len());
    for c in &connectors {
        // Only Composio toolkits can report a tool count without a live
        // connection (the platform API key can list them directly). A generic
        // `mcp_server` connector genuinely requires connecting first to
        // discover its tools — `null` here means "unknown until connected",
        // not zero.
        let tool_count = if c.is_composio() {
            composio_tool_count(state, c.id, &c.name).await
        } else {
            None
        };
        services.push(json!({
            "connector_id": c.id,
            "name": c.name,
            "type": c.provider_type,
            "display_name": c.display_name.clone().unwrap_or_else(|| capitalize(&c.name)),
            "description": c.description,
            "logo_url": c.logo_url,
            "auth_flow": auth_flow_for(c),
            "tool_count": tool_count,
        }));
    }
    Ok(json!({ "services": services }))
}

/// Cached tool count for a Composio toolkit. `None` if Composio isn't
/// configured or the lookup fails — never fabricated.
async fn composio_tool_count(state: &McpState, connector_id: Uuid, toolkit: &str) -> Option<usize> {
    let key = format!("mcp:toolcount:{connector_id}");
    if let Some(n) = cache::get_json::<usize>(&state.redis, &key).await {
        return Some(n);
    }
    let provider = state.providers.composio.as_ref()?;
    let count = provider.list_toolkit_tools(toolkit).await.ok()?.len();
    cache::set_json_ex(
        &state.redis,
        &key,
        &count,
        state.config.toolcount_ttl_seconds,
    )
    .await;
    Some(count)
}

/// Inputs for registering a platform Composio connector.
pub struct CreateComposioInput<'a> {
    pub toolkit: &'a str,
    pub use_composio_managed: bool,
    pub client_id: Option<&'a str>,
    pub client_secret: Option<&'a str>,
    pub scopes: Option<&'a [String]>,
    pub display_name: Option<&'a str>,
    pub logo_url: Option<&'a str>,
}

/// Register a platform Composio connector: collision check, register the OAuth
/// app with Composio, record it as a `provider_type='composio'` connector.
pub async fn create_composio_connector(
    state: &McpState,
    input: CreateComposioInput<'_>,
) -> Result<Value> {
    let toolkit = input.toolkit.to_lowercase();
    if repo::get_composio_connector_by_name(&state.db, &toolkit)
        .await?
        .is_some()
    {
        return Err(McpError::Conflict(format!(
            "composio connector '{toolkit}' already exists"
        )));
    }

    let provider = state.providers.require_composio()?;
    let created = provider
        .create_auth_config(
            &toolkit,
            input.use_composio_managed,
            input.client_id,
            input.client_secret,
            input.scopes,
        )
        .await?;

    let connector = repo::create_connector(
        &state.db,
        &NewConnector {
            provider_type: "composio".to_string(),
            owner_id: None,
            name: toolkit.clone(),
            display_name: input.display_name.map(str::to_string),
            logo_url: input.logo_url.map(str::to_string),
            auth_config_id: Some(created.auth_config_id),
            auth_scheme: Some("OAUTH2".to_string()),
            use_composio_managed: Some(input.use_composio_managed),
            ..Default::default()
        },
    )
    .await?;

    tracing::info!(toolkit = %toolkit, connector_id = %connector.id, "registered platform composio connector");
    Ok(json!({
        "connector_id": connector.id,
        "toolkit": connector.name,
        "auth_config_id": connector.auth_config_id,
    }))
}

/// Editable display metadata for a composio connector.
#[derive(Default)]
pub struct ComposioMetadata {
    pub display_name: Option<String>,
    pub logo_url: Option<String>,
    pub description: Option<String>,
}

/// Update a composio connector's catalog metadata (display_name / logo / description).
/// Does NOT touch the Composio `auth_config_id`, so connected users keep working.
pub async fn update_composio_metadata(
    state: &McpState,
    connector_id: Uuid,
    meta: ComposioMetadata,
) -> Result<Value> {
    let connector = repo::get_connector_by_id(&state.db, connector_id).await?;
    match connector {
        Some(c) if c.is_composio() => {
            let updated = repo::update_connector(
                &state.db,
                connector_id,
                &repo::UpdateConnector {
                    display_name: meta.display_name,
                    logo_url: meta.logo_url,
                    description: meta.description,
                    ..Default::default()
                },
            )
            .await?;
            Ok(json!({
                "connector_id": updated.id,
                "toolkit": updated.name,
                "display_name": updated.display_name,
                "logo_url": updated.logo_url,
            }))
        }
        _ => Err(McpError::NotFound(format!(
            "composio connector '{connector_id}' not found"
        ))),
    }
}

/// `GET /api/mcp/auth-configs` — list platform Composio connectors.
pub async fn list_composio_connectors_view(state: &McpState) -> Result<Value> {
    let connectors = repo::list_composio_connectors(&state.db).await?;
    let data: Vec<Value> = connectors
        .into_iter()
        .map(|c| {
            json!({
                "connector_id": c.id,
                "toolkit": c.name,
                "auth_config_id": c.auth_config_id,
                "display_name": c.display_name,
                "logo_url": c.logo_url,
            })
        })
        .collect();
    let total = data.len();
    Ok(json!({ "connectors": data, "total": total }))
}

/// Delete a platform Composio connector by id.
pub async fn delete_composio_connector(state: &McpState, connector_id: Uuid) -> Result<()> {
    let connector = repo::get_connector_by_id(&state.db, connector_id).await?;
    match connector {
        Some(c) if c.is_composio() => {
            repo::delete_connector(&state.db, connector_id).await?;
            Ok(())
        }
        _ => Err(McpError::NotFound(format!(
            "composio connector '{connector_id}' not found"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::McpConfig;
    use crate::provider::composio::ComposioProvider;
    use crate::provider::{GenericMcpProvider, Providers};

    #[test]
    fn capitalize_cases() {
        assert_eq!(capitalize("gmail"), "Gmail");
        assert_eq!(capitalize("Slack"), "Slack");
        assert_eq!(capitalize(""), "");
        assert_eq!(capitalize("x"), "X");
        assert_eq!(capitalize("gitHub"), "GitHub");
        assert_eq!(capitalize("émoji"), "Émoji");
        assert_eq!(capitalize("123abc"), "123abc");
    }

    /// `db` is a *lazy* pool (never actually connects) — fine here since
    /// `composio_tool_count` never touches it; only `redis` (degrades
    /// gracefully to a cache-miss) and `providers.composio`.
    fn test_state(composio: Option<Arc<dyn crate::provider::ToolProvider>>) -> McpState {
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
                session_ttl_seconds: 60,
                perm_cache_ttl_seconds: 60,
                manifest_ttl_seconds: 60,
                toolcount_ttl_seconds: 3600,
                oauth_state_signing_key: "test".to_string(),
            },
            providers: Providers {
                composio,
                mcp: GenericMcpProvider::new(reqwest::Client::new(), reqwest::Client::new()),
            },
            authorizer: Arc::new(crate::authorizer::OssConnectorAuthorizer),
            endpoint_refresher: Arc::new(crate::endpoint_refresh::NoopEndpointRefresher),
        }
    }

    #[tokio::test]
    async fn composio_tool_count_none_when_composio_not_configured() {
        let state = test_state(None);
        assert_eq!(
            composio_tool_count(&state, Uuid::new_v4(), "gmail").await,
            None
        );
    }

    #[tokio::test]
    async fn composio_tool_count_counts_live_tools() {
        let mut srv = mockito::Server::new_async().await;
        srv.mock("GET", mockito::Matcher::Regex("^/api/v3/tools".into()))
            .with_status(200)
            .with_body(r#"{"items":[{"slug":"GMAIL_SEND","description":"send"},{"slug":"GMAIL_READ","description":"read"}]}"#)
            .create_async()
            .await;
        let provider: Arc<dyn crate::provider::ToolProvider> = Arc::new(ComposioProvider::new(
            reqwest::Client::new(),
            "ak_test".into(),
            srv.url(),
        ));
        let state = test_state(Some(provider));
        assert_eq!(
            composio_tool_count(&state, Uuid::new_v4(), "gmail").await,
            Some(2)
        );
    }
}

//! Service catalog + platform Composio auth-config (toolkit) registration —
//! pure logic behind the server's `/api/mcp/catalog` and `/api/mcp/auth-configs`
//! routes.

use serde_json::{Value, json};

use crate::error::{McpError, Result};
use crate::repo;
use crate::state::McpState;

/// Capitalize the first character (default catalog display names).
pub fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// `GET /api/mcp/catalog` view: connectable services, credential-free.
pub async fn get_catalog_view(state: &McpState) -> Result<Value> {
    let configs = repo::list_platform_auth_configs(&state.db).await?;
    let servers = repo::list_platform_mcp_servers(&state.db).await?;

    let mut services: Vec<Value> = Vec::new();
    for ac in configs {
        services.push(json!({
            "name": ac.toolkit,
            "type": "composio",
            "display_name": ac.display_name.unwrap_or_else(|| capitalize(&ac.toolkit)),
            "description": Value::Null,
            "logo_url": ac.logo_url,
            "auth_flow": "oauth",
        }));
    }
    for s in servers {
        let auth_flow = match s.auth_type.as_str() {
            "oauth2" => "oauth",
            "bearer" | "basic" | "url_param" => "api_key",
            _ => "none",
        };
        services.push(json!({
            "name": s.name,
            "type": "mcp",
            "display_name": s.display_name.unwrap_or_else(|| capitalize(&s.name)),
            "description": s.description,
            "logo_url": s.logo_url,
            "auth_flow": auth_flow,
        }));
    }

    Ok(json!({ "services": services }))
}

/// Inputs for registering a new platform Composio toolkit.
pub struct CreateAuthConfigInput<'a> {
    pub toolkit: &'a str,
    pub use_composio_managed: bool,
    pub client_id: Option<&'a str>,
    pub client_secret: Option<&'a str>,
    pub scopes: Option<&'a [String]>,
    pub display_name: Option<&'a str>,
    pub logo_url: Option<&'a str>,
}

/// Register a platform Composio toolkit: duplicate/collision checks, register
/// the OAuth app with Composio, then record it locally. Returns the created
/// row's JSON view (`auth_config_id`, `toolkit`, `is_platform`).
pub async fn create_platform_auth_config(
    state: &McpState,
    input: CreateAuthConfigInput<'_>,
) -> Result<Value> {
    let toolkit = input.toolkit.to_lowercase();
    if repo::get_platform_auth_config_by_toolkit(&state.db, &toolkit).await?.is_some() {
        return Err(McpError::Conflict(format!(
            "platform auth config for '{toolkit}' already exists"
        )));
    }
    // Guard against a toolkit name colliding with a platform MCP server name —
    // per-agent permission rows key on server_name alone, so a collision would
    // make one toggle control both. (Same guard as the PoC.)
    if repo::get_platform_mcp_server_by_name(&state.db, &toolkit).await?.is_some() {
        return Err(McpError::Conflict(format!(
            "'{toolkit}' is already a platform MCP server — choose a different name"
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

    let row = repo::create_auth_config(
        &state.db,
        &created.auth_config_id,
        None, // platform
        &toolkit,
        input.use_composio_managed,
        true, // is_platform
        input.display_name,
        input.logo_url,
    )
    .await?;

    tracing::info!(toolkit = %toolkit, auth_config_id = %row.auth_config_id, "registered platform composio toolkit");
    Ok(json!({
        "auth_config_id": row.auth_config_id,
        "toolkit": row.toolkit,
        "is_platform": row.is_platform,
    }))
}

/// `GET /api/mcp/auth-configs` view: list platform toolkits.
pub async fn list_auth_configs_view(state: &McpState) -> Result<Value> {
    let configs = repo::list_platform_auth_configs(&state.db).await?;
    let out: Vec<Value> = configs
        .into_iter()
        .map(|ac| {
            json!({
                "auth_config_id": ac.auth_config_id,
                "toolkit": ac.toolkit,
                "is_platform": ac.is_platform,
                "display_name": ac.display_name,
                "logo_url": ac.logo_url,
            })
        })
        .collect();
    let total = out.len();
    Ok(json!({ "data": out, "total": total }))
}

/// Delete a platform auth config. Errors `NotFound` if it doesn't exist.
pub async fn delete_auth_config(state: &McpState, auth_config_id: &str) -> Result<()> {
    if !repo::delete_auth_config(&state.db, auth_config_id).await? {
        return Err(McpError::NotFound(format!("auth config '{auth_config_id}' not found")));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::capitalize;

    #[test]
    fn capitalize_lowercase_ascii() {
        assert_eq!(capitalize("gmail"), "Gmail");
    }

    #[test]
    fn capitalize_already_uppercase_is_unchanged() {
        assert_eq!(capitalize("Slack"), "Slack");
    }

    #[test]
    fn capitalize_empty_string_returns_empty() {
        assert_eq!(capitalize(""), "");
    }

    #[test]
    fn capitalize_single_char() {
        assert_eq!(capitalize("x"), "X");
    }

    #[test]
    fn capitalize_only_uppercases_first_char() {
        assert_eq!(capitalize("gitHub"), "GitHub");
    }

    #[test]
    fn capitalize_multibyte_first_char() {
        // 'é' uppercases to 'É' — exercises the char (not byte) boundary.
        assert_eq!(capitalize("émoji"), "Émoji");
    }

    #[test]
    fn capitalize_numeric_first_char_is_unchanged() {
        assert_eq!(capitalize("123abc"), "123abc");
    }
}

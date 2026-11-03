//! Boot-time seeding of remote MCP connectors from `SEED_CONNECTORS`.
//!
//! Each entry is registered as a connector owned by the first superuser and
//! shared publicly, so every user sees it without registering it themselves.
//! Seeding is idempotent: a connector the owner already has by that name is
//! not re-registered (its stored config is left alone), but its public share
//! is re-asserted every boot. A failing entry is logged and skipped — a third-party MCP
//! server being down must never block boot.

use std::collections::HashMap;

use serde::Deserialize;
use tracing::{info, warn};
use uuid::Uuid;

use crate::state::AppState;
use nasiko_mcp_gateway::connectors::{self, NewConnectorInput, ShareTarget};

/// One connector to seed. Field names match the register-connector API body.
#[derive(Debug, Deserialize, PartialEq)]
pub struct SeedConnectorSpec {
    pub name: String,
    pub url: String,
    #[serde(default = "default_auth_type")]
    pub auth_type: String,
    #[serde(default = "default_transport")]
    pub transport: String,
    #[serde(default)]
    pub headers: Option<HashMap<String, String>>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub logo_url: Option<String>,
}

fn default_auth_type() -> String {
    "none".to_string()
}

fn default_transport() -> String {
    "streamable_http".to_string()
}

/// Parse the `SEED_CONNECTORS` JSON array. Blank means nothing to seed;
/// invalid JSON is logged and treated as nothing to seed, never a boot failure.
/// The error is logged without the input, which carries API keys.
pub fn parse_seed_connectors(raw: &str) -> Vec<SeedConnectorSpec> {
    if raw.trim().is_empty() {
        return Vec::new();
    }
    match serde_json::from_str(raw) {
        Ok(specs) => specs,
        Err(e) => {
            warn!(error = %e, "SEED_CONNECTORS is not a valid JSON array of connectors, skipping connector seeding");
            Vec::new()
        }
    }
}

/// Register and publicly share every connector in `SEED_CONNECTORS`.
/// Designed to run as a background task — does not block server startup.
pub async fn seed_connectors_if_configured(state: &AppState) {
    let specs = parse_seed_connectors(&state.config.seed_connectors);
    if specs.is_empty() {
        info!("SEED_CONNECTORS not set, skipping connector seeding");
        return;
    }
    let Some(owner_id) = first_superuser(state).await else {
        warn!("no admin user found, cannot seed connectors (run bootstrap first)");
        return;
    };
    info!(
        count = specs.len(),
        "SEED_CONNECTORS configured, registering"
    );
    for spec in specs {
        seed_one(state, owner_id, spec).await;
    }
}

async fn first_superuser(state: &AppState) -> Option<Uuid> {
    sqlx::query_scalar(
        "SELECT id FROM users WHERE is_superuser = true AND deleted_at IS NULL ORDER BY created_at LIMIT 1",
    )
    .fetch_optional(&state.db)
    .await
    .ok()
    .flatten()
}

/// Register the connector unless it already exists, then make sure it is
/// shared publicly. The share runs for an existing connector too — it is an
/// upsert — so a connector whose share failed, or whose boot died between
/// register and share, is repaired on the next boot instead of staying
/// owner-only forever.
async fn seed_one(state: &AppState, owner_id: Uuid, spec: SeedConnectorSpec) {
    let name = spec.name.clone();
    let Some(connector_id) = find_or_register(state, owner_id, spec).await else {
        return;
    };
    match connectors::share_connector(
        &state.mcp,
        owner_id,
        true,
        connector_id,
        ShareTarget::Public,
    )
    .await
    {
        Ok(_) => info!(connector = %name, "seed connector registered and shared publicly"),
        Err(e) => {
            warn!(connector = %name, %e, "seed connector registered but sharing failed, retrying next boot")
        }
    }
}

async fn find_or_register(
    state: &AppState,
    owner_id: Uuid,
    spec: SeedConnectorSpec,
) -> Option<Uuid> {
    let name = spec.name.clone();
    match nasiko_mcp_gateway::repo::get_owned_connector_by_name(&state.db, owner_id, &name).await {
        Ok(Some(existing)) => {
            info!(connector = %name, "connector already registered, not re-registering");
            return Some(existing.id);
        }
        Ok(None) => {}
        Err(e) => {
            warn!(connector = %name, %e, "failed to look up seed connector, skipping");
            return None;
        }
    }
    match connectors::register_connector(&state.mcp, owner_id, into_input(spec)).await {
        Ok(c) => Some(c.id),
        Err(e) => {
            warn!(connector = %name, %e, "failed to seed connector");
            None
        }
    }
}

fn into_input(spec: SeedConnectorSpec) -> NewConnectorInput {
    NewConnectorInput {
        name: spec.name,
        url: spec.url,
        transport: spec.transport,
        auth_type: spec.auth_type,
        url_param_name: None,
        credential_header_name: None,
        headers: spec.headers,
        basic_username: None,
        basic_password: None,
        description: spec.description,
        display_name: spec.display_name,
        logo_url: spec.logo_url,
        oauth_client_id: None,
        oauth_client_secret: None,
        oauth_scopes: None,
        oauth_send_resource: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blank_input_seeds_nothing() {
        assert!(parse_seed_connectors("").is_empty());
        assert!(parse_seed_connectors("   ").is_empty());
    }

    #[test]
    fn defaults_apply_when_only_name_and_url_are_given() {
        let specs =
            parse_seed_connectors(r#"[{"name":"docs","url":"https://mcp.example.com/mcp"}]"#);

        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].auth_type, "none");
        assert_eq!(specs[0].transport, "streamable_http");
        assert_eq!(specs[0].headers, None);
        assert_eq!(specs[0].description, None);
    }

    #[test]
    fn all_fields_are_read() {
        let raw = r#"[
            {"name":"search","url":"https://a.example.com/mcp",
             "headers":{"Authorization":"Bearer k"},"description":"d",
             "display_name":"Search","logo_url":"https://a.example.com/l.svg"},
            {"name":"crm","url":"https://b.example.com/mcp","auth_type":"oauth2","transport":"sse"}
        ]"#;

        let specs = parse_seed_connectors(raw);

        assert_eq!(specs.len(), 2);
        assert_eq!(
            specs[0]
                .headers
                .as_ref()
                .and_then(|h| h.get("Authorization")),
            Some(&"Bearer k".to_string())
        );
        assert_eq!(specs[0].display_name.as_deref(), Some("Search"));
        assert_eq!(
            specs[0].logo_url.as_deref(),
            Some("https://a.example.com/l.svg")
        );
        assert_eq!(specs[1].auth_type, "oauth2");
        assert_eq!(specs[1].transport, "sse");
    }

    #[test]
    fn invalid_json_seeds_nothing() {
        assert!(parse_seed_connectors("not json").is_empty());
        assert!(parse_seed_connectors(r#"{"name":"x","url":"y"}"#).is_empty());
        assert!(parse_seed_connectors(r#"[{"name":"missing-url"}]"#).is_empty());
    }

    #[test]
    fn into_input_carries_the_spec_through() {
        let spec =
            parse_seed_connectors(r#"[{"name":"n","url":"https://u.example.com"}]"#).remove(0);

        let input = into_input(spec);

        assert_eq!(input.name, "n");
        assert_eq!(input.url, "https://u.example.com");
        assert_eq!(input.auth_type, "none");
        assert!(input.oauth_client_id.is_none());
    }
}

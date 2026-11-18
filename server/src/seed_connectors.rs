//! Boot-time seeding of remote MCP connectors from `SEED_CONNECTORS`.
//!
//! Each entry is registered as a connector owned by the first superuser and
//! shared publicly, so every user sees it without registering it themselves.
//! Seeding is idempotent: a connector the owner already has by that name is
//! not re-registered (its stored config is left alone), but its public share
//! is re-asserted and its tool catalog re-synced every boot. A failing entry
//! is logged and skipped — a third-party MCP server being down must never
//! block boot.
//!
//! A `bearer`/`basic` entry is a per-user credential connector: each user
//! enters their own key in the UI. Such an entry that also carries a static
//! `Authorization` header is refused, because the gateway falls back to static
//! headers for a user with no credential of their own — every such user would
//! silently run on the operator's key. Ship a shared key as `auth_type: none`.

use std::collections::HashMap;

use serde::Deserialize;
use tracing::{info, warn};
use uuid::Uuid;

use crate::state::AppState;
use nasiko_mcp_gateway::connectors::{self, NewConnectorInput, ShareTarget};
use nasiko_mcp_gateway::permissions;

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

/// The header seeded per-user connectors carry their credential in. Seeds
/// set no `credential_header_name`, so the gateway uses its default.
const CREDENTIAL_HEADER: &str = "Authorization";

/// The static header that would stand in for a missing per-user credential,
/// if `spec` is a per-user (`bearer`/`basic`) connector that carries one.
fn shared_credential_header(spec: &SeedConnectorSpec) -> Option<&str> {
    if !matches!(spec.auth_type.as_str(), "bearer" | "basic") {
        return None;
    }
    spec.headers
        .as_ref()?
        .keys()
        .find(|name| name.eq_ignore_ascii_case(CREDENTIAL_HEADER))
        .map(String::as_str)
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
    if let Some(header) = shared_credential_header(&spec) {
        warn!(
            connector = %name, auth_type = %spec.auth_type, %header,
            "seed connector is per-user but carries a static credential header, skipping \
             (remove the header so users enter their own key, or use auth_type none for a shared key)"
        );
        return;
    }
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
    sync_tools(state, owner_id, connector_id, &name).await;
}

/// Pull the connector's tool catalog now. Registering stores no tools, and
/// `nasiko_search_tools` only finds tools in the catalog, so without this a
/// seeded connector stays invisible to every agent until someone happens to
/// open its tools page. The sync also rebuilds the search index, and running
/// it here — in the boot task, not a request — means it cannot be cut short
/// by a client disconnecting. Runs every boot, so a sync that failed before
/// is retried. An OAuth connector with no credential yet syncs nothing.
async fn sync_tools(state: &AppState, owner_id: Uuid, connector_id: Uuid, name: &str) {
    match permissions::sync_connector_tools_by_id(&state.mcp, owner_id, connector_id).await {
        Ok(()) => info!(connector = %name, "seed connector tools synced"),
        Err(e) => {
            warn!(connector = %name, %e, "seed connector tool sync failed, retrying next boot")
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
            repair_description(state, &existing, spec.description).await;
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

/// Replace a description registration copied from the server's MCP
/// `instructions` with the seed's own.
///
/// Registration used to fall back to `instructions`, which are written for
/// the model, so a seeded Firecrawl card showed its prompt text. A
/// description still equal to the stored `instructions` was never edited by
/// anyone, so overwriting it loses nothing; any other description is left alone.
async fn repair_description(
    state: &AppState,
    existing: &nasiko_mcp_gateway::repo::McpConnector,
    seeded: Option<String>,
) {
    if !copied_from_instructions(existing) {
        return;
    }
    let Some(description) = seeded else {
        return;
    };
    let update = nasiko_mcp_gateway::repo::UpdateConnector {
        description: Some(description),
        ..Default::default()
    };
    match nasiko_mcp_gateway::repo::update_connector(&state.db, existing.id, &update).await {
        Ok(_) => info!(connector = %existing.name, "seed connector description replaced"),
        Err(e) => warn!(connector = %existing.name, %e, "seed connector description repair failed"),
    }
}

fn copied_from_instructions(c: &nasiko_mcp_gateway::repo::McpConnector) -> bool {
    c.description.is_some() && c.description == c.instructions
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

    fn spec(raw: &str) -> SeedConnectorSpec {
        parse_seed_connectors(&format!("[{raw}]")).remove(0)
    }

    #[test]
    fn per_user_connector_with_static_authorization_header_is_refused() {
        let bearer = spec(
            r#"{"name":"f","url":"https://f.example.com","auth_type":"bearer","headers":{"Authorization":"Bearer k"}}"#,
        );
        let basic = spec(
            r#"{"name":"b","url":"https://b.example.com","auth_type":"basic","headers":{"authorization":"Basic k"}}"#,
        );

        assert_eq!(shared_credential_header(&bearer), Some("Authorization"));
        assert_eq!(shared_credential_header(&basic), Some("authorization"));
    }

    #[test]
    fn per_user_connector_without_a_credential_header_is_allowed() {
        let bare = spec(r#"{"name":"f","url":"https://f.example.com","auth_type":"bearer"}"#);
        let unrelated = spec(
            r#"{"name":"f","url":"https://f.example.com","auth_type":"bearer","headers":{"X-Trace":"1"}}"#,
        );

        assert_eq!(shared_credential_header(&bare), None);
        assert_eq!(shared_credential_header(&unrelated), None);
    }

    #[test]
    fn shared_key_and_oauth_connectors_are_not_affected() {
        let shared = spec(
            r#"{"name":"f","url":"https://f.example.com","headers":{"Authorization":"Bearer k"}}"#,
        );
        let oauth = spec(
            r#"{"name":"a","url":"https://a.example.com","auth_type":"oauth2","headers":{"Authorization":"x"}}"#,
        );

        assert_eq!(shared_credential_header(&shared), None);
        assert_eq!(shared_credential_header(&oauth), None);
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

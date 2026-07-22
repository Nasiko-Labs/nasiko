//! Service layer — thin wrappers over the `nasiko-mcp-gateway` crate.
//!
//! Each function forwards already-extracted identity + plain values into the
//! crate (where all logic and SQL live) and returns its result. Handlers do
//! axum extraction + ACL; the crate does the work; this is the seam between them
//! (and what `ee/` reuses).

use serde_json::{Value, json};
use uuid::Uuid;

use nasiko_mcp_gateway::McpError;
use crate::state::AppState;

type R<T> = Result<T, McpError>;

pub mod catalog {
    use super::*;
    pub use nasiko_mcp_gateway::catalog::ComposioMetadata;
    use nasiko_mcp_gateway::catalog::{self, CreateComposioInput};

    /// Owned form of a composio registration request.
    pub struct ComposioReg {
        pub toolkit: String,
        pub use_composio_managed: bool,
        pub client_id: Option<String>,
        pub client_secret: Option<String>,
        pub scopes: Option<Vec<String>>,
        pub display_name: Option<String>,
        pub logo_url: Option<String>,
    }

    pub async fn get_catalog(s: &AppState, user: Uuid) -> R<Value> {
        catalog::get_catalog_view(&s.mcp, user).await
    }
    pub async fn create_composio(s: &AppState, r: &ComposioReg) -> R<Value> {
        catalog::create_composio_connector(
            &s.mcp,
            CreateComposioInput {
                toolkit: &r.toolkit,
                use_composio_managed: r.use_composio_managed,
                client_id: r.client_id.as_deref(),
                client_secret: r.client_secret.as_deref(),
                scopes: r.scopes.as_deref(),
                display_name: r.display_name.as_deref(),
                logo_url: r.logo_url.as_deref(),
            },
        )
        .await
    }
    pub async fn list_composio(s: &AppState) -> R<Value> {
        catalog::list_composio_connectors_view(&s.mcp).await
    }
    pub async fn delete_composio(s: &AppState, id: Uuid) -> R<()> {
        catalog::delete_composio_connector(&s.mcp, id).await
    }
    pub async fn update_composio(s: &AppState, id: Uuid, meta: ComposioMetadata) -> R<Value> {
        catalog::update_composio_metadata(&s.mcp, id, meta).await
    }
}

pub mod connect {
    use super::*;
    pub use nasiko_mcp_gateway::connect::{ConnectInput, ConnectOutcome, DisconnectOutcome};
    use nasiko_mcp_gateway::connect;
    use nasiko_mcp_gateway::oauth::CallbackOutcome;

    pub async fn connect(s: &AppState, user: Uuid, input: ConnectInput) -> R<ConnectOutcome> {
        connect::connect_service(&s.mcp, user, input).await
    }
    pub async fn list_connections(s: &AppState, user: Uuid) -> R<Value> {
        connect::list_connections_view(&s.mcp, user).await
    }
    pub async fn disconnect(s: &AppState, user: Uuid, connector_id: Uuid) -> R<DisconnectOutcome> {
        connect::disconnect(&s.mcp, user, connector_id).await
    }
    pub async fn composio_callback(
        s: &AppState,
        user: Option<Uuid>,
        connector_id: Option<Uuid>,
        success_url: Option<String>,
    ) -> CallbackOutcome {
        connect::handle_composio_callback(&s.mcp, user, connector_id, success_url).await
    }
}

pub mod connectors {
    use super::*;
    pub use nasiko_mcp_gateway::connectors::{NewConnectorInput, ShareTarget, UpdateConnectorInput};
    use nasiko_mcp_gateway::connectors;

    pub async fn list(s: &AppState, user: Uuid) -> R<Value> {
        connectors::list_connectors_view(&s.mcp, user).await
    }
    pub async fn get(s: &AppState, user: Uuid, id: Uuid) -> R<Value> {
        connectors::get_connector_view(&s.mcp, user, id).await
    }
    pub async fn create(s: &AppState, owner: Uuid, input: NewConnectorInput) -> R<Value> {
        let c = connectors::register_connector(&s.mcp, owner, input).await?;
        Ok(connectors::connector_dto(&c))
    }
    pub async fn update(s: &AppState, caller: Uuid, is_admin: bool, id: Uuid, input: UpdateConnectorInput) -> R<Value> {
        let c = connectors::update_connector(&s.mcp, caller, is_admin, id, input).await?;
        Ok(connectors::connector_dto(&c))
    }
    pub async fn probe(s: &AppState, url: &str) -> R<Value> {
        connectors::probe_connector_view(&s.mcp, url).await
    }
    pub async fn delete(s: &AppState, caller: Uuid, is_admin: bool, id: Uuid) -> R<()> {
        let c = connectors::get_connector_for_deletion(&s.mcp, id).await?;
        if !is_admin && c.owner_id != Some(caller) {
            return Err(McpError::Forbidden("this connector does not belong to you".into()));
        }
        connectors::delete_connector(&s.mcp, &c).await
    }
    pub async fn share(s: &AppState, caller: Uuid, is_admin: bool, id: Uuid, target: ShareTarget) -> R<Value> {
        connectors::share_connector(&s.mcp, caller, is_admin, id, target).await
    }
    pub async fn revoke(s: &AppState, caller: Uuid, is_admin: bool, id: Uuid, target: ShareTarget) -> R<()> {
        connectors::revoke_share(&s.mcp, caller, is_admin, id, target).await
    }
    pub async fn list_shares(s: &AppState, caller: Uuid, is_admin: bool, id: Uuid) -> R<Value> {
        connectors::list_shares_view(&s.mcp, caller, is_admin, id).await
    }
    pub async fn search_share_targets(s: &AppState, q: &str) -> R<Value> {
        connectors::search_share_targets_view(&s.mcp, q).await
    }
    pub async fn list_consumers(s: &AppState, caller: Uuid, is_admin: bool, id: Uuid) -> R<Value> {
        connectors::list_consumers_view(&s.mcp, caller, is_admin, id).await
    }
    pub async fn pin(s: &AppState, user: Uuid, id: Uuid) -> R<()> {
        connectors::pin_connector_view(&s.mcp, user, id).await
    }
    pub async fn unpin(s: &AppState, user: Uuid, id: Uuid) -> R<()> {
        connectors::unpin_connector_view(&s.mcp, user, id).await
    }
    pub async fn list_pinned(s: &AppState, user: Uuid) -> R<Value> {
        connectors::list_pinned_view(&s.mcp, user).await
    }
    pub async fn list_recent(s: &AppState, user: Uuid) -> R<Value> {
        connectors::list_recent_view(&s.mcp, user).await
    }
}

pub mod credentials {
    use super::*;
    use nasiko_mcp_gateway::credentials;

    pub async fn register(s: &AppState, user: Uuid, connector_id: Uuid, value: &str) -> R<Value> {
        let c = credentials::authorize_connector(&s.mcp, user, connector_id).await?;
        credentials::register_credential(&s.mcp, user, &c, value).await?;
        Ok(json!({ "connector_id": c.id, "name": c.name, "connected": true }))
    }
    pub async fn status(s: &AppState, user: Uuid, connector_id: Uuid) -> R<Value> {
        let c = credentials::authorize_connector(&s.mcp, user, connector_id).await?;
        let auth = credentials::credential_status(&s.mcp, connector_id, user).await?;
        Ok(json!({ "connector_id": c.id, "name": c.name, "connected": auth.is_some(), "auth_type": auth }))
    }
    pub async fn delete(s: &AppState, user: Uuid, connector_id: Uuid) -> R<()> {
        let c = credentials::authorize_connector(&s.mcp, user, connector_id).await?;
        credentials::delete_credential(&s.mcp, &c, user).await
    }
}

pub mod oauth {
    use super::*;
    pub use nasiko_mcp_gateway::oauth::CallbackOutcome;
    use nasiko_mcp_gateway::{oauth, repo, session};

    pub async fn authorize(
        s: &AppState,
        user: Uuid,
        connector_id: Uuid,
        client_id: Option<String>,
        redirect_url: Option<String>,
    ) -> R<Value> {
        let c = oauth::load_accessible_oauth_connector(&s.mcp, user, connector_id).await?;
        let (id, name) = (c.id, c.name.clone());
        let url = oauth::begin_authorization(&s.mcp, user, c, redirect_url, client_id).await?;
        Ok(json!({ "connector_id": id, "name": name, "authorization_url": url }))
    }
    pub async fn status(s: &AppState, user: Uuid, connector_id: Uuid) -> R<Value> {
        let c = oauth::load_accessible_oauth_connector(&s.mcp, user, connector_id).await?;
        let conn = repo::get_user_connection(&s.mcp.db, user, connector_id).await?;
        let authorized = conn.as_ref().and_then(|x| x.encrypted_credential.as_ref()).is_some();
        Ok(json!({
            "connector_id": c.id,
            "name": c.name,
            "authorized": authorized,
            "expires_at": conn.as_ref().and_then(|x| x.token_expires_at),
            "scope": conn.and_then(|x| x.scope),
        }))
    }
    pub async fn revoke(s: &AppState, user: Uuid, connector_id: Uuid) -> R<()> {
        oauth::load_accessible_oauth_connector(&s.mcp, user, connector_id).await?;
        if !repo::delete_user_connection(&s.mcp.db, user, connector_id).await? {
            return Err(McpError::NotFound("no token to revoke".into()));
        }
        session::invalidate_session_cache(&s.mcp, user).await;
        Ok(())
    }
    pub async fn callback(
        s: &AppState,
        code: Option<String>,
        state: Option<String>,
        error: Option<String>,
        error_description: Option<String>,
    ) -> CallbackOutcome {
        oauth::handle_callback(&s.mcp, code, state, error, error_description).await
    }
}

pub mod permissions {
    use super::*;
    pub use nasiko_mcp_gateway::permissions::ToolRuleInput;
    use nasiko_mcp_gateway::permissions;

    pub async fn list_connectors(s: &AppState, user: Uuid, agent: Uuid) -> R<Value> {
        permissions::list_connectors_view(&s.mcp, user, agent).await
    }
    pub async fn set_connector_access(s: &AppState, user: Uuid, agent: Uuid, connector: Uuid, enabled: bool) -> R<Value> {
        permissions::set_connector_access_view(&s.mcp, user, agent, connector, enabled).await
    }
    pub async fn list_connector_tools(s: &AppState, user: Uuid, agent: Uuid, connector: Uuid) -> R<Value> {
        permissions::list_connector_tools_view(&s.mcp, user, agent, connector).await
    }
    pub async fn list_tool_rules(s: &AppState, agent: Uuid) -> R<Value> {
        permissions::list_tool_rules_view(&s.mcp, agent).await
    }
    pub async fn bulk_update_tools(s: &AppState, user: Uuid, agent: Uuid, rules: &[ToolRuleInput]) -> R<Value> {
        permissions::bulk_update_tools(&s.mcp, user, agent, rules).await
    }
    pub async fn reset(s: &AppState, agent: Uuid) -> R<u64> {
        permissions::reset(&s.mcp, agent).await
    }
}

pub mod webhooks {
    use super::*;
    pub use nasiko_mcp_gateway::webhooks::WebhookOutcome;
    use nasiko_mcp_gateway::webhooks;

    pub fn verify_signature(id: &str, ts: &str, body: &str, sig: &str, secret: &str) -> bool {
        webhooks::verify_signature(id, ts, body, sig, secret)
    }
    pub async fn process(s: &AppState, payload: &Value) -> R<WebhookOutcome> {
        webhooks::process_event(&s.mcp, payload).await
    }
}

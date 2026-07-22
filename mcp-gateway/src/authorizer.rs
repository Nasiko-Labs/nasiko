//! Connector reachability ("Layer 1") behind a swappable trait.
//!
//! Reachability defaults to owner ∪ user/public grant (composio always open).
//! An edition can swap in a richer impl without changing this crate. Every
//! reachability decision routes through this trait so the single-connector check
//! and the connector/tool lists that feed the agent's live tool set stay
//! consistent. The trait carries no edition-specific concepts.

use std::collections::HashMap;

use async_trait::async_trait;
use sqlx::PgPool;
use uuid::Uuid;

use crate::error::Result;
use crate::repo::{self, McpConnector};
use crate::types::AccessReason;

/// Decides which connectors a user may reach (Layer 1). Held on [`McpState`].
///
/// [`McpState`]: crate::state::McpState
#[async_trait]
pub trait ConnectorAuthorizer: Send + Sync {
    /// Can `user_id` reach `connector_id` at all?
    async fn can_access_connector(&self, db: &PgPool, user_id: Uuid, connector_id: Uuid) -> Result<bool>;
    /// Every connector the user can reach (composio + custom).
    async fn list_accessible_connectors(&self, db: &PgPool, user_id: Uuid) -> Result<Vec<McpConnector>>;
    /// Accessible custom (mcp_server) connectors only — feeds the live tool set.
    async fn list_accessible_mcp_connectors(&self, db: &PgPool, user_id: Uuid) -> Result<Vec<McpConnector>>;
    /// Every specific person with access to `connector`, and why (owner, direct
    /// grant — EE additionally: team/department membership). Does NOT enumerate
    /// a "public" grant as a person; that's a flag on the connector, not a
    /// specific reachable user — callers surface it separately.
    async fn list_access_reasons(&self, db: &PgPool, connector: &McpConnector) -> Result<Vec<AccessReason>>;
}

/// Default reachability: composio ∪ owner ∪ user/public grant. Delegates to `repo`.
pub struct OssConnectorAuthorizer;

#[async_trait]
impl ConnectorAuthorizer for OssConnectorAuthorizer {
    async fn can_access_connector(&self, db: &PgPool, user_id: Uuid, connector_id: Uuid) -> Result<bool> {
        repo::can_access_connector(db, user_id, connector_id).await
    }
    async fn list_accessible_connectors(&self, db: &PgPool, user_id: Uuid) -> Result<Vec<McpConnector>> {
        repo::list_accessible_connectors(db, user_id).await
    }
    async fn list_accessible_mcp_connectors(&self, db: &PgPool, user_id: Uuid) -> Result<Vec<McpConnector>> {
        repo::list_accessible_mcp_connectors(db, user_id).await
    }
    async fn list_access_reasons(&self, db: &PgPool, connector: &McpConnector) -> Result<Vec<AccessReason>> {
        let grants = repo::list_grants_for_connector(db, connector.id).await?;
        let direct_ids: Vec<Uuid> =
            grants.iter().filter(|g| g.grant_type == "user").filter_map(|g| Uuid::parse_str(&g.grantee_id).ok()).collect();

        let mut candidate_ids: Vec<Uuid> = connector.owner_id.into_iter().chain(direct_ids.iter().copied()).collect();
        candidate_ids.sort();
        candidate_ids.dedup();
        let labels = repo::resolve_user_labels(db, &candidate_ids).await?;

        // Owner first so it's never overwritten by a (redundant) direct grant
        // on the same user — most-specific reason wins.
        let mut reasons: HashMap<Uuid, AccessReason> = HashMap::new();
        if let Some(owner_id) = connector.owner_id
            && let Some((username, display_name)) = labels.get(&owner_id)
        {
            reasons.insert(
                owner_id,
                AccessReason { user_id: owner_id, username: username.clone(), display_name: display_name.clone(), via: "owner".into(), via_label: None },
            );
        }
        for id in direct_ids {
            if reasons.contains_key(&id) {
                continue;
            }
            if let Some((username, display_name)) = labels.get(&id) {
                reasons.insert(
                    id,
                    AccessReason { user_id: id, username: username.clone(), display_name: display_name.clone(), via: "direct".into(), via_label: None },
                );
            }
        }
        Ok(reasons.into_values().collect())
    }
}

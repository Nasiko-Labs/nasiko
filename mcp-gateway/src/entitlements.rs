//! Per-user tool entitlements behind a swappable trait.
//!
//! An entitlement answers "which tools of which connectors may this user use",
//! independently of any agent. It can only narrow what an agent's
//! [`PermissionContext`](crate::permissions::PermissionContext) allows, never
//! widen it: see [`EffectiveAccess`](crate::permissions::EffectiveAccess).
//!
//! The OSS default ([`OssToolEntitlements`]) is [`UserEntitlement::Unrestricted`],
//! so OSS behaviour is unchanged. An edition swaps in its own impl on
//! `McpState::entitlements`.

use std::collections::{HashMap, HashSet};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::cache;
use crate::error::Result;
use crate::permissions::sha256_hex16;
use crate::state::McpState;

/// Hash of [`UserEntitlement::Unrestricted`]; a constant so it never splits a cache key.
const UNRESTRICTED_HASH: &str = "unrestricted";

/// The tools of one connector a user may use.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToolSet {
    /// Every tool, including tools the connector adds later.
    All,
    /// Exactly these tool names (exact, case-sensitive match).
    Only(HashSet<String>),
}

/// What one user is entitled to, keyed by connector id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum UserEntitlement {
    /// No per-user restriction: the agent's permissions alone decide.
    Unrestricted,
    /// Only the listed tools of the listed connectors. A connector with no
    /// entry grants nothing. The implementor resolves any exemptions (platform
    /// connectors, administrators) BEFORE returning, by listing them as `All`
    /// or returning `Unrestricted`; the gateway applies this map literally.
    Restricted(HashMap<Uuid, ToolSet>),
}

impl UserEntitlement {
    /// May the user use `tool_name` of `connector_id`?
    pub fn allows(&self, connector_id: Uuid, tool_name: &str) -> bool {
        match self {
            Self::Unrestricted => true,
            Self::Restricted(map) => match map.get(&connector_id) {
                None => false,
                Some(ToolSet::All) => true,
                Some(ToolSet::Only(names)) => names.contains(tool_name),
            },
        }
    }

    /// Does the user have at least one tool on `connector_id`?
    pub fn allows_connector(&self, connector_id: Uuid) -> bool {
        match self {
            Self::Unrestricted => true,
            Self::Restricted(map) => match map.get(&connector_id) {
                None => false,
                Some(ToolSet::All) => true,
                Some(ToolSet::Only(names)) => !names.is_empty(),
            },
        }
    }

    /// True for [`Self::Restricted`].
    pub fn is_restricted(&self) -> bool {
        matches!(self, Self::Restricted(_))
    }

    /// Stable, order-independent sha256[:16] folded into the manifest cache key.
    pub fn hash(&self) -> String {
        let Self::Restricted(map) = self else {
            return UNRESTRICTED_HASH.to_string();
        };
        let mut entries: Vec<(String, Option<Vec<String>>)> = map
            .iter()
            .map(|(cid, set)| {
                let names = match set {
                    ToolSet::All => None,
                    ToolSet::Only(names) => {
                        let mut sorted: Vec<String> = names.iter().cloned().collect();
                        sorted.sort();
                        Some(sorted)
                    }
                };
                (cid.to_string(), names)
            })
            .collect();
        entries.sort();
        let raw = serde_json::to_string(&entries).unwrap_or_default();
        sha256_hex16(raw.as_bytes())
    }
}

/// Resolves a user's tool entitlement. Held on [`McpState`].
#[async_trait]
pub trait ToolEntitlements: Send + Sync {
    /// The entitlement of `user_id`. Called once per gateway request (cached
    /// by [`load_user_entitlement`]).
    async fn for_user(&self, user_id: Uuid) -> Result<UserEntitlement>;
}

/// Default: nobody is restricted.
pub struct OssToolEntitlements;

#[async_trait]
impl ToolEntitlements for OssToolEntitlements {
    async fn for_user(&self, _user_id: Uuid) -> Result<UserEntitlement> {
        Ok(UserEntitlement::Unrestricted)
    }
}

fn entitlement_cache_key(user_id: Uuid) -> String {
    format!("mcp:ent:{user_id}")
}

/// Load the user's entitlement, Redis cached for `perm_cache_ttl_seconds`.
/// A Redis outage degrades to a cache miss, like `load_permission_context`.
/// `Unrestricted` is cached too: an edition's resolver may need a database read
/// to reach it (enforcement off, a superuser), which would otherwise run on every
/// request. Whatever changes the answer must call [`invalidate_user_entitlements`];
/// a change nothing hooks (e.g. editing `is_superuser` in the database by hand)
/// shows up after at most the TTL.
pub async fn load_user_entitlement(state: &McpState, user_id: Uuid) -> Result<UserEntitlement> {
    let key = entitlement_cache_key(user_id);
    if let Some(cached) = cache::get_json::<UserEntitlement>(&state.redis, &key).await {
        return Ok(cached);
    }
    let entitlement = state.entitlements.for_user(user_id).await?;
    cache::set_json_ex(
        &state.redis,
        &key,
        &entitlement,
        state.config.perm_cache_ttl_seconds,
    )
    .await;
    Ok(entitlement)
}

/// Drop the cached entitlements of `user_ids` (call after changing them), in
/// batched multi-key deletes so an org-wide change is a handful of commands.
pub async fn invalidate_user_entitlements(state: &McpState, user_ids: &[Uuid]) {
    let keys: Vec<String> = user_ids.iter().map(|u| entitlement_cache_key(*u)).collect();
    cache::delete_many(&state.redis, &keys).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn only(names: &[&str]) -> ToolSet {
        ToolSet::Only(names.iter().map(|n| n.to_string()).collect())
    }

    #[test]
    fn unrestricted_allows_everything() {
        let e = UserEntitlement::Unrestricted;
        assert!(e.allows(Uuid::new_v4(), "any"));
        assert!(e.allows_connector(Uuid::new_v4()));
    }

    #[test]
    fn restricted_missing_connector_denies() {
        let e = UserEntitlement::Restricted(HashMap::new());
        let c = Uuid::new_v4();
        assert!(!e.allows(c, "any"));
        assert!(!e.allows_connector(c));
    }

    #[test]
    fn restricted_all_and_only() {
        let (all, some, empty) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        let e = UserEntitlement::Restricted(HashMap::from([
            (all, ToolSet::All),
            (some, only(&["read"])),
            (empty, only(&[])),
        ]));
        assert!(e.allows(all, "whatever"));
        assert!(e.allows(some, "read"));
        assert!(!e.allows(some, "READ"), "match is case-sensitive");
        assert!(!e.allows(some, "write"));
        assert!(!e.allows(empty, "read"));
        assert!(e.allows_connector(all));
        assert!(e.allows_connector(some));
        assert!(!e.allows_connector(empty));
    }

    #[test]
    fn hash_is_constant_for_unrestricted_and_order_independent() {
        assert_eq!(UserEntitlement::Unrestricted.hash(), UNRESTRICTED_HASH);
        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        let one =
            UserEntitlement::Restricted(HashMap::from([(a, only(&["x", "y"])), (b, ToolSet::All)]));
        let two =
            UserEntitlement::Restricted(HashMap::from([(b, ToolSet::All), (a, only(&["y", "x"]))]));
        assert_eq!(one.hash(), two.hash());
        let narrower =
            UserEntitlement::Restricted(HashMap::from([(a, only(&["x"])), (b, ToolSet::All)]));
        assert_ne!(one.hash(), narrower.hash());
        assert_ne!(one.hash(), UserEntitlement::Unrestricted.hash());
    }

    #[test]
    fn serde_round_trips() {
        let c = Uuid::new_v4();
        let e = UserEntitlement::Restricted(HashMap::from([(c, only(&["x"]))]));
        let json = serde_json::to_string(&e).unwrap();
        assert_eq!(serde_json::from_str::<UserEntitlement>(&json).unwrap(), e);
    }
}

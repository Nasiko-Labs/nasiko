//! Per-agent permission engine, keyed by connector id.
//!
//! Two levels: (1) is a connector enabled for this agent at all, (2) per-tool
//! stance `allow | ask | block` with glob patterns (`*`, `GMAIL_*`,
//! `GMAIL_SEND_EMAIL`), priority `block > ask > allow`. Default (no row): every
//! connector enabled, every tool allowed. The [`PermissionContext`] is computed
//! once per request, Redis-cached, and dropped on any permission write. Its
//! `hash` feeds the manifest cache key.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::cache;
use crate::error::{McpError, Result};
use crate::provider::generic::LIST_TIMEOUT;
use crate::repo;
use crate::state::McpState;
use crate::types::Stance;

/// One `{pattern, stance}` entry as stored in `mcp_agent_connector_access.tool_rules`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolRule {
    pub pattern: String,
    pub stance: String,
}

/// One flattened sub-tool permission rule (connector-scoped).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionRule {
    pub connector_id: Uuid,
    pub tool_pattern: String,
    pub stance: Stance,
}

/// Pre-loaded, immutable permission state for one `(user_id, agent_id)` pair.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionContext {
    pub user_id: Uuid,
    pub agent_id: Uuid,
    /// Connector ids explicitly disabled (`enabled = false`).
    pub disabled_connectors: HashSet<Uuid>,
    pub rules: Vec<PermissionRule>,
    /// sha256[:16] of the rules + disabled set — the manifest cache-key signal.
    pub hash: String,
}

impl PermissionContext {
    /// True unless the connector is explicitly disabled.
    pub fn is_connector_enabled(&self, connector_id: Uuid) -> bool {
        !self.disabled_connectors.contains(&connector_id)
    }

    /// Resolve the stance for `(connector, tool)`. Priority `block > ask > allow`;
    /// default `allow`. Matching is case-insensitive glob.
    pub fn get_stance(&self, connector_id: Uuid, tool_name: &str) -> Stance {
        let tool_lower = tool_name.to_ascii_lowercase();
        let matching: Vec<Stance> = self
            .rules
            .iter()
            .filter(|r| {
                r.connector_id == connector_id
                    && wildcard_match(&r.tool_pattern.to_ascii_lowercase(), &tool_lower)
            })
            .map(|r| r.stance)
            .collect();

        if matching.is_empty() {
            return Stance::Allow;
        }
        for priority in [Stance::Block, Stance::Ask, Stance::Allow] {
            if matching.contains(&priority) {
                return priority;
            }
        }
        Stance::Allow
    }

    pub fn has_any_restriction(&self) -> bool {
        !self.disabled_connectors.is_empty() || !self.rules.is_empty()
    }
}

/// Extract the Composio toolkit slug from a tool slug:
/// `GMAIL_SEND_EMAIL` → `gmail`. Skips leading underscores so a malformed
/// `_GMAIL_SEND` still resolves to `gmail` rather than an empty toolkit (which
/// would bypass the per-toolkit permission check).
pub fn toolkit_from_composio_slug(slug: &str) -> String {
    slug.split('_').find(|s| !s.is_empty()).unwrap_or("").to_ascii_lowercase()
}

fn perm_cache_key(user_id: Uuid, agent_id: Uuid) -> String {
    format!("mcp:perm:{user_id}:{agent_id}")
}

/// Load the permission context for `(user_id, agent_id)` — Redis cached.
pub async fn load_permission_context(
    state: &McpState,
    user_id: Uuid,
    agent_id: Uuid,
) -> Result<PermissionContext> {
    let key = perm_cache_key(user_id, agent_id);
    if let Some(ctx) = cache::get_json::<PermissionContext>(&state.redis, &key).await {
        return Ok(ctx);
    }

    let rows = repo::get_agent_connector_access(&state.db, user_id, agent_id).await?;
    let mut disabled_connectors = HashSet::new();
    let mut rules = Vec::new();
    for row in rows {
        if !row.enabled {
            disabled_connectors.insert(row.connector_id);
        }
        for tr in parse_tool_rules(&row.tool_rules) {
            if let Some(stance) = Stance::from_str(&tr.stance) {
                rules.push(PermissionRule { connector_id: row.connector_id, tool_pattern: tr.pattern, stance });
            }
        }
    }

    let hash = compute_hash(&rules, &disabled_connectors);
    let ctx = PermissionContext { user_id, agent_id, disabled_connectors, rules, hash };
    cache::set_json_ex(&state.redis, &key, &ctx, state.config.perm_cache_ttl_seconds).await;
    Ok(ctx)
}

/// Drop the cached permission context for a `(user, agent)` pair.
pub async fn invalidate_permission_cache(state: &McpState, user_id: Uuid, agent_id: Uuid) {
    cache::delete(&state.redis, &perm_cache_key(user_id, agent_id)).await;
}

fn parse_tool_rules(raw: &Value) -> Vec<ToolRule> {
    serde_json::from_value(raw.clone()).unwrap_or_default()
}

/// Deterministic sha256[:16] over the rules + disabled set (order-independent).
fn compute_hash(rules: &[PermissionRule], disabled: &HashSet<Uuid>) -> String {
    let mut data: Vec<[String; 3]> = rules
        .iter()
        .map(|r| [r.connector_id.to_string(), r.tool_pattern.clone(), r.stance.as_str().to_string()])
        .collect();
    data.sort();

    let mut disabled_sorted: Vec<String> = disabled.iter().map(Uuid::to_string).collect();
    disabled_sorted.sort();
    for s in disabled_sorted {
        data.push(["__disabled__".to_string(), s, "block".to_string()]);
    }

    let raw = serde_json::to_string(&data).unwrap_or_default();
    sha256_hex16(raw.as_bytes())
}

/// First 16 hex chars of the SHA-256 of `bytes`. Shared with the manifest key.
pub(crate) fn sha256_hex16(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(bytes);
    let mut hex = hex::encode(digest);
    hex.truncate(16);
    hex
}

/// Case-sensitive glob match supporting `*` and `?`. Callers lowercase both sides.
fn wildcard_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    let (mut pi, mut ti) = (0usize, 0usize);
    let mut star: Option<usize> = None;
    let mut star_ti = 0usize;

    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            star_ti = ti;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            star_ti += 1;
            ti = star_ti;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

// ═══════════════════════════════════════════════════════════════════════════
// Management — the connector UI backend behind `/api/mcp/agents/{agent_id}/*`.
// ═══════════════════════════════════════════════════════════════════════════

pub const STANCES: [&str; 3] = ["allow", "ask", "block"];

/// `GET /agents/{id}/connectors` view: connectors this agent can use, with
/// per-agent enabled + connected status.
pub async fn list_connectors_view(state: &McpState, user_id: Uuid, agent_id: Uuid) -> Result<Value> {
    let connectors = repo::list_accessible_connectors(&state.db, user_id).await?;
    let access = repo::get_agent_connector_access(&state.db, user_id, agent_id).await?;
    let enabled_map: HashMap<Uuid, bool> = access.into_iter().map(|r| (r.connector_id, r.enabled)).collect();
    let connected: HashSet<Uuid> = repo::list_user_connections(&state.db, user_id, Some("ACTIVE"))
        .await?
        .into_iter()
        .map(|c| c.connector_id)
        .collect();

    let data: Vec<Value> = connectors
        .into_iter()
        .map(|c| {
            let is_connected = connected.contains(&c.id) || c.auth_type.as_deref() == Some("none");
            json!({
                "connector_id": c.id,
                "provider_type": c.provider_type,
                "name": c.name,
                "display_name": c.display_name.unwrap_or_else(|| crate::catalog::capitalize(&c.name)),
                "logo_url": c.logo_url,
                "enabled": enabled_map.get(&c.id).copied().unwrap_or(true),
                "connected": is_connected,
            })
        })
        .collect();
    Ok(json!({ "data": data }))
}

/// `PUT /agents/{id}/connectors/{connector_id}` — toggle a connector, preserving
/// any existing tool rules. Requires the connector be reachable (Layer 1).
pub async fn set_connector_access_view(
    state: &McpState,
    user_id: Uuid,
    agent_id: Uuid,
    connector_id: Uuid,
    enabled: bool,
) -> Result<Value> {
    if !repo::can_access_connector(&state.db, user_id, connector_id).await? {
        return Err(McpError::NotFound(format!("connector '{connector_id}' not found")));
    }
    let existing_rules = repo::get_agent_connector_access_row(&state.db, user_id, agent_id, connector_id)
        .await?
        .map(|r| r.tool_rules)
        .unwrap_or_else(|| json!([]));
    let row = repo::upsert_agent_connector_access(&state.db, user_id, agent_id, connector_id, enabled, &existing_rules)
        .await?;
    invalidate_permission_cache(state, user_id, agent_id).await;
    Ok(json!({ "connector_id": row.connector_id, "enabled": row.enabled }))
}

/// `GET /agents/{id}/connectors/{connector_id}/tools` — tools for a connector
/// with this agent's current stance. Reads the synced catalog; syncs live if empty.
pub async fn list_connector_tools_view(
    state: &McpState,
    user_id: Uuid,
    agent_id: Uuid,
    connector_id: Uuid,
) -> Result<Value> {
    if !repo::can_access_connector(&state.db, user_id, connector_id).await? {
        return Err(McpError::NotFound(format!("connector '{connector_id}' not found")));
    }
    let perms = load_permission_context(state, user_id, agent_id).await?;

    let mut catalog = repo::list_connector_tools(&state.db, connector_id).await?;
    if catalog.is_empty() {
        sync_connector_tools(state, user_id, connector_id).await?;
        catalog = repo::list_connector_tools(&state.db, connector_id).await?;
    }

    let out: Vec<Value> = catalog
        .into_iter()
        .map(|t| {
            let stance = perms.get_stance(connector_id, &t.tool_name);
            json!({ "name": t.tool_name, "description": t.description, "stance": stance.as_str() })
        })
        .collect();
    Ok(json!({ "data": out }))
}

/// Sync a connector's tool catalog from its live backend into `mcp_connector_tools`.
async fn sync_connector_tools(state: &McpState, user_id: Uuid, connector_id: Uuid) -> Result<()> {
    let connector = repo::get_connector_by_id(&state.db, connector_id)
        .await?
        .ok_or_else(|| McpError::NotFound(format!("connector '{connector_id}' not found")))?;

    let tools: Vec<(String, Option<String>)> = if connector.is_composio() {
        match &state.providers.composio {
            Some(p) => p.list_toolkit_tools(&connector.name).await?.into_iter().map(|t| (t.name, t.description)).collect(),
            None => Vec::new(),
        }
    } else {
        let built = crate::credentials::build_generic_servers(state, user_id).await?;
        match built.iter().find(|s| s.connector_id == connector_id) {
            Some(cfg) => state
                .providers
                .mcp
                .list_tools(cfg, LIST_TIMEOUT, None)
                .await?
                .into_iter()
                .filter_map(|t| {
                    t.get("name").and_then(|n| n.as_str()).map(|name| {
                        (name.to_string(), t.get("description").and_then(|d| d.as_str()).map(str::to_string))
                    })
                })
                .collect(),
            None => Vec::new(),
        }
    };

    if !tools.is_empty() {
        repo::upsert_connector_tools(&state.db, connector_id, &tools).await?;
    }
    Ok(())
}

/// `GET /agents/{id}/tools` view: the agent's current tool rules across connectors.
pub async fn list_tool_rules_view(state: &McpState, user_id: Uuid, agent_id: Uuid) -> Result<Value> {
    let rows = repo::get_agent_connector_access(&state.db, user_id, agent_id).await?;
    let mut data: Vec<Value> = Vec::new();
    for row in rows {
        for tr in parse_tool_rules(&row.tool_rules) {
            data.push(json!({ "connector_id": row.connector_id, "tool_pattern": tr.pattern, "stance": tr.stance }));
        }
    }
    Ok(json!({ "data": data }))
}

/// One rule input for [`bulk_update_tools`].
pub struct ToolRuleInput {
    pub connector_id: Uuid,
    pub tool_pattern: String,
    pub stance: String,
}

/// `PUT /agents/{id}/tools` — replace tool rules. Groups by connector, validates
/// + dedupes each connector's rules, and upserts (preserving `enabled`).
pub async fn bulk_update_tools(
    state: &McpState,
    user_id: Uuid,
    agent_id: Uuid,
    rules: &[ToolRuleInput],
) -> Result<Value> {
    for rule in rules {
        if !STANCES.contains(&rule.stance.as_str()) {
            return Err(McpError::BadRequest(format!("stance must be one of {STANCES:?}")));
        }
    }

    // Group by connector, deduping on tool_pattern (last write wins).
    let mut by_connector: HashMap<Uuid, HashMap<String, String>> = HashMap::new();
    for rule in rules {
        by_connector
            .entry(rule.connector_id)
            .or_default()
            .insert(rule.tool_pattern.clone(), rule.stance.clone());
    }

    let mut applied: Vec<Value> = Vec::new();
    for (connector_id, patterns) in by_connector {
        if !repo::can_access_connector(&state.db, user_id, connector_id).await? {
            return Err(McpError::NotFound(format!("connector '{connector_id}' not found")));
        }
        let enabled = repo::get_agent_connector_access_row(&state.db, user_id, agent_id, connector_id)
            .await?
            .map(|r| r.enabled)
            .unwrap_or(true);
        let tool_rules: Vec<ToolRule> =
            patterns.iter().map(|(p, s)| ToolRule { pattern: p.clone(), stance: s.clone() }).collect();
        let rules_json = serde_json::to_value(&tool_rules)?;
        repo::upsert_agent_connector_access(&state.db, user_id, agent_id, connector_id, enabled, &rules_json).await?;
        for tr in tool_rules {
            applied.push(json!({ "connector_id": connector_id, "tool_pattern": tr.pattern, "stance": tr.stance }));
        }
    }

    invalidate_permission_cache(state, user_id, agent_id).await;
    Ok(json!({ "data": applied }))
}

/// `DELETE /agents/{id}/permissions` — reset to all-allowed.
pub async fn reset(state: &McpState, user_id: Uuid, agent_id: Uuid) -> Result<u64> {
    let deleted = repo::delete_all_agent_access(&state.db, user_id, agent_id).await?;
    invalidate_permission_cache(state, user_id, agent_id).await;
    Ok(deleted)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(rules: Vec<PermissionRule>, disabled: &[Uuid]) -> PermissionContext {
        PermissionContext {
            user_id: Uuid::nil(),
            agent_id: Uuid::nil(),
            disabled_connectors: disabled.iter().copied().collect(),
            rules,
            hash: String::new(),
        }
    }
    fn rule(c: Uuid, pat: &str, stance: Stance) -> PermissionRule {
        PermissionRule { connector_id: c, tool_pattern: pat.into(), stance }
    }

    #[test]
    fn wildcard_semantics_and_edges() {
        assert!(wildcard_match("*", "anything"));
        assert!(wildcard_match("gmail_*", "gmail_send_email"));
        assert!(!wildcard_match("gmail_*", "slack_post"));
        assert!(wildcard_match("gmail_?end", "gmail_send"));
        assert!(!wildcard_match("gmail_?end", "gmail_bend_x"));
        assert!(wildcard_match("", ""));
        assert!(!wildcard_match("", "x"));
        assert!(wildcard_match("*send*", "gmail_send_email"));
        assert!(!wildcard_match("a?c", "ac"));
    }

    #[test]
    fn stance_priority_and_connector_scoping() {
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let c = ctx(
            vec![
                rule(a, "*", Stance::Allow),
                rule(a, "gmail_send_*", Stance::Ask),
                rule(a, "gmail_send_email", Stance::Block),
            ],
            &[],
        );
        assert_eq!(c.get_stance(a, "GMAIL_SEND_EMAIL"), Stance::Block);
        assert_eq!(c.get_stance(a, "GMAIL_SEND_SMS"), Stance::Ask);
        assert_eq!(c.get_stance(a, "GMAIL_READ"), Stance::Allow);
        // Rule on connector a must not affect connector b.
        assert_eq!(c.get_stance(b, "ANYTHING"), Stance::Allow);
    }

    #[test]
    fn enabled_and_restriction_flags() {
        let d = Uuid::new_v4();
        let c = ctx(vec![], &[d]);
        assert!(!c.is_connector_enabled(d));
        assert!(c.is_connector_enabled(Uuid::new_v4()));
        assert!(c.has_any_restriction());
        assert!(!ctx(vec![], &[]).has_any_restriction());
    }

    #[test]
    fn hash_is_deterministic_and_order_independent() {
        let a = Uuid::from_u128(1);
        let b = Uuid::from_u128(2);
        let disabled: HashSet<Uuid> = [a, b].into_iter().collect();
        let r1 = vec![rule(a, "A", Stance::Block), rule(b, "B", Stance::Ask)];
        let r2 = vec![rule(b, "B", Stance::Ask), rule(a, "A", Stance::Block)];
        assert_eq!(compute_hash(&r1, &disabled), compute_hash(&r2, &disabled));
        let r3 = vec![rule(a, "A", Stance::Allow), rule(b, "B", Stance::Ask)];
        assert_ne!(compute_hash(&r1, &disabled), compute_hash(&r3, &disabled));
        assert_eq!(compute_hash(&r1, &disabled).len(), 16);
    }

    #[test]
    fn toolkit_slug_extraction() {
        assert_eq!(toolkit_from_composio_slug("GMAIL_SEND_EMAIL"), "gmail");
        assert_eq!(toolkit_from_composio_slug("NOUNDERSCORE"), "nounderscore");
        assert_eq!(toolkit_from_composio_slug(""), "");
        // Leading underscore must not yield an empty (bypass-prone) toolkit.
        assert_eq!(toolkit_from_composio_slug("_GMAIL_SEND"), "gmail");
        assert_eq!(toolkit_from_composio_slug("___"), "");
    }

    #[test]
    fn stances_are_exactly_allow_ask_block() {
        assert_eq!(STANCES, ["allow", "ask", "block"]);
    }
}

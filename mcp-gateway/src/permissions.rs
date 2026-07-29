//! Per-agent permission engine, keyed by connector id.
//!
//! Two levels: (1) is a connector enabled for this agent at all, (2) per-tool
//! stance `allow | ask | block` with glob patterns (`*`, `GMAIL_*`,
//! `GMAIL_SEND_EMAIL`), priority `block > ask > allow`. Default (no row): connector
//! disabled (default-deny), every tool allowed once the connector is enabled. The [`PermissionContext`] is computed
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

/// Pre-loaded, immutable permission state for one agent — shared by every
/// caller who manages it (see `load_permission_context`'s doc).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionContext {
    pub agent_id: Uuid,
    /// Connector ids explicitly enabled (`enabled = true`). Default-off:
    /// only connectors in this set are allowed; everything else is denied.
    pub enabled_connectors: HashSet<Uuid>,
    pub rules: Vec<PermissionRule>,
    /// sha256[:16] of the rules + enabled set — the manifest cache-key signal.
    pub hash: String,
}

/// The per-agent Layer-2 decision for one `(connector, tool)`. This is the
/// SINGLE source of truth shared by `tools/list` filtering (aggregator) and
/// `tools/call` enforcement (protocol): both call [`PermissionContext::decide`],
/// so the two surfaces can never drift apart (the drift that let a disabled
/// connector's tools stay callable — see the `decide` doc).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolAccess {
    /// Callable, no prompt.
    Allowed,
    /// Requires explicit user approval (surfaced, never auto-approved).
    Ask,
    /// Must not run — connector disabled for the agent, or tool stance `block`.
    Denied,
}

impl PermissionContext {
    /// True only when the connector is explicitly enabled. Default-off:
    /// connectors must be toggled on per-agent before their tools are visible.
    pub fn is_connector_enabled(&self, connector_id: Uuid) -> bool {
        self.enabled_connectors.contains(&connector_id)
    }

    /// The full Layer-2 access decision for `(connector, tool)`: connector-enable
    /// toggle FIRST (a disabled connector denies every tool, regardless of any
    /// stale allow rule), then the per-tool stance. Layer-1 reachability
    /// (owner/grant) is a separate DB check the caller must already have passed.
    ///
    /// Every place that gates a tool — list filtering and call enforcement —
    /// MUST go through this one method. Do not re-derive the decision from
    /// `is_connector_enabled` + `get_stance` at a call site; that is exactly how
    /// the two paths drifted before (list hid a disabled connector while call
    /// still executed it).
    pub fn decide(&self, connector_id: Uuid, tool_name: &str) -> ToolAccess {
        if !self.is_connector_enabled(connector_id) {
            return ToolAccess::Denied;
        }
        match self.get_stance(connector_id, tool_name) {
            Stance::Block => ToolAccess::Denied,
            Stance::Ask => ToolAccess::Ask,
            Stance::Allow => ToolAccess::Allowed,
        }
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
        !self.enabled_connectors.is_empty() || !self.rules.is_empty()
    }
}

/// Extract the Composio toolkit slug from a tool slug:
/// `GMAIL_SEND_EMAIL` → `gmail`. Skips leading underscores so a malformed
/// `_GMAIL_SEND` still resolves to `gmail` rather than an empty toolkit (which
/// would bypass the per-toolkit permission check).
pub fn toolkit_from_composio_slug(slug: &str) -> String {
    slug.split('_')
        .find(|s| !s.is_empty())
        .unwrap_or("")
        .to_ascii_lowercase()
}

fn perm_cache_key(agent_id: Uuid) -> String {
    format!("mcp:perm:{agent_id}")
}

/// Load the permission context for `agent_id` — Redis cached. Shared by every
/// caller who manages the agent (there is exactly one row per
/// `(agent_id, connector_id)`, not one per caller) — see
/// `mcp_agent_connector_access`'s table comment for why.
pub async fn load_permission_context(
    state: &McpState,
    agent_id: Uuid,
) -> Result<PermissionContext> {
    let key = perm_cache_key(agent_id);
    if let Some(ctx) = cache::get_json::<PermissionContext>(&state.redis, &key).await {
        return Ok(ctx);
    }

    let rows = repo::get_agent_connector_access(&state.db, agent_id).await?;
    let enabled_connectors: HashSet<Uuid> = rows
        .iter()
        .filter(|r| r.enabled)
        .map(|r| r.connector_id)
        .collect();
    let mut rules = Vec::new();
    for row in &rows {
        for tr in parse_tool_rules(&row.tool_rules) {
            if let Some(stance) = Stance::from_str(&tr.stance) {
                rules.push(PermissionRule {
                    connector_id: row.connector_id,
                    tool_pattern: tr.pattern,
                    stance,
                });
            }
        }
    }

    let hash = compute_hash(&rules, &enabled_connectors);
    let ctx = PermissionContext {
        agent_id,
        enabled_connectors,
        rules,
        hash,
    };
    cache::set_json_ex(
        &state.redis,
        &key,
        &ctx,
        state.config.perm_cache_ttl_seconds,
    )
    .await;
    Ok(ctx)
}

/// Drop the cached permission context for an agent.
pub async fn invalidate_permission_cache(state: &McpState, agent_id: Uuid) {
    cache::delete(&state.redis, &perm_cache_key(agent_id)).await;
}

fn parse_tool_rules(raw: &Value) -> Vec<ToolRule> {
    serde_json::from_value(raw.clone()).unwrap_or_default()
}

/// Deterministic sha256[:16] over the rules + disabled set (order-independent).
fn compute_hash(rules: &[PermissionRule], enabled: &HashSet<Uuid>) -> String {
    let mut data: Vec<[String; 3]> = rules
        .iter()
        .map(|r| {
            [
                r.connector_id.to_string(),
                r.tool_pattern.clone(),
                r.stance.as_str().to_string(),
            ]
        })
        .collect();
    data.sort();

    let mut enabled_sorted: Vec<String> = enabled.iter().map(Uuid::to_string).collect();
    enabled_sorted.sort();
    for s in enabled_sorted {
        data.push(["__enabled__".to_string(), s, "allow".to_string()]);
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
pub async fn list_connectors_view(
    state: &McpState,
    user_id: Uuid,
    agent_id: Uuid,
) -> Result<Value> {
    let mut connectors = state
        .authorizer
        .list_accessible_connectors(&state.db, user_id)
        .await?;
    // Union in connectors granted directly to THIS agent (grant_type="agent"),
    // independent of the caller's own reachability — so the owner can see and
    // enable a connector that was shared with their agent specifically.
    let agent_granted = repo::list_agent_granted_connectors(&state.db, agent_id).await?;
    let existing: HashSet<Uuid> = connectors.iter().map(|c| c.id).collect();
    connectors.extend(
        agent_granted
            .into_iter()
            .filter(|c| !existing.contains(&c.id)),
    );

    let access = repo::get_agent_connector_access(&state.db, agent_id).await?;
    let enabled_map: HashMap<Uuid, bool> = access
        .into_iter()
        .map(|r| (r.connector_id, r.enabled))
        .collect();
    let connected: HashSet<Uuid> = repo::list_user_connections(&state.db, user_id, Some("ACTIVE"))
        .await?
        .into_iter()
        .map(|c| c.connector_id)
        .collect();

    let data: Vec<Value> = connectors
        .into_iter()
        .filter(|c| {
            // Only show connectors the user has actually connected to,
            // or custom MCP servers that don't require auth (always reachable).
            connected.contains(&c.id) || c.auth_type.as_deref() == Some("none")
        })
        .map(|c| {
            json!({
                "connector_id": c.id,
                "provider_type": c.provider_type,
                "name": c.name,
                "display_name": c.display_name.unwrap_or_else(|| crate::catalog::capitalize(&c.name)),
                "description": c.description,
                "logo_url": c.logo_url,
                "enabled": enabled_map.get(&c.id).copied().unwrap_or(false),
                "connected": true,
            })
        })
        .collect();
    Ok(json!({ "connectors": data }))
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
    // Reachable either the normal way (caller's own owner/user/public/team/
    // department grant) OR because the connector was shared directly with
    // THIS agent (grant_type="agent") — letting whoever manages the agent
    // configure it even without their own personal reachability.
    let reachable = state
        .authorizer
        .can_access_connector(&state.db, user_id, connector_id)
        .await?
        || repo::agent_has_connector_grant(&state.db, agent_id, connector_id).await?;
    if !reachable {
        return Err(McpError::NotFound(format!(
            "connector '{connector_id}' not found"
        )));
    }
    let existing_rules = repo::get_agent_connector_access_row(&state.db, agent_id, connector_id)
        .await?
        .map(|r| r.tool_rules)
        .unwrap_or_else(|| json!([]));
    let row = repo::upsert_agent_connector_access(
        &state.db,
        agent_id,
        connector_id,
        enabled,
        &existing_rules,
    )
    .await?;
    invalidate_permission_cache(state, agent_id).await;
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
    if !state
        .authorizer
        .can_access_connector(&state.db, user_id, connector_id)
        .await?
    {
        return Err(McpError::NotFound(format!(
            "connector '{connector_id}' not found"
        )));
    }
    let perms = load_permission_context(state, agent_id).await?;

    // Read tools from the DB catalog. If empty, trigger a one-time sync
    // (Composio via list_toolkit_tools, custom MCP via live tools/list).
    let mut catalog = repo::list_connector_tools(&state.db, connector_id).await?;
    if catalog.is_empty() {
        sync_connector_tools(state, user_id, connector_id).await?;
        catalog = repo::list_connector_tools(&state.db, connector_id).await?;
    }
    let out: Vec<Value> = catalog
        .into_iter()
        .map(|t| {
            let stance = perms.get_stance(connector_id, &t.tool_name);
            json!({
                "name": t.tool_name,
                "description": t.description,
                "stance": stance.as_str(),
                "last_synced_at": t.last_synced_at,
            })
        })
        .collect();
    Ok(json!({ "tools": out }))
}

/// Sync a connector's tool catalog from its live backend into `mcp_connector_tools`.
async fn sync_connector_tools(state: &McpState, user_id: Uuid, connector_id: Uuid) -> Result<()> {
    let connector = repo::get_connector_by_id(&state.db, connector_id)
        .await?
        .ok_or_else(|| McpError::NotFound(format!("connector '{connector_id}' not found")))?;

    let tools: Vec<(String, Option<String>)> = if connector.is_composio() {
        match &state.providers.composio {
            Some(p) => p
                .list_toolkit_tools(&connector.name)
                .await?
                .into_iter()
                .map(|t| (t.name, t.description))
                .collect(),
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
                        (
                            name.to_string(),
                            t.get("description")
                                .and_then(|d| d.as_str())
                                .map(str::to_string),
                        )
                    })
                })
                .collect(),
            None => Vec::new(),
        }
    };

    if !tools.is_empty() {
        repo::upsert_connector_tools(&state.db, connector_id, &tools).await?;
        // The catalog's cached tool count (catalog::cached_tool_count) would
        // otherwise keep serving whatever it saw before this sync — most
        // commonly a stale `0` — for up to toolcount_ttl_seconds.
        cache::delete(
            &state.redis,
            &crate::catalog::toolcount_cache_key(connector_id),
        )
        .await;
    }
    Ok(())
}

/// `GET /agents/{id}/tools` view: the agent's current tool rules across connectors.
pub async fn list_tool_rules_view(state: &McpState, agent_id: Uuid) -> Result<Value> {
    let rows = repo::get_agent_connector_access(&state.db, agent_id).await?;
    let mut data: Vec<Value> = Vec::new();
    for row in rows {
        for tr in parse_tool_rules(&row.tool_rules) {
            data.push(json!({ "connector_id": row.connector_id, "tool_pattern": tr.pattern, "stance": tr.stance }));
        }
    }
    Ok(json!({ "rules": data }))
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
            return Err(McpError::BadRequest(format!(
                "stance must be one of {STANCES:?}"
            )));
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
        if !state
            .authorizer
            .can_access_connector(&state.db, user_id, connector_id)
            .await?
        {
            return Err(McpError::NotFound(format!(
                "connector '{connector_id}' not found"
            )));
        }
        let enabled = repo::get_agent_connector_access_row(&state.db, agent_id, connector_id)
            .await?
            .map(|r| r.enabled)
            .unwrap_or(true);
        let tool_rules: Vec<ToolRule> = patterns
            .iter()
            .map(|(p, s)| ToolRule {
                pattern: p.clone(),
                stance: s.clone(),
            })
            .collect();
        let rules_json = serde_json::to_value(&tool_rules)?;
        repo::upsert_agent_connector_access(
            &state.db,
            agent_id,
            connector_id,
            enabled,
            &rules_json,
        )
        .await?;
        for tr in tool_rules {
            applied.push(json!({ "connector_id": connector_id, "tool_pattern": tr.pattern, "stance": tr.stance }));
        }
    }

    invalidate_permission_cache(state, agent_id).await;
    Ok(json!({ "rules": applied }))
}

/// `DELETE /agents/{id}/permissions` — reset to all-allowed.
pub async fn reset(state: &McpState, agent_id: Uuid) -> Result<u64> {
    let deleted = repo::delete_all_agent_access(&state.db, agent_id).await?;
    invalidate_permission_cache(state, agent_id).await;
    Ok(deleted)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(rules: Vec<PermissionRule>, disabled: &[Uuid]) -> PermissionContext {
        // The old test convention passes "disabled" IDs. With the new allowlist
        // model, "disabled" means "not in enabled_connectors". For tests that
        // check disabled behavior, we put nothing in enabled. For tests that
        // need a connector enabled, they pass an empty disabled slice and the
        // connector is implicitly tested via rules.
        // To keep existing tests working: extract all connector IDs from rules,
        // then remove the disabled ones → that's the enabled set.
        let all_from_rules: HashSet<Uuid> = rules.iter().map(|r| r.connector_id).collect();
        let disabled_set: HashSet<Uuid> = disabled.iter().copied().collect();
        let enabled: HashSet<Uuid> = all_from_rules.difference(&disabled_set).copied().collect();
        PermissionContext {
            agent_id: Uuid::nil(),
            enabled_connectors: enabled,
            rules,
            hash: String::new(),
        }
    }
    fn rule(c: Uuid, pat: &str, stance: Stance) -> PermissionRule {
        PermissionRule {
            connector_id: c,
            tool_pattern: pat.into(),
            stance,
        }
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
        let a = Uuid::new_v4(); // explicitly enabled, no tool rules
        let d = Uuid::new_v4(); // never mentioned anywhere
        let c = PermissionContext {
            agent_id: Uuid::nil(),
            enabled_connectors: [a].into_iter().collect(),
            rules: vec![],
            hash: String::new(),
        };
        assert!(c.is_connector_enabled(a));
        // Default-deny: a connector that's never been explicitly enabled is
        // denied, not just one on some old blocklist.
        assert!(!c.is_connector_enabled(d));
        assert!(c.has_any_restriction());
        assert!(!ctx(vec![], &[]).has_any_restriction());
    }

    #[test]
    fn decide_disable_beats_any_stance_and_maps_stances() {
        let a = Uuid::new_v4();
        // A disabled connector denies every tool, even one with an explicit Allow
        // rule — the exact bypass finding #10 was about.
        let disabled = ctx(vec![rule(a, "*", Stance::Allow)], &[a]);
        assert_eq!(disabled.decide(a, "anything"), ToolAccess::Denied);

        // Enabled connector: stance maps 1:1 to the access decision.
        let b = Uuid::new_v4();
        let enabled = ctx(
            vec![
                rule(b, "blocked_*", Stance::Block),
                rule(b, "ask_*", Stance::Ask),
            ],
            &[],
        );
        assert_eq!(enabled.decide(b, "blocked_tool"), ToolAccess::Denied);
        assert_eq!(enabled.decide(b, "ask_tool"), ToolAccess::Ask);
        assert_eq!(enabled.decide(b, "free_tool"), ToolAccess::Allowed); // default allow
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

    // ─── Exhaustive wildcard_match matrix ──────────────────────────────────

    #[test]
    fn wildcard_all_star_matches_everything_including_empty_and_long() {
        assert!(wildcard_match("*", ""));
        assert!(wildcard_match("*", "x"));
        let long_text = "a".repeat(5000);
        assert!(wildcard_match("*", &long_text));
    }

    #[test]
    fn wildcard_single_char_edge_cases() {
        // `?` requires exactly one char — empty text never matches a bare `?`.
        assert!(!wildcard_match("?", ""));
        assert!(wildcard_match("?", "x"));
        assert!(!wildcard_match("?", "xy"));
        // `??` matches exactly two chars, no more, no fewer.
        assert!(wildcard_match("??", "xy"));
        assert!(!wildcard_match("??", "x"));
        assert!(!wildcard_match("??", "xyz"));
    }

    #[test]
    fn wildcard_placement_start_middle_end() {
        // Wildcard only at the end.
        assert!(wildcard_match("gmail_*", "gmail_send"));
        assert!(!wildcard_match("gmail_*", "xgmail_send"));
        // Wildcard only at the start.
        assert!(wildcard_match("*_send", "gmail_send"));
        assert!(!wildcard_match("*_send", "gmail_sendx"));
        // Wildcard in the middle only.
        assert!(wildcard_match("a*z", "az"));
        assert!(wildcard_match("a*z", "axyz"));
        assert!(!wildcard_match("a*z", "ax"));
        assert!(!wildcard_match("a*z", "za"));
    }

    #[test]
    fn wildcard_match_itself_is_case_sensitive_callers_must_lowercase() {
        // `wildcard_match` documents that it is case-sensitive and relies on
        // callers (`get_stance`) to lowercase both sides first — verify that
        // contract holds at this layer.
        assert!(!wildcard_match("ABC", "abc"));
        assert!(wildcard_match("ABC", "ABC"));
    }

    #[test]
    fn wildcard_unicode_tool_names_match_by_char_not_byte() {
        // Multi-byte chars must be matched as whole `char`s (the impl collects
        // into `Vec<char>`), not sliced mid-codepoint — would panic/misbehave
        // if it indexed into the raw UTF-8 bytes instead.
        assert!(wildcard_match("日本*", "日本語_ツール"));
        assert!(wildcard_match("*ツール", "日本語_ツール"));
        assert!(wildcard_match("日本?", "日本語"));
        assert!(!wildcard_match("日本?", "日本語語"));
    }

    #[test]
    fn wildcard_extremely_long_strings_do_not_panic() {
        let pattern = format!("{}*", "a".repeat(4000));
        let text = format!("{}{}", "a".repeat(4000), "b".repeat(4000));
        assert!(wildcard_match(&pattern, &text));

        // All-`?` pattern must match only the exact same length.
        let q_pattern = "?".repeat(3000);
        assert!(wildcard_match(&q_pattern, &"x".repeat(3000)));
        assert!(!wildcard_match(&q_pattern, &"x".repeat(2999)));
        assert!(!wildcard_match(&q_pattern, &"x".repeat(3001)));
    }

    #[test]
    fn wildcard_empty_pattern_vs_empty_and_nonempty_text() {
        assert!(wildcard_match("", ""));
        assert!(!wildcard_match("", "x"));
        // Non-empty pattern (no wildcard) vs empty text never matches.
        assert!(!wildcard_match("x", ""));
    }

    // ─── get_stance combinatorics ──────────────────────────────────────────

    #[test]
    fn get_stance_all_three_stances_on_identical_pattern_block_wins_either_order() {
        let a = Uuid::new_v4();
        // Same exact pattern (`*`), three different stances registered for it —
        // block must win regardless of the order the rules were inserted in.
        let order1 = ctx(
            vec![
                rule(a, "*", Stance::Allow),
                rule(a, "*", Stance::Ask),
                rule(a, "*", Stance::Block),
            ],
            &[],
        );
        let order2 = ctx(
            vec![
                rule(a, "*", Stance::Block),
                rule(a, "*", Stance::Ask),
                rule(a, "*", Stance::Allow),
            ],
            &[],
        );
        assert_eq!(order1.get_stance(a, "anything"), Stance::Block);
        assert_eq!(order2.get_stance(a, "anything"), Stance::Block);
    }

    #[test]
    fn get_stance_duplicate_exact_pattern_different_stances_is_priority_not_last_write() {
        // Two rules for the *exact same* pattern+connector but different
        // stances (e.g. from an inconsistent bulk-update). The engine does not
        // dedupe by pattern — it collects every matching stance and picks by
        // block > ask > allow priority, NOT by insertion/"last write" order.
        let a = Uuid::new_v4();
        let allow_first = ctx(
            vec![
                rule(a, "gmail_send", Stance::Allow),
                rule(a, "gmail_send", Stance::Block),
            ],
            &[],
        );
        let block_first = ctx(
            vec![
                rule(a, "gmail_send", Stance::Block),
                rule(a, "gmail_send", Stance::Allow),
            ],
            &[],
        );
        assert_eq!(allow_first.get_stance(a, "gmail_send"), Stance::Block);
        assert_eq!(block_first.get_stance(a, "gmail_send"), Stance::Block);

        // Ask vs allow duplicate — ask (higher priority) wins in both orders.
        let ask_allow = ctx(
            vec![rule(a, "x", Stance::Ask), rule(a, "x", Stance::Allow)],
            &[],
        );
        let allow_ask = ctx(
            vec![rule(a, "x", Stance::Allow), rule(a, "x", Stance::Ask)],
            &[],
        );
        assert_eq!(ask_allow.get_stance(a, "x"), Stance::Ask);
        assert_eq!(allow_ask.get_stance(a, "x"), Stance::Ask);
    }

    #[test]
    fn get_stance_ignores_disabled_connectors_short_circuit_lives_in_caller() {
        // `get_stance` does NOT consult `disabled_connectors` at all — the
        // enabled/disabled short-circuit is applied by callers (e.g.
        // aggregator.rs checks `is_connector_enabled` BEFORE ever calling
        // `get_stance`). So a disabled connector's tool_rules are still fully
        // evaluated here if you call get_stance directly; disabling does not
        // implicitly force Block. This documents where the short-circuit
        // actually lives.
        let a = Uuid::new_v4();
        let disabled_but_explicit_allow = ctx(vec![rule(a, "gmail_send", Stance::Allow)], &[a]);
        assert!(!disabled_but_explicit_allow.is_connector_enabled(a));
        assert_eq!(
            disabled_but_explicit_allow.get_stance(a, "gmail_send"),
            Stance::Allow
        );

        // Even with a disabled connector and zero rules, get_stance defaults
        // to Allow — disabling alone is invisible to get_stance.
        let disabled_no_rules = ctx(vec![], &[a]);
        assert_eq!(disabled_no_rules.get_stance(a, "anything"), Stance::Allow);
    }

    #[test]
    fn get_stance_empty_tool_name_and_pattern_longer_than_tool() {
        let a = Uuid::new_v4();
        // `*` matches an empty tool name too.
        let block_all = ctx(vec![rule(a, "*", Stance::Block)], &[]);
        assert_eq!(block_all.get_stance(a, ""), Stance::Block);

        // A literal (non-wildcard) pattern longer than the tool name can never
        // match — falls through to the default Allow.
        let c = ctx(
            vec![rule(a, "gmail_send_email_extra_suffix", Stance::Block)],
            &[],
        );
        assert_eq!(c.get_stance(a, "gmail_send"), Stance::Allow);
    }

    #[test]
    fn get_stance_case_insensitive_on_both_pattern_and_tool_name() {
        let a = Uuid::new_v4();
        let c = ctx(vec![rule(a, "GmAiL_SeNd_*", Stance::Block)], &[]);
        assert_eq!(c.get_stance(a, "gmail_send_email"), Stance::Block);
        assert_eq!(c.get_stance(a, "GMAIL_SEND_EMAIL"), Stance::Block);
    }

    #[test]
    fn get_stance_unicode_tool_name() {
        let a = Uuid::new_v4();
        let c = ctx(vec![rule(a, "日本_*", Stance::Ask)], &[]);
        assert_eq!(c.get_stance(a, "日本_ツール"), Stance::Ask);
        assert_eq!(c.get_stance(a, "other_tool"), Stance::Allow);
    }
}

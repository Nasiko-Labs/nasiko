//! Per-agent permission engine.
//!
//! Two levels, mirroring Claude Desktop's Connectors UI (and the PoC's
//! `permissions.py`):
//!   1. **Server toggle** — can this agent use `gmail` / `serpapi` at all?
//!   2. **Tool stance** — within an allowed server, each tool is
//!      `allow | ask | block`, with glob patterns (`*`, `GMAIL_*`,
//!      `GMAIL_SEND_EMAIL`) and priority `block > ask > allow`.
//!
//! Default (no rows): every server enabled, every tool allowed — opt-in
//! restrictions only. The [`PermissionContext`] is computed once per gateway
//! request and Redis-cached (`mcp:perm:{user}:{agent}`, short TTL), deleted
//! immediately on any permission write so intentional changes take effect at
//! once. Its `hash` is the cross-process cache-invalidation signal: it feeds the
//! manifest cache key, so a permission change forces a fresh, re-filtered tool
//! list with no other signalling.

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

/// One sub-tool permission rule.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionRule {
    pub server_name: String,
    /// Glob pattern: `*` | `GMAIL_*` | `GMAIL_SEND_EMAIL`.
    pub tool_pattern: String,
    pub stance: Stance,
}

/// Pre-loaded, immutable permission state for one `(user_id, agent_id)` pair.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionContext {
    pub user_id: Uuid,
    pub agent_id: Uuid,
    /// Server names explicitly disabled (`enabled = false`).
    pub disabled_servers: HashSet<String>,
    pub rules: Vec<PermissionRule>,
    /// sha256[:16] of the rules + disabled set — the manifest cache-key signal.
    pub hash: String,
}

impl PermissionContext {
    /// True unless the server is explicitly disabled. Default: enabled.
    pub fn is_server_enabled(&self, server_name: &str) -> bool {
        !self.disabled_servers.contains(server_name)
    }

    /// Resolve the stance for `(server, tool)`. Priority `block > ask > allow`;
    /// default `allow` when no rule matches. Matching is case-insensitive glob.
    pub fn get_stance(&self, server_name: &str, tool_name: &str) -> Stance {
        let tool_lower = tool_name.to_ascii_lowercase();
        let matching: Vec<Stance> = self
            .rules
            .iter()
            .filter(|r| {
                r.server_name == server_name
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

    /// True when any server toggle or tool rule is set.
    pub fn has_any_restriction(&self) -> bool {
        !self.disabled_servers.is_empty() || !self.rules.is_empty()
    }
}

/// Extract the Composio toolkit slug from a tool slug:
/// `GMAIL_SEND_EMAIL` → `gmail`, `GOOGLECALENDAR_CREATE_EVENT` → `googlecalendar`.
pub fn toolkit_from_composio_slug(slug: &str) -> String {
    slug.split('_').next().unwrap_or("").to_ascii_lowercase()
}

fn perm_cache_key(user_id: Uuid, agent_id: Uuid) -> String {
    format!("mcp:perm:{user_id}:{agent_id}")
}

/// Load the permission context for `(user_id, agent_id)`: Redis cache hit → 0 DB
/// reads; miss → two parallel reads, cached for the TTL.
pub async fn load_permission_context(
    state: &McpState,
    user_id: Uuid,
    agent_id: Uuid,
) -> Result<PermissionContext> {
    let key = perm_cache_key(user_id, agent_id);
    if let Some(ctx) = cache::get_json::<PermissionContext>(&state.redis, &key).await {
        return Ok(ctx);
    }

    let (server_rows, tool_rows) = tokio::try_join!(
        repo::get_agent_server_access(&state.db, user_id, agent_id),
        repo::get_agent_tool_permissions(&state.db, user_id, agent_id),
    )?;

    let disabled_servers: HashSet<String> = server_rows
        .iter()
        .filter(|r| !r.enabled)
        .map(|r| r.server_name.clone())
        .collect();

    let rules: Vec<PermissionRule> = tool_rows
        .iter()
        .filter_map(|r| {
            Stance::from_str(&r.stance).map(|stance| PermissionRule {
                server_name: r.server_name.clone(),
                tool_pattern: r.tool_pattern.clone(),
                stance,
            })
        })
        .collect();

    let hash = compute_hash(&rules, &disabled_servers);
    let ctx = PermissionContext { user_id, agent_id, disabled_servers, rules, hash };

    cache::set_json_ex(&state.redis, &key, &ctx, state.config.perm_cache_ttl_seconds).await;
    Ok(ctx)
}

/// Drop the cached permission context for a `(user, agent)` pair. Call this
/// immediately after any write to the agent's server-access / tool-permission
/// rows so the next request reads fresh data.
pub async fn invalidate_permission_cache(state: &McpState, user_id: Uuid, agent_id: Uuid) {
    cache::delete(&state.redis, &perm_cache_key(user_id, agent_id)).await;
}

/// Deterministic sha256[:16] over the rules + disabled set. Byte-for-byte
/// compatible with the PoC's `_compute_hash` (sorted rule triples, then sorted
/// `("__disabled__", server, "block")` triples, compact-JSON, sha256, first 16
/// hex chars).
fn compute_hash(rules: &[PermissionRule], disabled: &HashSet<String>) -> String {
    let mut data: Vec<[String; 3]> = rules
        .iter()
        .map(|r| {
            [
                r.server_name.clone(),
                r.tool_pattern.clone(),
                r.stance.as_str().to_string(),
            ]
        })
        .collect();
    data.sort();

    let mut disabled_sorted: Vec<&String> = disabled.iter().collect();
    disabled_sorted.sort();
    for s in disabled_sorted {
        data.push(["__disabled__".to_string(), s.clone(), "block".to_string()]);
    }

    let raw = serde_json::to_string(&data).unwrap_or_default();
    sha256_hex16(raw.as_bytes())
}

/// First 16 hex chars of the SHA-256 of `bytes`. Shared with the manifest cache key.
pub(crate) fn sha256_hex16(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(bytes);
    let mut hex = hex::encode(digest);
    hex.truncate(16);
    hex
}

/// Case-sensitive glob match supporting `*` (any run, incl. empty) and `?`
/// (one char) — the `fnmatch` subset the PoC's patterns use. Callers lowercase
/// both sides for case-insensitivity.
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

/// `GET /api/mcp/agents/{agent_id}/servers` view: all servers visible to the
/// user with this agent's enabled/connected status.
pub async fn list_servers_view(state: &McpState, user_id: Uuid, agent_id: Uuid) -> Result<Value> {
    let access = repo::get_agent_server_access(&state.db, user_id, agent_id).await?;
    let access_map: HashMap<String, bool> = access.into_iter().map(|r| (r.server_name, r.enabled)).collect();

    let active_conns = repo::list_connections_by_user(&state.db, user_id, Some("ACTIVE")).await?;
    let connected_toolkits: HashSet<String> = active_conns.into_iter().map(|c| c.toolkit).collect();

    let creds = repo::get_user_credentials_for_user(&state.db, user_id).await?;
    let tokens = repo::get_mcp_oauth_tokens_for_user(&state.db, user_id).await?;
    let cred_ids: HashSet<Uuid> = creds.into_iter().map(|c| c.mcp_server_id).collect();
    let token_ids: HashSet<Uuid> = tokens.into_iter().map(|t| t.mcp_server_id).collect();

    let mut entries: Vec<Value> = Vec::new();

    for ac in repo::list_platform_auth_configs(&state.db).await? {
        entries.push(json!({
            "server_name": ac.toolkit,
            "server_type": "composio",
            "enabled": access_map.get(&ac.toolkit).copied().unwrap_or(true),
            "connected": connected_toolkits.contains(&ac.toolkit),
            "display_name": ac.display_name.unwrap_or_else(|| crate::catalog::capitalize(&ac.toolkit)),
            "logo_url": ac.logo_url,
        }));
    }
    for s in repo::list_mcp_servers_for_user(&state.db, user_id).await? {
        let connected = cred_ids.contains(&s.id) || token_ids.contains(&s.id) || s.auth_type == "none";
        entries.push(json!({
            "server_name": s.name,
            "server_type": "mcp",
            "enabled": access_map.get(&s.name).copied().unwrap_or(true),
            "connected": connected,
            "display_name": s.display_name.unwrap_or_else(|| crate::catalog::capitalize(&s.name)),
            "logo_url": s.logo_url,
        }));
    }

    Ok(json!({ "data": entries }))
}

/// `PUT /api/mcp/agents/{agent_id}/servers/{server}` view: toggle a server for
/// the agent.
pub async fn set_server_access_view(
    state: &McpState,
    user_id: Uuid,
    agent_id: Uuid,
    server: &str,
    enabled: bool,
) -> Result<Value> {
    // Determine the server type (and that it exists).
    let server_type = if repo::get_platform_auth_config_by_toolkit(&state.db, server).await?.is_some() {
        "composio"
    } else if repo::get_platform_mcp_server_by_name(&state.db, server).await?.is_some()
        || repo::get_user_mcp_server_by_name(&state.db, user_id, server).await?.is_some()
    {
        "mcp"
    } else {
        return Err(McpError::NotFound(format!("server '{server}' not found")));
    };

    let row = repo::upsert_agent_server_access(&state.db, user_id, agent_id, server, server_type, enabled).await?;
    invalidate_permission_cache(state, user_id, agent_id).await;

    Ok(json!({ "server_name": row.server_name, "server_type": row.server_type, "enabled": row.enabled }))
}

/// `GET /api/mcp/agents/{agent_id}/servers/{server}/tools` view: tools for a
/// server with this agent's current stance per tool.
pub async fn list_server_tools_view(state: &McpState, user_id: Uuid, agent_id: Uuid, server: &str) -> Result<Value> {
    let perms = load_permission_context(state, user_id, agent_id).await?;

    // Collect (name, description) pairs from the right source.
    let tools: Vec<(String, Option<String>)> =
        if repo::get_platform_auth_config_by_toolkit(&state.db, server).await?.is_some() {
            match &state.providers.composio {
                Some(provider) => provider
                    .list_toolkit_tools(server)
                    .await?
                    .into_iter()
                    .map(|t| (t.name, t.description))
                    .collect(),
                None => Vec::new(),
            }
        } else {
            // Generic MCP server: build its config (with the user's creds) and probe it.
            let built = crate::credentials::build_generic_servers(state, user_id).await?;
            match built.iter().find(|s| s.name == server) {
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

    let out: Vec<Value> = tools
        .into_iter()
        .map(|(name, description)| {
            let stance = perms.get_stance(server, &name);
            json!({ "name": name, "description": description, "stance": stance.as_str() })
        })
        .collect();

    Ok(json!({ "data": out }))
}

/// `GET /api/mcp/agents/{agent_id}/tools` view: the agent's current tool
/// rules.
pub async fn list_tool_rules_view(state: &McpState, user_id: Uuid, agent_id: Uuid) -> Result<Value> {
    let rows = repo::get_agent_tool_permissions(&state.db, user_id, agent_id).await?;
    let data: Vec<Value> = rows
        .into_iter()
        .map(|r| json!({ "server_name": r.server_name, "tool_pattern": r.tool_pattern, "stance": r.stance }))
        .collect();
    Ok(json!({ "data": data }))
}

/// One rule input for [`bulk_update_tools`].
pub struct ToolRuleInput {
    pub server_name: String,
    pub tool_pattern: String,
    pub stance: String,
}

/// `PUT /api/mcp/agents/{agent_id}/tools` view: batch upsert tool permission
/// rules. Validates every stance before writing any of them.
pub async fn bulk_update_tools(state: &McpState, user_id: Uuid, agent_id: Uuid, rules: &[ToolRuleInput]) -> Result<Value> {
    for rule in rules {
        if !STANCES.contains(&rule.stance.as_str()) {
            return Err(McpError::BadRequest(format!("stance must be one of {STANCES:?}")));
        }
    }

    let mut applied = Vec::with_capacity(rules.len());
    for rule in rules {
        let row = repo::upsert_agent_tool_permission(
            &state.db,
            user_id,
            agent_id,
            &rule.server_name,
            &rule.tool_pattern,
            &rule.stance,
        )
        .await?;
        applied
            .push(json!({ "server_name": row.server_name, "tool_pattern": row.tool_pattern, "stance": row.stance }));
    }
    invalidate_permission_cache(state, user_id, agent_id).await;

    Ok(json!({ "data": applied }))
}

/// `DELETE /api/mcp/agents/{agent_id}/permissions` — reset to all-allowed.
/// Returns the number of rows deleted.
pub async fn reset(state: &McpState, user_id: Uuid, agent_id: Uuid) -> Result<u64> {
    let deleted = repo::delete_all_agent_permissions(&state.db, user_id, agent_id).await?;
    invalidate_permission_cache(state, user_id, agent_id).await;
    Ok(deleted)
}

#[cfg(test)]
mod management_tests {
    use super::STANCES;

    #[test]
    fn stances_are_exactly_allow_ask_block() {
        assert_eq!(STANCES, ["allow", "ask", "block"]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wildcard_semantics() {
        assert!(wildcard_match("*", "anything"));
        assert!(wildcard_match("gmail_*", "gmail_send_email"));
        assert!(wildcard_match("gmail_send_email", "gmail_send_email"));
        assert!(!wildcard_match("gmail_*", "slack_post"));
        assert!(wildcard_match("gmail_?end", "gmail_send"));
        assert!(!wildcard_match("gmail_?end", "gmail_bend_x"));
    }

    #[test]
    fn stance_priority_block_over_ask_over_allow() {
        let ctx = PermissionContext {
            user_id: Uuid::nil(),
            agent_id: Uuid::nil(),
            disabled_servers: HashSet::new(),
            rules: vec![
                PermissionRule { server_name: "gmail".into(), tool_pattern: "GMAIL_*".into(), stance: Stance::Allow },
                PermissionRule { server_name: "gmail".into(), tool_pattern: "GMAIL_SEND_EMAIL".into(), stance: Stance::Block },
            ],
            hash: String::new(),
        };
        assert_eq!(ctx.get_stance("gmail", "GMAIL_SEND_EMAIL"), Stance::Block);
        assert_eq!(ctx.get_stance("gmail", "GMAIL_FETCH_EMAILS"), Stance::Allow);
        assert_eq!(ctx.get_stance("gmail", "UNMATCHED"), Stance::Allow);
    }

    #[test]
    fn toolkit_slug_extraction() {
        assert_eq!(toolkit_from_composio_slug("GMAIL_SEND_EMAIL"), "gmail");
        assert_eq!(toolkit_from_composio_slug("GOOGLECALENDAR_CREATE_EVENT"), "googlecalendar");
    }
}

#[cfg(test)]
mod edge_tests {
    use super::*;

    #[test]
    fn wildcard_edge_cases() {
        // Empty pattern only matches empty text.
        assert!(wildcard_match("", ""));
        assert!(!wildcard_match("", "x"));
        // Star matches empty and anything.
        assert!(wildcard_match("*", ""));
        assert!(wildcard_match("**", "abc"));
        // Leading/trailing/middle stars.
        assert!(wildcard_match("*email", "gmail_send_email"));
        assert!(wildcard_match("gmail*email", "gmail_send_email"));
        assert!(wildcard_match("*send*", "gmail_send_email"));
        // Question mark is exactly one char.
        assert!(wildcard_match("a?c", "abc"));
        assert!(!wildcard_match("a?c", "ac"));
        assert!(!wildcard_match("a?c", "abbc"));
        // Star + question mark combined.
        assert!(wildcard_match("g*_?end_*", "gmail_send_email"));
        // No match.
        assert!(!wildcard_match("gmail_*", "slack_post"));
    }

    fn ctx(rules: Vec<PermissionRule>, disabled: &[&str]) -> PermissionContext {
        PermissionContext {
            user_id: Uuid::nil(),
            agent_id: Uuid::nil(),
            disabled_servers: disabled.iter().map(|s| s.to_string()).collect(),
            rules,
            hash: String::new(),
        }
    }
    fn rule(server: &str, pat: &str, stance: Stance) -> PermissionRule {
        PermissionRule { server_name: server.into(), tool_pattern: pat.into(), stance }
    }

    #[test]
    fn stance_default_allow_and_server_scoping() {
        let c = ctx(vec![rule("gmail", "*", Stance::Block)], &[]);
        // Rule on gmail must not affect slack.
        assert_eq!(c.get_stance("slack", "SLACK_POST"), Stance::Allow);
        assert_eq!(c.get_stance("gmail", "ANYTHING"), Stance::Block);
    }

    #[test]
    fn stance_priority_across_overlapping_patterns() {
        // allow on *, ask on GMAIL_SEND_*, block on GMAIL_SEND_EMAIL.
        let c = ctx(
            vec![
                rule("gmail", "*", Stance::Allow),
                rule("gmail", "gmail_send_*", Stance::Ask),
                rule("gmail", "gmail_send_email", Stance::Block),
            ],
            &[],
        );
        assert_eq!(c.get_stance("gmail", "GMAIL_SEND_EMAIL"), Stance::Block); // block wins
        assert_eq!(c.get_stance("gmail", "GMAIL_SEND_SMS"), Stance::Ask); // ask beats allow
        assert_eq!(c.get_stance("gmail", "GMAIL_READ"), Stance::Allow);
    }

    #[test]
    fn is_server_enabled_and_has_restriction() {
        let c = ctx(vec![], &["discord"]);
        assert!(!c.is_server_enabled("discord"));
        assert!(c.is_server_enabled("gmail"));
        assert!(c.has_any_restriction());
        assert!(!ctx(vec![], &[]).has_any_restriction());
    }

    #[test]
    fn hash_is_deterministic_and_order_independent() {
        let disabled: std::collections::HashSet<String> = ["a", "b"].iter().map(|s| s.to_string()).collect();
        let r1 = vec![rule("gmail", "A", Stance::Block), rule("slack", "B", Stance::Ask)];
        let r2 = vec![rule("slack", "B", Stance::Ask), rule("gmail", "A", Stance::Block)];
        // Same rules in different order → identical hash.
        assert_eq!(compute_hash(&r1, &disabled), compute_hash(&r2, &disabled));
        // A different stance → different hash.
        let r3 = vec![rule("gmail", "A", Stance::Allow), rule("slack", "B", Stance::Ask)];
        assert_ne!(compute_hash(&r1, &disabled), compute_hash(&r3, &disabled));
        // Adding a disabled server → different hash.
        let more: std::collections::HashSet<String> = ["a", "b", "c"].iter().map(|s| s.to_string()).collect();
        assert_ne!(compute_hash(&r1, &disabled), compute_hash(&r1, &more));
        // Hash length is 16 hex chars.
        assert_eq!(compute_hash(&r1, &disabled).len(), 16);
    }

    #[test]
    fn toolkit_slug_edges() {
        assert_eq!(toolkit_from_composio_slug("GMAIL_SEND_EMAIL"), "gmail");
        assert_eq!(toolkit_from_composio_slug("NOUNDERSCORE"), "nounderscore");
        assert_eq!(toolkit_from_composio_slug(""), "");
    }
}

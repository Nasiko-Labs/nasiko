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
use crate::entitlements::{ToolSet, UserEntitlement};
use crate::error::{McpError, Result};
use crate::provider::generic::LIST_TIMEOUT;
use crate::repo;
use crate::search::ToolSearchIndex;
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

/// The agent's [`PermissionContext`] narrowed by the caller's
/// [`UserEntitlement`]. Every gate on a tool (listing, search, call) goes
/// through [`EffectiveAccess::decide`], so a user's entitlement is checked
/// BEFORE the agent's stance and before any approval (HITL) logic: an approval
/// or session grant can never widen it.
#[derive(Debug, Clone, Copy)]
pub struct EffectiveAccess<'a> {
    pub perms: &'a PermissionContext,
    pub entitlement: &'a UserEntitlement,
}

/// Shared by [`EffectiveAccess::unrestricted`] so it can hand out a reference.
static UNRESTRICTED: UserEntitlement = UserEntitlement::Unrestricted;

impl<'a> EffectiveAccess<'a> {
    pub fn new(perms: &'a PermissionContext, entitlement: &'a UserEntitlement) -> Self {
        Self { perms, entitlement }
    }

    /// Agent permissions only (no per-user restriction).
    pub fn unrestricted(perms: &'a PermissionContext) -> Self {
        Self::new(perms, &UNRESTRICTED)
    }

    /// Entitlement first, then the agent's decision.
    pub fn decide(&self, connector_id: Uuid, tool_name: &str) -> ToolAccess {
        if !self.entitlement.allows(connector_id, tool_name) {
            return ToolAccess::Denied;
        }
        self.perms.decide(connector_id, tool_name)
    }

    /// Enabled for the agent AND the user has at least one tool on it.
    pub fn is_connector_enabled(&self, connector_id: Uuid) -> bool {
        self.perms.is_connector_enabled(connector_id)
            && self.entitlement.allows_connector(connector_id)
    }

    /// True when the user's tools are restricted.
    pub fn is_restricted(&self) -> bool {
        self.entitlement.is_restricted()
    }

    /// Manifest cache-key signal covering both inputs. Unrestricted keeps the
    /// agent hash unchanged, so OSS cache keys are exactly as before.
    pub fn hash(&self) -> String {
        if !self.is_restricted() {
            return self.perms.hash.clone();
        }
        let combined = format!("{}:{}", self.perms.hash, self.entitlement.hash());
        sha256_hex16(combined.as_bytes())
    }
}

/// Toolkit slug of Composio's cross-toolkit meta-tools (`COMPOSIO_SEARCH_TOOLS`, ...),
/// which map to no connector.
pub const COMPOSIO_META_TOOLKIT: &str = "composio";

/// The only meta-tools a user under a restricted entitlement may list or call.
/// Composio's router can also expose code-running meta-tools (a remote
/// workbench or shell) that reach every connected toolkit with the user's
/// credentials, which no per-slug check can attribute. Exact, case-sensitive
/// names; the two batch tools are still argument-filtered at `tools/call`.
const RESTRICTED_COMPOSIO_META_TOOLS: [&str; 4] = [
    "COMPOSIO_SEARCH_TOOLS",
    "COMPOSIO_GET_TOOL_SCHEMAS",
    "COMPOSIO_MULTI_EXECUTE_TOOL",
    "COMPOSIO_MANAGE_CONNECTIONS",
];

/// May a restricted user see/call the toolkit-`composio` tool `name`?
pub fn restricted_composio_meta_tool_allowed(name: &str) -> bool {
    RESTRICTED_COMPOSIO_META_TOOLS.contains(&name)
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
    let mut enabled_connectors: HashSet<Uuid> = rows
        .iter()
        .filter(|r| r.enabled)
        .map(|r| r.connector_id)
        .collect();
    // Default-on system connectors: a `provider_type = 'system'` connector with
    // `auto_enable = true` the agent's owner can reach is enabled for this agent
    // without any `mcp_agent_connector_access` row. A union at this one chokepoint —
    // the set feeds both `tools/list` (aggregator) and `tools/call` (protocol)
    // through `decide`, and `compute_hash` below folds the unioned ids so the
    // manifest cache busts correctly. An access row that switches the connector
    // off wins over the default, or the connector could never be turned off.
    let switched_off: HashSet<Uuid> = rows
        .iter()
        .filter(|r| !r.enabled)
        .map(|r| r.connector_id)
        .collect();
    let (system, toolkits) = tokio::try_join!(
        repo::list_auto_enabled_system_connector_ids(&state.db, agent_id),
        repo::list_coding_agent_owner_toolkit_ids(&state.db, agent_id),
    )?;
    enabled_connectors.extend(
        system
            .into_iter()
            .chain(toolkits)
            .filter(|id| !switched_off.contains(id)),
    );
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
    entitlement: &UserEntitlement,
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
            (connected.contains(&c.id) || c.auth_type.as_deref() == Some("none"))
                // A connector the caller has no tool on does not exist for them.
                && entitlement.allows_connector(c.id)
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

/// Where the caller stands with one connector an agent uses: whether its tools can run for
/// them, and if not, what they have to do. Every tool call runs on the calling user's own
/// connection — never the agent owner's — so this is per viewer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CallerConnection {
    /// Needs no credential (`auth_type = "none"`).
    Ready,
    /// The caller has connected their own account.
    Connected,
    /// The caller can reach the connector but has not connected their own account.
    NeedsConnection,
    /// The connector is not shared with the caller, so they cannot connect it yet.
    NotShared,
}

impl CallerConnection {
    /// Pure: the caller's standing from what is known about them and the connector. The
    /// connector owner's static headers deliberately do not count (see
    /// `credentials::static_headers_usable_by`): a caller without their own connection needs one.
    pub fn of(needs_credential: bool, reachable: bool, connected: bool) -> Self {
        match (reachable, needs_credential, connected) {
            (false, _, _) => Self::NotShared,
            (true, false, _) => Self::Ready,
            (true, true, true) => Self::Connected,
            (true, true, false) => Self::NeedsConnection,
        }
    }
}

/// Whether running `c`'s tools takes a connection of the caller's own: every Composio toolkit
/// does, and every custom server except an `auth_type = "none"` one.
fn needs_own_credential(c: &repo::McpConnector) -> bool {
    c.is_composio() || c.auth_type.as_deref().is_some_and(|a| a != "none")
}

/// `GET /agents/{id}/my-connections` view: the connectors enabled on this agent, each with the
/// caller's own [`CallerConnection`] — for someone the agent is shared with, which of its tools
/// need them to connect their own account. Names nothing about anyone else's connections.
pub async fn list_caller_connections_view(
    state: &McpState,
    user_id: Uuid,
    agent_id: Uuid,
) -> Result<Value> {
    let enabled: Vec<Uuid> = repo::get_agent_connector_access(&state.db, agent_id)
        .await?
        .into_iter()
        .filter(|row| row.enabled)
        .map(|row| row.connector_id)
        .collect();
    let connected: HashSet<Uuid> = repo::list_user_connections(&state.db, user_id, Some("ACTIVE"))
        .await?
        .into_iter()
        .map(|c| c.connector_id)
        .collect();

    let mut data = Vec::with_capacity(enabled.len());
    for connector_id in enabled {
        let Some(c) = repo::get_connector_by_id(&state.db, connector_id).await? else {
            continue;
        };
        if !c.active() {
            continue;
        }
        let reachable = state
            .authorizer
            .can_access_connector(&state.db, user_id, c.id)
            .await?;
        let standing = CallerConnection::of(
            needs_own_credential(&c),
            reachable,
            connected.contains(&c.id),
        );
        data.push(json!({
            "connector_id": c.id,
            "provider_type": c.provider_type,
            "name": c.name,
            "display_name": c.display_name.unwrap_or_else(|| crate::catalog::capitalize(&c.name)),
            "logo_url": c.logo_url,
            "auth_type": c.auth_type,
            "status": standing,
        }));
    }
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
    entitlement: &UserEntitlement,
) -> Result<Value> {
    if !entitlement.allows_connector(connector_id) {
        return Err(connector_not_found(connector_id));
    }
    // Toggling the connector also switches every tool the caller cannot see,
    // the same reason `reset` is refused for restricted callers.
    if is_filtered(entitlement, connector_id) {
        return Err(McpError::Forbidden(
            "a tool-restricted user cannot enable or disable a connector they only partly see"
                .into(),
        ));
    }
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
    if enabled {
        tokio::spawn(sync_catalog_if_empty(state.clone(), user_id, connector_id));
    }
    Ok(json!({ "connector_id": row.connector_id, "enabled": row.enabled }))
}

/// Fill a never-synced connector's tool catalog. Search and the connector list find a
/// connector's tools only once they are in the catalog, and enabling a connector on an agent
/// (`nasiko mcp agent tools enable`) or connecting an account is often the only step a caller
/// takes — nothing else guarantees the tool list is ever opened. Runs off the request, since a
/// live `tools/list` can take `LIST_TIMEOUT`. Best-effort: the toggle or connection is already
/// saved, so an unreachable backend only logs, and the tool list view retries the sync on its
/// next read.
pub async fn sync_catalog_if_empty(state: McpState, user_id: Uuid, connector_id: Uuid) {
    let synced = match repo::list_connector_tools(&state.db, connector_id).await {
        Ok(catalog) if !catalog.is_empty() => return,
        Ok(_) => sync_connector_tools(&state, user_id, connector_id).await,
        Err(e) => Err(e),
    };
    if let Err(e) = synced {
        tracing::warn!(%connector_id, error = %e, "tool catalog sync on connector enable failed");
    }
}

/// `GET /agents/{id}/connectors/{connector_id}/tools` — tools for a connector
/// with this agent's current stance. Reads the synced catalog; syncs live if empty.
pub async fn list_connector_tools_view(
    state: &McpState,
    user_id: Uuid,
    agent_id: Uuid,
    connector_id: Uuid,
    entitlement: &UserEntitlement,
) -> Result<Value> {
    if !entitlement.allows_connector(connector_id)
        || !state
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
    // (Composio via list_toolkit_tools, custom MCP via live tools/list), and
    // rebuild the search index with it: a flow-less caller (a local coding or
    // discovered agent) reaches tools only through `nasiko_search_tools`, so
    // tools synced here and nowhere else would stay unfindable until restart.
    let mut catalog = repo::list_connector_tools(&state.db, connector_id).await?;
    if catalog.is_empty() {
        sync_connector_tools_by_id(state, user_id, connector_id).await?;
        catalog = repo::list_connector_tools(&state.db, connector_id).await?;
    }
    let out: Vec<Value> = catalog
        .into_iter()
        .filter(|t| entitlement.allows(connector_id, &t.tool_name))
        .map(|t| {
            let stance = perms.get_stance(connector_id, &t.tool_name);
            json!({
                "name": t.tool_name,
                "description": t.description.as_deref().map(|d| summarize_description(d, DESCRIPTION_SUMMARY_MAX_CHARS)),
                "stance": stance.as_str(),
                "last_synced_at": t.last_synced_at,
            })
        })
        .collect();
    Ok(json!({ "tools": out }))
}

/// UI-facing description length cap — every place that shows a tool
/// description to a human (agent-scoped and connector-scoped views alike)
/// summarizes to this. The live agent-facing tool contract in
/// `aggregator.rs` always forwards the full text unchanged, since the LLM
/// needs the complete usage rules.
pub(crate) const DESCRIPTION_SUMMARY_MAX_CHARS: usize = 220;

/// Common abbreviations whose trailing `.` doesn't end a sentence. Checked
/// case-insensitively against the word immediately before the dot.
const ABBREVIATIONS: &[&str] = &[
    "e.g", "i.e", "etc", "vs", "cf", "approx", "fig", "no", "al", "eq", "resp", "dr", "mr", "mrs",
    "ms", "prof", "inc", "ltd", "st", "u.s", "u.k", "co",
];

/// Shortens a tool description for UI display: prefers the first paragraph
/// (most MCP tool descriptions lead with a one-line summary before a blank
/// line), else the first two sentences, always capped at `max_chars` and cut
/// on a word boundary. Sentence detection skips decimal numbers, initials,
/// abbreviations, and ellipses so it doesn't fragment mid-thought. Leaves
/// short descriptions completely unchanged. Leading Markdown headings (e.g.
/// `## Overview`, common in Notion's tool descriptions) carry no summary
/// content on their own, so they're skipped before picking a paragraph.
///
/// Shared by every UI-facing tools listing — currently
/// `list_connector_tools_view` here and `get_connector_view` in
/// `connectors.rs` — so a connector's tools look the same whether viewed
/// standalone or through a specific agent.
pub(crate) fn summarize_description(desc: &str, max_chars: usize) -> String {
    let desc = strip_leading_markdown_headings(desc.trim());

    // Paragraph preference comes first: a short first paragraph should win
    // even when the *whole* (multi-section) description would also fit
    // under `max_chars` — showing every section just because it's short
    // enough isn't the same as showing a summary.
    if let Some(idx) = desc.find("\n\n") {
        let paragraph = desc[..idx].trim();
        if !paragraph.is_empty() && paragraph.chars().count() <= max_chars {
            return paragraph.to_string();
        }
    }

    if desc.chars().count() <= max_chars {
        return desc.to_string();
    }

    let cut_at = match sentence_boundary(desc, 2) {
        Some(byte_idx) if byte_idx <= char_boundary_at(desc, max_chars) => byte_idx,
        _ => char_boundary_at(desc, max_chars),
    };

    let mut truncated = desc[..cut_at].trim_end().to_string();
    if truncated.len() < desc.trim_end().len() && !truncated.ends_with(['.', '!', '?']) {
        if let Some(last_space) = truncated.rfind(char::is_whitespace) {
            truncated.truncate(last_space);
        }
        truncated = truncated
            .trim_end_matches(|c: char| c.is_whitespace() || c == ',')
            .to_string();
        truncated.push('…');
    }
    truncated
}

/// Byte index of the `n`th sentence-ending punctuation (`.`/`!`/`?`) in
/// `text`, or `None` if fewer than `n` real sentence boundaries exist. A `.`
/// only counts when it's followed by whitespace/end-of-string *and* isn't
/// part of a decimal number, an abbreviation, or an initial. A run of
/// consecutive dots (`...`) counts as a single boundary.
fn sentence_boundary(text: &str, n: usize) -> Option<usize> {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut found = 0;
    let mut i = 0;

    while i < chars.len() {
        let (byte_idx, ch) = chars[i];
        if ch != '.' && ch != '!' && ch != '?' {
            i += 1;
            continue;
        }

        // Collapse a run of consecutive dots (ellipsis) into one boundary.
        let mut run_end = i;
        while run_end + 1 < chars.len() && chars[run_end + 1].1 == '.' {
            run_end += 1;
        }
        let is_ellipsis = run_end > i;

        let next_char = chars.get(run_end + 1).map(|&(_, c)| c);
        let followed_by_boundary = matches!(next_char, None | Some(' ') | Some('\n') | Some('\t'));
        if !followed_by_boundary {
            i = run_end + 1;
            continue;
        }

        if ch == '.' && !is_ellipsis && is_non_terminal_dot(text, byte_idx, next_char) {
            i = run_end + 1;
            continue;
        }

        found += 1;
        if found >= n {
            return Some(
                chars
                    .get(run_end + 1)
                    .map(|&(b, _)| b)
                    .unwrap_or(text.len()),
            );
        }
        i = run_end + 1;
    }
    None
}

/// True when the `.` at `byte_idx` is a decimal point, an initial (single
/// capital letter), or a known abbreviation — not a real sentence end.
fn is_non_terminal_dot(text: &str, byte_idx: usize, next_char: Option<char>) -> bool {
    let prev_char = text[..byte_idx].chars().next_back();

    // Decimal number: digit immediately before and after the dot.
    if prev_char.is_some_and(|c| c.is_ascii_digit())
        && next_char.is_some_and(|c| c.is_ascii_digit())
    {
        return true;
    }

    let word_start = text[..byte_idx]
        .rfind(|c: char| c.is_whitespace())
        .map(|p| p + 1)
        .unwrap_or(0);
    let word = &text[word_start..byte_idx];

    // Single-letter initial, e.g. "J." in "J. Smith".
    if word.chars().count() == 1 && word.chars().next().is_some_and(char::is_uppercase) {
        return true;
    }

    ABBREVIATIONS.iter().any(|a| a.eq_ignore_ascii_case(word))
}

/// Byte index of the char boundary at or before the `n`th character of `s`.
fn char_boundary_at(s: &str, n: usize) -> usize {
    s.char_indices().nth(n).map(|(b, _)| b).unwrap_or(s.len())
}

/// Skips leading paragraphs whose first line is a bare Markdown heading
/// (`#` through `######` followed by a space, e.g. `## Overview`) — these
/// are section labels, not summary text, so picking one as "the first
/// paragraph" would show something meaningless like just `## Overview`.
fn strip_leading_markdown_headings(desc: &str) -> &str {
    let mut rest = desc.trim_start();
    while is_markdown_heading_line(first_line(rest)) {
        match rest.find("\n\n") {
            Some(idx) => rest = rest[idx + 2..].trim_start(),
            None => return "",
        }
    }
    rest
}

fn first_line(text: &str) -> &str {
    &text[..text.find('\n').unwrap_or(text.len())]
}

fn is_markdown_heading_line(line: &str) -> bool {
    let after_hashes = line.trim_end().trim_start_matches('#');
    after_hashes.len() != line.trim_end().len() // at least one leading '#'
        && after_hashes.starts_with(' ')
}

/// Public entry point for eager tool sync (e.g. after OAuth callback).
/// `sync_connector_tools` itself rebuilds the search index, so new/changed
/// tools are discoverable as soon as this returns.
pub async fn sync_connector_tools_by_id(
    state: &McpState,
    user_id: Uuid,
    connector_id: Uuid,
) -> Result<()> {
    sync_connector_tools(state, user_id, connector_id).await
}

/// Rebuild the tool search index after `mcp_connector_tools` changed, so
/// `nasiko_search_tools` sees the change without a server restart.
/// Never fails the caller: the catalog write already succeeded, and an
/// embedding outage must not turn a connector sync into an error.
pub(crate) async fn refresh_search_index(index: &dyn ToolSearchIndex, db: &sqlx::PgPool) {
    if let Err(e) = index.rebuild(db).await {
        tracing::warn!(error = %e, "search index rebuild after tool catalog change failed");
    }
}

/// Sync a connector's tool catalog from its live backend into `mcp_connector_tools`.
async fn sync_connector_tools(state: &McpState, user_id: Uuid, connector_id: Uuid) -> Result<()> {
    let connector = repo::get_connector_by_id(&state.db, connector_id)
        .await?
        .ok_or_else(|| McpError::NotFound(format!("connector '{connector_id}' not found")))?;

    // (name, description, input_schema) — schema is only ever used as
    // LLM-fallback signal below, never persisted.
    let mut tools: Vec<(String, Option<String>, Option<Value>)> = if connector.is_composio() {
        match state.providers.composio.get() {
            Some(p) => p
                .list_toolkit_tools(&connector.name)
                .await?
                .into_iter()
                .map(|t| (t.name, t.description, t.input_schema))
                .collect(),
            None => Vec::new(),
        }
    } else {
        let built = crate::credentials::build_generic_servers(state, user_id).await?;
        match built
            .configs
            .iter()
            .find(|s| s.connector_id == connector_id)
        {
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
                            t.get("inputSchema").cloned(),
                        )
                    })
                })
                .collect(),
            None => Vec::new(),
        }
    };

    if !tools.is_empty() {
        // LLM fallback — only reached for the subset that came back from the
        // live backend above without a description. Never overwrites one
        // that's already there.
        let missing: Vec<crate::description_backfill::ToolNeedingDescription> = tools
            .iter()
            .filter(|(_, desc, _)| crate::description_backfill::is_missing(desc))
            .map(
                |(name, _, schema)| crate::description_backfill::ToolNeedingDescription {
                    name: name.clone(),
                    input_schema: schema.clone(),
                },
            )
            .collect();
        if !missing.is_empty() {
            let known_tool_names: Vec<String> = tools
                .iter()
                .filter(|(_, desc, _)| !crate::description_backfill::is_missing(desc))
                .map(|(name, _, _)| name.clone())
                .collect();
            let provider_type = if connector.is_composio() {
                "composio"
            } else {
                "external"
            };
            let result = crate::description_backfill::backfill(
                &state.llm,
                &state.config.description_model,
                &connector.name,
                provider_type,
                &known_tool_names,
                false,
                &missing,
            )
            .await;
            for (name, desc, _) in tools.iter_mut() {
                if crate::description_backfill::is_missing(desc)
                    && let Some(generated) = result.tool_descriptions.get(name)
                {
                    *desc = Some(generated.clone());
                }
            }
        }

        repo::upsert_connector_tools(&state.db, connector_id, &tools).await?;
        // The catalog's cached tool count (catalog::cached_tool_count) would
        // otherwise keep serving whatever it saw before this sync — most
        // commonly a stale `0` — for up to toolcount_ttl_seconds.
        cache::delete(
            &state.redis,
            &crate::catalog::toolcount_cache_key(connector_id),
        )
        .await;
        refresh_search_index(state.search_index.as_ref(), &state.db).await;
    }
    Ok(())
}

/// `GET /agents/{id}/tools` view: the agent's current tool rules across connectors.
/// A restricted caller sees only the rules they may edit (see [`rule_is_editable`]).
pub async fn list_tool_rules_view(
    state: &McpState,
    agent_id: Uuid,
    entitlement: &UserEntitlement,
) -> Result<Value> {
    let rows = repo::get_agent_connector_access(&state.db, agent_id).await?;
    let mut data: Vec<Value> = Vec::new();
    for row in rows {
        let rules = parse_tool_rules(&row.tool_rules);
        let universe = if is_filtered(entitlement, row.connector_id) {
            known_tool_names(state, entitlement, row.connector_id).await?
        } else {
            Vec::new()
        };
        for tr in rules {
            if is_filtered(entitlement, row.connector_id)
                && !rule_is_editable(entitlement, row.connector_id, &universe, &tr.pattern)
            {
                continue;
            }
            data.push(json!({ "connector_id": row.connector_id, "tool_pattern": tr.pattern, "stance": tr.stance }));
        }
    }
    Ok(json!({ "rules": data }))
}

fn connector_not_found(connector_id: Uuid) -> McpError {
    McpError::NotFound(format!("connector '{connector_id}' not found"))
}

/// True when the caller sees only part of this connector's tools: restricted
/// and not granted `All`. Such callers get filtered views and merge-writes.
fn is_filtered(entitlement: &UserEntitlement, connector_id: Uuid) -> bool {
    match entitlement {
        UserEntitlement::Unrestricted => false,
        UserEntitlement::Restricted(map) => !matches!(map.get(&connector_id), Some(ToolSet::All)),
    }
}

/// Tool names a rule pattern could apply to: the synced catalog plus the
/// caller's own entitled names (which may not be synced yet).
async fn known_tool_names(
    state: &McpState,
    entitlement: &UserEntitlement,
    connector_id: Uuid,
) -> Result<Vec<String>> {
    let mut names: Vec<String> = repo::list_connector_tools(&state.db, connector_id)
        .await?
        .into_iter()
        .map(|t| t.tool_name)
        .collect();
    if let UserEntitlement::Restricted(map) = entitlement
        && let Some(ToolSet::Only(only)) = map.get(&connector_id)
    {
        names.extend(only.iter().cloned());
    }
    names.sort();
    names.dedup();
    Ok(names)
}

/// A rule is editable by a filtered caller only if its pattern has no wildcard
/// and names a tool they are entitled to. A wildcard could cover tools the
/// caller cannot see (an unsynced or later-added catalog entry) for every user
/// of the agent, so it belongs to the hidden part: it is never shown, replaced,
/// or accepted from the caller. The name is compared case-insensitively, as
/// `get_stance` does, and every known tool it matches must be entitled.
fn rule_is_editable(
    entitlement: &UserEntitlement,
    connector_id: Uuid,
    universe: &[String],
    pattern: &str,
) -> bool {
    if pattern.contains(['*', '?']) {
        return false;
    }
    let pattern = pattern.to_ascii_lowercase();
    let mut matched = universe
        .iter()
        .filter(|name| name.to_ascii_lowercase() == pattern)
        .peekable();
    matched.peek().is_some() && matched.all(|name| entitlement.allows(connector_id, name))
}

/// One rule input for [`bulk_update_tools`].
pub struct ToolRuleInput {
    pub connector_id: Uuid,
    pub tool_pattern: String,
    pub stance: String,
}

/// `PUT /agents/{id}/tools` — replace tool rules. Groups by connector, validates and dedupes
/// each connector's rules, and upserts them, preserving `enabled`. An agent with no access row
/// yet gets one that is not enabled: setting a rule is not enabling.
///
/// For a caller who sees only part of a connector's tools (see [`is_filtered`])
/// the write is a merge: every incoming rule must be editable by them (else
/// `Forbidden`), the existing rules they cannot see are kept, and only the
/// editable ones are replaced. The caller can thus never change or wipe a rule
/// for a tool they cannot see.
pub async fn bulk_update_tools(
    state: &McpState,
    user_id: Uuid,
    agent_id: Uuid,
    rules: &[ToolRuleInput],
    entitlement: &UserEntitlement,
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

    // Check every connector before writing any, so a refused save changes nothing.
    let mut writes: Vec<(Uuid, bool, Vec<ToolRule>, Vec<ToolRule>)> = Vec::new();
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
            .unwrap_or(false);
        let tool_rules: Vec<ToolRule> = patterns
            .iter()
            .map(|(p, s)| ToolRule {
                pattern: p.clone(),
                stance: s.clone(),
            })
            .collect();
        let stored_rules = if is_filtered(entitlement, connector_id) {
            merge_hidden_rules(state, entitlement, agent_id, connector_id, &tool_rules).await?
        } else {
            tool_rules.clone()
        };
        writes.push((connector_id, enabled, stored_rules, tool_rules));
    }

    let written = write_tool_rules(state, agent_id, writes).await;
    // Also after a failed write: earlier connectors may already have changed.
    invalidate_permission_cache(state, agent_id).await;
    Ok(json!({ "rules": written? }))
}

async fn write_tool_rules(
    state: &McpState,
    agent_id: Uuid,
    writes: Vec<(Uuid, bool, Vec<ToolRule>, Vec<ToolRule>)>,
) -> Result<Vec<Value>> {
    let mut applied: Vec<Value> = Vec::new();
    for (connector_id, enabled, stored_rules, tool_rules) in writes {
        let rules_json = serde_json::to_value(&stored_rules)?;
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
    Ok(applied)
}

/// The rules to store for a filtered caller: their `incoming` rules (each
/// validated as editable) plus the existing rules they cannot see.
async fn merge_hidden_rules(
    state: &McpState,
    entitlement: &UserEntitlement,
    agent_id: Uuid,
    connector_id: Uuid,
    incoming: &[ToolRule],
) -> Result<Vec<ToolRule>> {
    let universe = known_tool_names(state, entitlement, connector_id).await?;
    if let Some(rule) = incoming
        .iter()
        .find(|r| !rule_is_editable(entitlement, connector_id, &universe, &r.pattern))
    {
        return Err(McpError::Forbidden(format!(
            "you are not entitled to every tool matched by '{}'",
            rule.pattern
        )));
    }
    let existing = repo::get_agent_connector_access_row(&state.db, agent_id, connector_id)
        .await?
        .map(|r| parse_tool_rules(&r.tool_rules))
        .unwrap_or_default();
    let mut merged: Vec<ToolRule> = existing
        .into_iter()
        .filter(|r| !rule_is_editable(entitlement, connector_id, &universe, &r.pattern))
        .collect();
    merged.extend(incoming.iter().cloned());
    Ok(merged)
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
    use crate::entitlements::ToolSet;

    #[test]
    fn a_connector_not_shared_with_the_caller_is_not_shared_whatever_the_owner_connected() {
        for (needs, connected) in [(true, false), (true, true), (false, false)] {
            assert_eq!(
                CallerConnection::of(needs, false, connected),
                CallerConnection::NotShared
            );
        }
    }

    #[test]
    fn a_caller_without_their_own_connection_needs_one() {
        assert_eq!(
            CallerConnection::of(true, true, false),
            CallerConnection::NeedsConnection
        );
        assert_eq!(
            CallerConnection::of(true, true, true),
            CallerConnection::Connected
        );
        assert_eq!(
            CallerConnection::of(false, true, false),
            CallerConnection::Ready
        );
    }

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

    #[test]
    fn summarize_description_leaves_short_text_unchanged() {
        let short = "Add two numbers together.";
        assert_eq!(summarize_description(short, 220), short);
    }

    #[test]
    fn summarize_description_prefers_first_paragraph() {
        let desc = "Create an attachment and upload it to Notion.\n\nProvide exactly one source:\n- content for small UTF-8 text artifacts such as HTML, Markdown, plain text, CSV, JSON, XML, CSS, YAML, TSV, calendar, GPX, or SVG files.\n- source_url for a file available at a direct, publicly reachable HTTPS URL.";
        assert_eq!(
            summarize_description(desc, 220),
            "Create an attachment and upload it to Notion."
        );
    }

    #[test]
    fn summarize_description_skips_leading_markdown_heading() {
        // Real shape of Notion's "notion-create-pages" tool description: a
        // bare "## Overview" heading with nothing else on its line, then the
        // actual one-line summary, then far more Markdown sections below.
        let desc = "## Overview\n\nCreates one or more Notion pages, with the specified properties and content.\n\n## Parent\n\nAll pages created with a single call to this tool will have the same parent.";
        assert_eq!(
            summarize_description(desc, 220),
            "Creates one or more Notion pages, with the specified properties and content."
        );
    }

    #[test]
    fn summarize_description_skips_multiple_leading_headings() {
        let desc = "## Overview\n\n### Details\n\nThe real summary sentence finally appears here after two heading lines.\n\n## More\n\nEven more content follows in later sections of this tool description.";
        assert_eq!(
            summarize_description(desc, 220),
            "The real summary sentence finally appears here after two heading lines."
        );
    }

    #[test]
    fn summarize_description_heading_only_with_no_body_returns_empty() {
        assert_eq!(summarize_description("## Overview", 220), "");
    }

    /// Padding long enough to push any two-sentence prefix past the 220-char
    /// cap, so these tests actually exercise sentence-boundary detection
    /// instead of hitting the short-circuit for already-short descriptions.
    const DROP_ME: &str = " This dropped sentence exists purely to push the total description past the summary length cap so the real truncation logic actually runs during this test, instead of returning the whole string unchanged.";

    #[test]
    fn summarize_description_stops_after_two_real_sentences() {
        let desc = format!(
            "Search the workspace and return results. Use it when you need pages to read or cite.{DROP_ME}"
        );
        let out = summarize_description(&desc, 220);
        assert_eq!(
            out,
            "Search the workspace and return results. Use it when you need pages to read or cite."
        );
    }

    #[test]
    fn summarize_description_does_not_split_on_abbreviations() {
        // "e.g." and "etc." sit mid-sentence here (not at a real sentence
        // boundary) — the cut must land on the two real sentence ends, not
        // right after either abbreviation.
        let desc = format!(
            "This tool supports many formats, e.g. HTML and Markdown, and more. It validates the filename before uploading, avoiding mismatches with the content type, etc., along the way.{DROP_ME}"
        );
        let out = summarize_description(&desc, 220);
        assert!(!out.ends_with("e.g."));
        assert!(!out.ends_with("etc."));
        assert_eq!(
            out,
            "This tool supports many formats, e.g. HTML and Markdown, and more. It validates the filename before uploading, avoiding mismatches with the content type, etc., along the way."
        );
    }

    #[test]
    fn summarize_description_does_not_split_on_decimal_numbers() {
        let desc = format!(
            "Downloads are limited to 5.0 MiB for free workspaces. Paid workspaces get a larger 50.5 MiB limit instead, which is plenty for most attachments in practice.{DROP_ME}"
        );
        let out = summarize_description(&desc, 220);
        assert!(out.contains("5.0 MiB"));
        assert_eq!(
            out,
            "Downloads are limited to 5.0 MiB for free workspaces. Paid workspaces get a larger 50.5 MiB limit instead, which is plenty for most attachments in practice."
        );
    }

    #[test]
    fn summarize_description_treats_ellipsis_as_one_boundary() {
        let desc = format!("Wait for the operation to finish... This can take a while.{DROP_ME}");
        let out = summarize_description(&desc, 220);
        assert_eq!(
            out,
            "Wait for the operation to finish... This can take a while."
        );
    }

    #[test]
    fn summarize_description_does_not_split_on_initials() {
        let desc = format!(
            "This integration was designed by J. Smith and the Notion platform team. It has since been extended to support many more file types for uploads.{DROP_ME}"
        );
        let out = summarize_description(&desc, 220);
        assert!(out.contains("J. Smith"));
        assert_eq!(
            out,
            "This integration was designed by J. Smith and the Notion platform team. It has since been extended to support many more file types for uploads."
        );
    }

    #[test]
    fn summarize_description_hard_caps_when_sentences_run_long() {
        // Single sentence far longer than max_chars, no early sentence boundary
        // to stop at — must still cap length and cut on a word boundary.
        let desc = "This is one extremely long single sentence that keeps going and going without any period to stop at until finally it just keeps rambling on well past any reasonable UI display length for a short summary line";
        let out = summarize_description(desc, 100);
        assert!(
            out.chars().count() <= 101,
            "expected <= 101 chars, got {}",
            out.chars().count()
        );
        assert!(out.ends_with('…'));
        assert!(
            !out.ends_with(" …"),
            "must not leave a dangling space before the ellipsis"
        );
    }

    #[test]
    fn summarize_description_caps_even_with_long_first_two_sentences() {
        let long_sentence_a = "A".repeat(150);
        let long_sentence_b = "B".repeat(150);
        let desc = format!("{long_sentence_a}. {long_sentence_b}. Third sentence.");
        let out = summarize_description(&desc, 220);
        assert!(out.chars().count() <= 221);
        assert!(out.ends_with('…'));
    }

    #[test]
    fn summarize_description_handles_empty_and_whitespace() {
        assert_eq!(summarize_description("", 220), "");
        assert_eq!(summarize_description("   ", 220), "");
    }

    #[test]
    fn summarize_description_no_terminal_punctuation_falls_back_to_char_cap() {
        let desc = "a".repeat(300);
        let out = summarize_description(&desc, 100);
        assert!(out.chars().count() <= 101);
        assert!(out.ends_with('…'));
    }
    // ── EffectiveAccess: entitlement narrows, never widens ──────────────────

    fn only_ent(c: Uuid, names: &[&str]) -> UserEntitlement {
        use crate::entitlements::ToolSet;
        UserEntitlement::Restricted(HashMap::from([(
            c,
            ToolSet::Only(names.iter().map(|n| n.to_string()).collect()),
        )]))
    }

    #[test]
    fn rule_is_editable_needs_an_exact_entitled_name() {
        let c = Uuid::new_v4();
        let ent = UserEntitlement::Restricted(HashMap::from([(
            c,
            ToolSet::Only(HashSet::from(["send".to_string()])),
        )]));
        let universe = vec!["send".to_string(), "read".to_string()];
        assert!(rule_is_editable(&ent, c, &universe, "send"));
        assert!(rule_is_editable(&ent, c, &universe, "SEND"));
        assert!(!rule_is_editable(&ent, c, &universe, "read"));
        assert!(!rule_is_editable(&ent, c, &universe, "unknown"));
        assert!(!rule_is_editable(&ent, c, &universe, "sen*"));
        assert!(!rule_is_editable(&ent, c, &universe, "sen?"));
        assert!(!rule_is_editable(&ent, c, &universe, "*"));
    }

    #[test]
    fn effective_entitlement_deny_beats_agent_allow() {
        let a = Uuid::new_v4();
        let perms = ctx(vec![rule(a, "*", Stance::Allow)], &[]);
        let ent = only_ent(a, &["read"]);
        let access = EffectiveAccess::new(&perms, &ent);
        assert_eq!(access.decide(a, "write"), ToolAccess::Denied);
        assert_eq!(access.decide(a, "read"), ToolAccess::Allowed);
    }

    #[test]
    fn effective_entitled_tool_still_obeys_agent_block_and_ask() {
        let a = Uuid::new_v4();
        let perms = ctx(
            vec![
                rule(a, "blocked", Stance::Block),
                rule(a, "asked", Stance::Ask),
            ],
            &[],
        );
        let ent = only_ent(a, &["blocked", "asked", "free"]);
        let access = EffectiveAccess::new(&perms, &ent);
        assert_eq!(access.decide(a, "blocked"), ToolAccess::Denied);
        assert_eq!(access.decide(a, "asked"), ToolAccess::Ask);
        assert_eq!(access.decide(a, "free"), ToolAccess::Allowed);
    }

    #[test]
    fn effective_entitlement_cannot_enable_a_connector_the_agent_disabled() {
        let a = Uuid::new_v4();
        let perms = ctx(vec![rule(a, "*", Stance::Allow)], &[a]);
        let ent = UserEntitlement::Restricted(HashMap::from([(a, ToolSet::All)]));
        let access = EffectiveAccess::new(&perms, &ent);
        assert_eq!(access.decide(a, "x"), ToolAccess::Denied);
        assert!(!access.is_connector_enabled(a));
    }

    #[test]
    fn effective_connector_toggle_requires_entitlement() {
        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        let perms = PermissionContext {
            agent_id: Uuid::nil(),
            enabled_connectors: [a, b].into_iter().collect(),
            rules: vec![],
            hash: "h".into(),
        };
        let ent = only_ent(a, &["read"]);
        let access = EffectiveAccess::new(&perms, &ent);
        assert!(access.is_connector_enabled(a));
        assert!(!access.is_connector_enabled(b));
    }

    #[test]
    fn effective_unrestricted_matches_agent_permissions_exactly() {
        let a = Uuid::new_v4();
        let perms = ctx(vec![rule(a, "x", Stance::Ask)], &[]);
        let access = EffectiveAccess::unrestricted(&perms);
        assert_eq!(access.decide(a, "x"), perms.decide(a, "x"));
        assert_eq!(access.decide(a, "y"), perms.decide(a, "y"));
        assert_eq!(access.hash(), perms.hash, "OSS cache keys are unchanged");
    }

    #[test]
    fn effective_hash_differs_per_entitlement() {
        let a = Uuid::new_v4();
        let perms = ctx(vec![], &[]);
        let (e1, e2) = (only_ent(a, &["x"]), only_ent(a, &["y"]));
        let h1 = EffectiveAccess::new(&perms, &e1).hash();
        let h2 = EffectiveAccess::new(&perms, &e2).hash();
        assert_ne!(h1, h2);
        assert_ne!(h1, EffectiveAccess::unrestricted(&perms).hash());
    }

    // ─── refresh_search_index ─────────────────────────────────────────────────

    /// Counts `rebuild` calls; optionally fails them, like an embedding outage.
    struct CountingIndex {
        rebuilds: std::sync::atomic::AtomicUsize,
        fail: bool,
    }

    #[async_trait::async_trait]
    impl ToolSearchIndex for CountingIndex {
        async fn search_tools(
            &self,
            _query: &str,
            _accessible_connector_ids: &[Uuid],
            _access: &EffectiveAccess<'_>,
            _limit: usize,
        ) -> Vec<crate::search::ToolMatch> {
            Vec::new()
        }

        fn granted_tools(
            &self,
            _connector_ids: &[Uuid],
            _access: &EffectiveAccess<'_>,
        ) -> Vec<crate::search::ToolMatch> {
            Vec::new()
        }

        async fn rebuild(&self, _db: &sqlx::PgPool) -> Result<()> {
            self.rebuilds
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if self.fail {
                return Err(McpError::Internal("embedding API down".into()));
            }
            Ok(())
        }
    }

    fn lazy_pool() -> sqlx::PgPool {
        sqlx::PgPool::connect_lazy("postgres://user:pass@127.0.0.1:1/db")
            .expect("lazy pool construction must not touch the network")
    }

    #[tokio::test]
    async fn refresh_search_index_requests_a_rebuild() {
        let index = CountingIndex {
            rebuilds: Default::default(),
            fail: false,
        };
        refresh_search_index(&index, &lazy_pool()).await;
        assert_eq!(index.rebuilds.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn refresh_search_index_swallows_rebuild_failure() {
        let index = CountingIndex {
            rebuilds: Default::default(),
            fail: true,
        };
        // Returns `()` — a failed rebuild must never fail the tool sync.
        refresh_search_index(&index, &lazy_pool()).await;
        assert_eq!(index.rebuilds.load(std::sync::atomic::Ordering::SeqCst), 1);
    }
}

//! Tool aggregation — fan out `tools/list` to every backend concurrently,
//! namespace generic-server tools, filter by the agent's permissions, merge, and
//! Redis-cache the result.
//!
//! Port of the PoC's `tool_aggregator.aggregate_tools`. Composio meta-tools
//! (`COMPOSIO_SEARCH_TOOLS`, `COMPOSIO_MULTI_EXECUTE_TOOL`, …) keep their names
//! and are **never** filtered here — they span all toolkits, and per-toolkit
//! enforcement happens at `tools/call` (see `protocol.rs`). Generic-server tools
//! are namespaced `{server}__{tool}`; a disabled server is dropped wholesale and
//! individually-blocked tools are removed.
//!
//! A backend that errors or times out is skipped for this cycle (its tools
//! reappear when it recovers) — one slow server never fails the whole list.

use serde_json::{Value, json};
use uuid::Uuid;

use crate::cache;
use crate::error::Result;
use crate::permissions::{PermissionContext, sha256_hex16};
use crate::provider::generic::LIST_TIMEOUT;
use crate::state::McpState;
use crate::types::{MCPServerConfig, Stance};

/// Fan out, namespace, filter, merge, cache. Returns the merged tool list.
pub async fn aggregate_tools(
    state: &McpState,
    user_id: Uuid,
    servers: &[MCPServerConfig],
    connected_toolkits: &[String],
    perms: &PermissionContext,
    traceparent: Option<&str>,
) -> Result<Vec<Value>> {
    let key = manifest_key(user_id, servers, connected_toolkits, &perms.hash);
    if let Some(cached) = cache::get_json::<Vec<Value>>(&state.redis, &key).await {
        tracing::debug!(%user_id, "manifest cache hit");
        return Ok(cached);
    }

    // Fan out to every live backend concurrently.
    let active: Vec<&MCPServerConfig> = servers.iter().filter(|s| !s.url.is_empty()).collect();
    let provider = &state.providers.mcp;
    let results = futures::future::join_all(
        active
            .iter()
            .map(|s| async move { (*s, provider.list_tools(s, LIST_TIMEOUT, traceparent).await) }),
    )
    .await;

    let mut merged: Vec<Value> = Vec::new();
    for (server, result) in results {
        let tools = match result {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!(server = %server.name, error = %e, "tools/list failed — skipping backend this cycle");
                continue;
            }
        };

        if server.name == "composio" {
            // Composio meta-tools pass through unchanged, unfiltered.
            merged.extend(tools);
            continue;
        }

        // Generic server: server-level toggle, then per-tool block filter + namespacing.
        if !perms.is_server_enabled(&server.name) {
            continue;
        }
        for mut tool in tools {
            let Some(obj) = tool.as_object_mut() else { continue };
            let Some(original) = obj.get("name").and_then(|n| n.as_str()).map(str::to_string) else {
                continue;
            };
            if perms.get_stance(&server.name, &original) == Stance::Block {
                continue;
            }
            obj.insert("name".to_string(), json!(format!("{}__{}", server.name, original)));
            merged.push(tool);
        }
    }

    cache::set_json_ex(&state.redis, &key, &merged, state.config.manifest_ttl_seconds).await;
    tracing::info!(%user_id, tool_count = merged.len(), backends = active.len(), perms_hash = %perms.hash, "manifest built");
    Ok(merged)
}

/// `mcp:manifest:{user}:{servers_fp}:{perms_hash}` where `servers_fp` hashes the
/// sorted `(name, url)` backends **and** the sorted connected toolkits — the
/// latter is essential because the Composio backend URL is stable across toolkit
/// changes, so without it a newly-connected toolkit's tools would be masked by a
/// stale manifest.
fn manifest_key(
    user_id: Uuid,
    servers: &[MCPServerConfig],
    connected_toolkits: &[String],
    perms_hash: &str,
) -> String {
    let mut backends: Vec<(&str, &str)> = servers
        .iter()
        .filter(|s| !s.url.is_empty())
        .map(|s| (s.name.as_str(), s.url.as_str()))
        .collect();
    backends.sort();

    let mut toolkits: Vec<&str> = connected_toolkits.iter().map(String::as_str).collect();
    toolkits.sort();

    let raw = serde_json::to_string(&(backends, toolkits)).unwrap_or_default();
    let fp = sha256_hex16(raw.as_bytes());
    format!("mcp:manifest:{user_id}:{fp}:{perms_hash}")
}

//! MCP JSON-RPC handlers for the aggregating gateway.
//!
//! Permission enforcement is two-layer, in order: Layer 1 — can the caller reach
//! this connector at all (owner/grant/composio); Layer 2 — is the tool allowed
//! for this agent. Composio meta-tools are never filtered at list; per-toolkit
//! enforcement happens here at `tools/call` via slug→connector resolution.

use serde_json::{Value, json};
use uuid::Uuid;

use crate::aggregator;
use crate::error::McpError;
use crate::permissions::{self, PermissionContext, ToolAccess, toolkit_from_composio_slug};
use crate::provider::generic::DEFAULT_CALL_TIMEOUT;
use crate::router;
use crate::session::{self, ResolvedSession};
use crate::state::McpState;
use crate::types::{
    ConnectorUnusable, LATEST_PROTOCOL_VERSION, MCPServerConfig, SUPPORTED_PROTOCOL_VERSIONS,
    ServerType, codes,
};

fn ok(req_id: &Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": req_id, "result": result })
}

fn err(req_id: &Value, code: i64, message: impl Into<String>) -> Value {
    json!({ "jsonrpc": "2.0", "id": req_id, "error": { "code": code, "message": message.into() } })
}

/// Like [`err`] but carries a `data` object (e.g. the human-readable connector
/// name, so the route layer's approval flow event stays readable — id-based tool
/// prefixes are otherwise opaque).
fn err_data(req_id: &Value, code: i64, message: impl Into<String>, data: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": req_id, "error": { "code": code, "message": message.into(), "data": data } })
}

/// Build a JSON-RPC error object (for the route layer's identity failures).
pub fn rpc_error(req_id: &Value, code: i64, message: impl Into<String>) -> Value {
    err(req_id, code, message)
}

/// The three tool names the gateway answers itself rather than routing to a
/// backend. `nasiko_call_tool` refuses every one of them as its inner tool.
const SEARCH_TOOLS_META: &str = "nasiko_search_tools";
const CALL_TOOL_META: &str = "nasiko_call_tool";
const RECOVER_COMPRESSED_META: &str = "recover_compressed";

fn is_meta_tool(name: &str) -> bool {
    matches!(
        name,
        SEARCH_TOOLS_META | CALL_TOOL_META | RECOVER_COMPRESSED_META
    )
}

/// Full JSON-RPC dispatch for the agent-facing gateway. Returns `None` for a
/// notification (a request with no `id`).
///
/// `traceparent` and `verified_flow_id` are deliberately two different
/// parameters, not one: `traceparent` is the raw header, propagated to
/// backends and used to resolve unrelated things (a flow's title for tool
/// search, a `context_id` for HITL) that only need a best-effort trace id.
/// `verified_flow_id` is what the route layer (`oss/server/src/mcp/handlers/
/// gateway.rs::flow_user`) actually proved resolves to a live flow this agent
/// participates in — it alone may be signed into the identity header, because
/// only it is trustworthy. A caller on the read-only owner-fallback path
/// (flow lookup failed, so `user_id` is the agent's owner instead) must pass
/// `None` here even when `traceparent` still carries a well-formed but
/// unverified trace id — signing that trace id back out would launder an
/// unverified claim into a header a backend is told to trust unconditionally.
pub async fn handle_request(
    state: &McpState,
    user_id: Uuid,
    agent_id: Uuid,
    body: &Value,
    traceparent: Option<&str>,
    verified_flow_id: Option<&str>,
) -> Option<Value> {
    let method = body.get("method").and_then(|m| m.as_str()).unwrap_or("");

    let Some(req_id) = body.get("id").cloned() else {
        tracing::debug!(method, "mcp notification");
        return None;
    };

    if !implements(method) {
        return Some(err(
            &req_id,
            codes::METHOD_NOT_FOUND,
            format!("Method not found: {method}"),
        ));
    }

    if method == "ping" {
        return Some(json!({ "jsonrpc": "2.0", "id": req_id, "result": {} }));
    }

    let perms = match permissions::load_permission_context(state, agent_id).await {
        Ok(p) => p,
        Err(e) => return Some(err(&req_id, e.json_rpc_code(), e.to_json_rpc().message)),
    };
    let mut resolved = match session::resolve_session(state, user_id).await {
        Ok(r) => r,
        Err(e) => return Some(err(&req_id, e.json_rpc_code(), e.to_json_rpc().message)),
    };

    // `initialize` is the one client-agnostic prompt channel the gateway has —
    // every enabled connector's own harvested `instructions` rides on it,
    // alongside the gateway's own sentence. This does mean `initialize` now
    // performs the permission/session lookups above (it didn't before this
    // was wired up); that's intended, not a leftover of moving the match arm.
    if method == "initialize" {
        let instr: Vec<String> = connector_instructions(&resolved.servers, &perms);
        return Some(handle_initialize(
            &req_id,
            body,
            &state.config.gateway_instructions,
            &instr,
        ));
    }

    // Stamp every system backend's headers with a freshly-signed
    // `(agent_id, user_id, verified_flow_id)` so it can trust the caller's
    // identity without re-deriving it — see `identity.rs`'s module doc. Only
    // `initialize` (handled above) never needs this; both remaining methods
    // route to a real backend. `verified_flow_id` — never a re-parse of the
    // raw `traceparent` — is the only acceptable source for the signed
    // `flow_id`: see this function's own doc comment for why.
    if resolved.servers.iter().any(|s| s.system) {
        let signed = crate::identity::SignedIdentity::new(
            agent_id,
            user_id,
            verified_flow_id.map(str::to_string),
        )
        .sign(&state.config.identity_signing_key);
        inject_identity(&mut resolved.servers, &signed);
    }

    let result = match method {
        "tools/list" => {
            handle_tools_list(
                state,
                user_id,
                &req_id,
                &resolved.servers,
                &resolved.connected_toolkits,
                &perms,
                traceparent,
                verified_flow_id,
            )
            .await
        }
        "tools/call" => {
            let params = body.get("params").cloned().unwrap_or_else(|| json!({}));
            handle_tools_call(
                state,
                user_id,
                &req_id,
                &params,
                &resolved,
                &perms,
                traceparent,
                verified_flow_id,
            )
            .await
        }
        // Unreachable in practice — `implements(method)` already gated every
        // other value above — but a request path must never panic
        // (CLEAN_CODE_GUIDE §6), so this degrades to the same clean
        // `-32601` a real unimplemented method gets, rather than a `panic!`
        // that would 500 the whole request over what `implements()` and this
        // match's arms simply drifted out of sync on.
        other => err(
            &req_id,
            codes::METHOD_NOT_FOUND,
            format!("Method not found: {other}"),
        ),
    };
    Some(result)
}

/// True for exactly the methods [`handle_request`] answers itself
/// (`initialize`, `ping`, `tools/list`, `tools/call`) — the single source of
/// truth for `-32601`. `handle_request` returns `METHOD_NOT_FOUND` for
/// anything this says `false` to, before either match runs, so a method added
/// to either match must be added here too (see the unit tests below, which
/// pin both halves of that list together and prove an unknown method never
/// reaches `load_permission_context`/`resolve_session`).
///
/// Used by the route layer (`oss/server/src/mcp/handlers/gateway.rs`) to gate
/// `MCP-Protocol-Version` header enforcement: streamable-http clients probe
/// with a method this gateway doesn't implement (e.g. `server/discover`)
/// before ever calling `initialize`, relying on our `-32601` to trigger their
/// fallback — that probe must not be rejected with a bare 400 just because it
/// carries a protocol version we haven't negotiated yet.
pub fn implements(method: &str) -> bool {
    matches!(method, "initialize" | "ping" | "tools/list" | "tools/call")
}

/// Version reported in `initialize`'s `serverInfo.version` — the gateway's own
/// release marker, unrelated to the negotiated MCP protocol version computed
/// in `handle_initialize` below.
const GATEWAY_SERVER_VERSION: &str = "1.1.0";

/// `initialize` — negotiate the protocol version (echo a supported client
/// version, else our latest) and advertise instructions: the gateway's own
/// sentence followed by each enabled connector's harvested instructions,
/// joined with a blank line, trimmed, with empty entries dropped. The
/// `instructions` key is omitted entirely when both sources are empty.
pub fn handle_initialize(
    req_id: &Value,
    body: &Value,
    gateway_instructions: &str,
    connector_instructions: &[String],
) -> Value {
    let requested = body
        .get("params")
        .and_then(|p| p.get("protocolVersion"))
        .and_then(Value::as_str);
    let version = match requested {
        Some(v) if SUPPORTED_PROTOCOL_VERSIONS.contains(&v) => v,
        _ => LATEST_PROTOCOL_VERSION,
    };
    let mut instructions: Vec<&str> = Vec::new();
    if !gateway_instructions.trim().is_empty() {
        instructions.push(gateway_instructions.trim());
    }
    instructions.extend(
        connector_instructions
            .iter()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty()),
    );
    let mut result = json!({
        "protocolVersion": version,
        "capabilities": { "tools": {} },
        "serverInfo": { "name": "MCP Gateway", "version": GATEWAY_SERVER_VERSION },
    });
    if !instructions.is_empty() {
        result["instructions"] = json!(instructions.join("\n\n"));
    }
    ok(req_id, result)
}

/// The instruction-collection step `handle_request` feeds into
/// [`handle_initialize`]'s `connector_instructions` parameter: every backend
/// enabled for this agent (Layer 2's connector-level gate — the same one
/// `aggregator`/`tools/call` use) whose harvested `instructions` is present.
/// Kept as its own pure function, separate from `resolve_session`/
/// `load_permission_context` (which need a real DB this crate's hermetic unit
/// tests can't provide), so the composition itself stays unit-testable.
fn connector_instructions(servers: &[MCPServerConfig], perms: &PermissionContext) -> Vec<String> {
    servers
        .iter()
        .filter(|s| perms.is_connector_enabled(s.connector_id))
        .filter_map(|s| s.instructions.clone())
        .collect()
}

/// Stamps every system backend's `headers` with `signed` — an already-built
/// [`crate::identity::SignedIdentity::sign`] header value — so a system
/// backend (the workspace server) can trust who's calling without
/// re-deriving it. Non-system backends are left untouched: the identity
/// header is meaningless (and untrusted) to any backend the platform doesn't
/// itself serve on loopback.
///
/// Deliberately takes the pre-signed string rather than the identity's parts
/// (`agent_id`/`user_id`/`flow_id`) plus a key: the caller (`handle_request`)
/// already gates this whole call on "is there a system backend at all", so
/// signing happens at most once regardless of how many system servers are
/// present, and this function stays a plain, allocation-free stamp.
///
/// Pure and synchronous by design, unlike `handle_request` itself — that
/// function's `resolve_session`/`load_permission_context` calls need a real
/// DB this crate's hermetic unit tests can't provide, so this is the seam the
/// tests below actually exercise.
fn inject_identity(servers: &mut [MCPServerConfig], signed: &str) {
    for s in servers.iter_mut().filter(|s| s.system) {
        s.headers.insert(
            crate::identity::IDENTITY_HEADER.to_string(),
            signed.to_string(),
        );
    }
}

/// `tools/list` — query-aware search (semantic/BM25) or eager fan-out (none mode).
///
/// When search is enabled (`MCP_TOOL_SEARCH_MODE != none`):
/// - With a verified flow: resolves the user's query from `flows.title`, runs
///   flat search, returns top-k matched tools + the three meta-tools
///   (`nasiko_search_tools`, `nasiko_call_tool`, `recover_compressed`).
/// - Without one (agent startup, or a flow-less owner-fallback call): returns
///   only the meta-tools.
///
/// When search is disabled (`none`): delegates to `aggregate_tools` (legacy fan-out).
// `verified_flow_id` pushed this from 7 to 8 (added for the same
// participant-laundering fix `handle_tools_call` below carries) — a params
// struct isn't worth it for one more `Option<&str>` on an internal function
// with a single call site; `ask_with_hitl_request` below already carries the
// same allow for the same reason.
#[allow(clippy::too_many_arguments)]
pub async fn handle_tools_list(
    state: &McpState,
    user_id: Uuid,
    req_id: &Value,
    servers: &[MCPServerConfig],
    connected_toolkits: &[String],
    perms: &PermissionContext,
    traceparent: Option<&str>,
    verified_flow_id: Option<&str>,
) -> Value {
    use crate::config::ToolSearchMode;

    if state.config.tool_search_mode == ToolSearchMode::None {
        // Rollback path: eager fan-out (existing behavior). The forwarded trace context is only
        // ever used as outbound W3C trace context for backend telemetry (`provider.list_tools`),
        // never to resolve identity or read another flow's data — but it still must not be the
        // raw `traceparent` unconditionally: a coding-agent row's owner-fallback call can carry a
        // well-formed traceparent naming a flow it isn't a participant of, and forwarding that
        // verbatim would land this call's backend spans inside someone else's Tempo trace. Same
        // `Option::and` collapse `handle_tools_call`'s `outbound_traceparent` uses — only `Some`
        // on the verified path, where `traceparent` and `verified_flow_id` name the same flow
        // anyway, so a deployed agent's behavior is unchanged.
        return match aggregator::aggregate_tools(
            state,
            user_id,
            servers,
            connected_toolkits,
            perms,
            verified_flow_id.and(traceparent),
        )
        .await
        {
            Ok(tools) => ok(req_id, json!({ "tools": tools })),
            Err(e) => err(req_id, e.json_rpc_code(), e.to_json_rpc().message),
        };
    }

    // ── Search path ────────────────────────────────────────────────────────
    let mut tools: Vec<Value> = Vec::new();

    // Resolve the user's query from the flow record — keyed on
    // `verified_flow_id`, never a re-parse of the raw `traceparent`: a
    // coding-agent row's owner-fallback call carries `verified_flow_id: None`
    // even when `traceparent` still names some OTHER user's live flow (it
    // resolves to the owner precisely because it isn't a participant of
    // that flow), and that other flow's `title` is that user's private query
    // text — nothing this caller was ever proven to have a claim on.
    let user_query = resolve_flow_title(state, verified_flow_id).await;

    if let Some(ref query) = user_query {
        // Get the connector IDs this user can access.
        let accessible_ids: Vec<Uuid> = match state
            .authorizer
            .list_accessible_connectors(&state.db, user_id)
            .await
        {
            Ok(connectors) => connectors.iter().map(|c| c.id).collect(),
            Err(e) => {
                tracing::warn!(error = %e, "failed to list accessible connectors for search");
                Vec::new()
            }
        };

        let matches = state
            .search_index
            .search_tools(
                query,
                &accessible_ids,
                perms,
                state.config.tool_search_tool_limit,
            )
            .await;

        for m in matches {
            tools.push(tool_match_to_json(&m));
        }
    }

    // The meta-tools ride along in every search-mode listing: the search
    // itself, the executor that runs what it finds (the real tools are not in
    // this list, so a client that can only call listed tools has no other way
    // to reach one), and compressed-content recovery.
    tools.push(nasiko_search_tools_definition());
    tools.push(nasiko_call_tool_definition());
    tools.push(recover_compressed_definition());

    ok(req_id, json!({ "tools": tools }))
}

/// Resolve `flows.title` (the user's original query) for a *verified* flow
/// id — never a raw `traceparent` re-parse. Only the route layer
/// (`oss/server/src/mcp/handlers/gateway.rs::flow_user`) proves a trace id
/// names a live flow this agent actually participates in; keying this lookup
/// on anything less would let a caller with no claim on a flow (e.g. a
/// coding-agent row's owner-fallback call, which carries a `None` here
/// exactly because it ISN'T a participant) read another user's private query
/// text out of `flows.title` by naming their trace id.
async fn resolve_flow_title(state: &McpState, verified_flow_id: Option<&str>) -> Option<String> {
    let flow_id = verified_flow_id?;
    sqlx::query_scalar::<_, Option<String>>("SELECT title FROM flows WHERE flow_id = $1")
        .bind(flow_id)
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten()
        .flatten()
}

/// Convert a `ToolMatch` to the JSON format expected by `tools/list`.
fn tool_match_to_json(m: &crate::search::ToolMatch) -> Value {
    let mut obj = json!({
        "name": m.tool_name,
    });
    if let Some(ref desc) = m.description {
        obj["description"] = json!(desc);
    }
    if let Some(ref schema) = m.input_schema {
        obj["inputSchema"] = schema.clone();
    }
    obj
}

/// The `nasiko_search_tools` meta-tool definition — always included in
/// `tools/list` so agents can search for tools not in the initial set.
fn nasiko_search_tools_definition() -> Value {
    json!({
        "name": SEARCH_TOOLS_META,
        "description": "Search for available tools by describing what you need. Use this when you need a capability not already in your tool list.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Describe what you want to do, e.g. 'send an email' or 'create a GitHub issue'"
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of tools to return",
                    "default": 10
                }
            },
            "required": ["query"]
        }
    })
}

/// The `nasiko_call_tool` meta-tool definition — the executor for an MCP
/// client that can only invoke tools present in `tools/list` (Claude Code,
/// Codex, OpenCode). Listed exactly where `nasiko_search_tools` is: in the
/// search modes the real tools are not in the list, so a tool the search
/// found is otherwise unreachable from such a client.
fn nasiko_call_tool_definition() -> Value {
    json!({
        "name": CALL_TOOL_META,
        "description": "Execute a tool found with nasiko_search_tools by name. Use this when you cannot call the found tool directly. `name` is the tool name exactly as returned by the search; `arguments` is the JSON object matching that tool's inputSchema.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "The tool name exactly as returned by nasiko_search_tools"
                },
                "arguments": {
                    "type": "object",
                    "description": "The arguments for that tool, matching its inputSchema",
                    "default": {}
                }
            },
            "required": ["name"]
        }
    })
}

/// What `nasiko_call_tool` was asked to run — the inner tool's name and its
/// own arguments (`{}` when omitted) — or the invalid-params message to
/// answer with. A gateway meta-tool is refused as the inner name: the
/// executor exists to reach tools a client cannot call directly, and the
/// meta-tools are always directly callable, so nesting one could only loop.
fn parse_call_tool_target(arguments: Option<&Value>) -> Result<(&str, Value), &'static str> {
    let name = arguments
        .and_then(|a| a.get("name"))
        .and_then(Value::as_str)
        .ok_or("nasiko_call_tool requires `name`, the tool name exactly as returned by nasiko_search_tools")?;
    if is_meta_tool(name) {
        return Err("nasiko_call_tool cannot call gateway meta-tools; call them directly");
    }
    let inner_arguments = match arguments.and_then(|a| a.get("arguments")) {
        None | Some(Value::Null) => json!({}),
        Some(object @ Value::Object(_)) => object.clone(),
        Some(_) => return Err("nasiko_call_tool `arguments` must be a JSON object"),
    };
    Ok((name, inner_arguments))
}

/// The tool a `tools/call` is really about, for everything outside this
/// module that records or reports a tool name (usage, metrics, the approval
/// flow event): `params.name`, except that `nasiko_call_tool` resolves to the
/// inner tool it dispatches — by the same parse `handle_tools_call` uses, so
/// the recorded name can never differ from the called one. An executor call
/// the parse rejects resolves to `nasiko_call_tool` itself: no inner tool was
/// ever attempted.
pub fn invoked_tool_name(params: &Value) -> &str {
    let outer = params.get("name").and_then(Value::as_str).unwrap_or("");
    if outer != CALL_TOOL_META {
        return outer;
    }
    match parse_call_tool_target(params.get("arguments")) {
        Ok((inner, _)) => inner,
        Err(_) => outer,
    }
}

/// The W3C trace-id out of a `traceparent`, which is what a flow is keyed by.
///
/// Mirrors `nasiko_llm_router::routing::boundary::parse_flow_id`; duplicated rather than shared
/// because this crate does not depend on the router.
pub(crate) fn flow_id_of(traceparent: &str) -> Option<String> {
    let parts: Vec<&str> = traceparent.split('-').collect();
    if parts.len() < 4 {
        return None;
    }
    let trace_id = parts[1];
    let valid = trace_id.len() == 32
        && trace_id.bytes().all(|b| b.is_ascii_hexdigit())
        && trace_id.bytes().any(|b| b != b'0');
    valid.then(|| trace_id.to_ascii_lowercase())
}

/// The `recover_compressed` meta-tool definition (PRD §9 IP-5).
///
/// Listed unconditionally, like the search meta-tool: an agent has to know it exists *before* it
/// meets its first elision marker, because the marker is the only place the handle appears.
fn recover_compressed_definition() -> Value {
    json!({
        "name": RECOVER_COMPRESSED_META,
        "description": "Retrieve the full, uncompressed content that an elision marker stands in for. Call this when a payload you were given contains a marker like `[… 412 lines elided · recover: nasiko://c/9f3a… ]` and the elided part matters for your answer.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "handle": {
                    "type": "string",
                    "description": "The handle from the elision marker, with or without the `nasiko://c/` prefix"
                }
            },
            "required": ["handle"]
        }
    })
}

/// Strip the marker's URI prefix and parse what is left as a handle.
///
/// The model copies the handle out of prose, so it arrives with whatever punctuation surrounded
/// it. Accepting both forms is cheaper than teaching it one.
fn parse_recovery_handle(raw: &str) -> Option<Uuid> {
    raw.trim()
        .trim_start_matches("nasiko://c/")
        .trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '-')
        .parse()
        .ok()
}

/// Whether a connector-connection lookup means an `auth_required` pause should fire. `Err` (a
/// transient DB blip) must NOT be treated the same as `Ok(None)`/a non-`ACTIVE` row — a DB error
/// says nothing about whether the user is actually connected, so it must not, by itself, tell the
/// user to re-authenticate a connector that could be perfectly fine.
fn needs_auth_required(
    result: &crate::error::Result<Option<crate::repo::McpUserConnection>>,
) -> bool {
    match result {
        Ok(Some(c)) => !c.status.eq_ignore_ascii_case("ACTIVE"),
        Ok(None) => true,
        Err(_) => false,
    }
}

/// `tools/call` — answer the gateway's own meta-tools (`nasiko_search_tools`,
/// `recover_compressed`, and the `nasiko_call_tool` executor), and hand every
/// real tool — named directly, or as the executor's inner tool — to
/// `call_routed_tool`, the one path that routes, enforces both permission
/// layers and forwards to the backend.
///
/// `traceparent` and `verified_flow_id` carry the same two-different-things
/// split `handle_request`'s own doc comment explains: `traceparent` is the
/// raw header (only ever used below as *outbound* trace context, gated to
/// the verified case — see `outbound_traceparent`), `verified_flow_id` is
/// what the route layer actually proved resolves to a live flow this agent
/// participates in. Every HITL `context_id` resolution in this call tree
/// (`handle_auth_required`, `create_tool_approval_id`,
/// `resolve_tool_approval_retry`, and their `detect_composio_auth_required`/
/// `ask_with_hitl_request` wrappers) takes `verified_flow_id`, never
/// `traceparent` — see those functions' own docs for why: a raw traceparent
/// naming a flow this agent isn't a participant of (the coding-agent
/// owner-fallback path's whole reason for existing) must never seed a HITL
/// row's `context_id`, or resolving that row would re-open and join the
/// NAMED flow, not this call's actual one.
// `verified_flow_id` pushed this from 7 to 8 — same call as `handle_tools_list`'s
// own identical allow just above: not worth a params struct for one more
// `Option<&str>` this fix required, on a function with exactly one call site
// (`handle_request`).
#[allow(clippy::too_many_arguments)]
pub async fn handle_tools_call(
    state: &McpState,
    user_id: Uuid,
    req_id: &Value,
    params: &Value,
    resolved: &ResolvedSession,
    perms: &PermissionContext,
    traceparent: Option<&str>,
    verified_flow_id: Option<&str>,
) -> Value {
    let tool_name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");

    // ── nasiko_call_tool meta-tool: the fixed-menu executor ──────────────
    // Unwrap and re-enter the routed path with the inner tool, so nothing
    // downstream can tell the two entry points apart. Handled before the
    // outer `arguments` are cloned: only the inner object is needed here.
    if tool_name == CALL_TOOL_META {
        return match parse_call_tool_target(params.get("arguments")) {
            Ok((inner_name, inner_arguments)) => {
                call_routed_tool(
                    state,
                    user_id,
                    req_id,
                    inner_name,
                    inner_arguments,
                    resolved,
                    perms,
                    traceparent,
                    verified_flow_id,
                )
                .await
            }
            Err(message) => err(req_id, codes::INVALID_PARAMS, message),
        };
    }

    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));

    // ── nasiko_search_tools meta-tool ────────────────────────────────────
    if tool_name == SEARCH_TOOLS_META {
        let query = arguments
            .get("query")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let limit = arguments
            .get("limit")
            .and_then(|v| v.as_u64())
            .unwrap_or(state.config.tool_search_meta_limit as u64) as usize;

        let accessible_ids: Vec<Uuid> = match state
            .authorizer
            .list_accessible_connectors(&state.db, user_id)
            .await
        {
            Ok(connectors) => connectors.iter().map(|c| c.id).collect(),
            Err(e) => {
                return err(
                    req_id,
                    codes::INTERNAL_ERROR,
                    format!("failed to resolve accessible connectors: {e}"),
                );
            }
        };

        let matches = state
            .search_index
            .search_tools(query, &accessible_ids, perms, limit)
            .await;

        let tools: Vec<Value> = matches.iter().map(tool_match_to_json).collect();
        let payload = json!({
            "tools": tools,
            "search_mode": format!("{:?}", state.config.tool_search_mode),
        });
        // A `tools/call` result is read through `content` by MCP clients (a
        // fixed-menu client such as Claude Code shows the model nothing else),
        // and through `structuredContent` by schema-aware ones; the bare
        // `tools`/`search_mode` fields stay at the top level for programmatic
        // callers that already read them directly.
        let text = payload.to_string();
        let mut result = payload.clone();
        result["content"] = json!([{ "type": "text", "text": text }]);
        result["structuredContent"] = payload;
        return ok(req_id, result);
    }

    // ── recover_compressed meta-tool (IP-5) ──────────────────────────────
    if tool_name == RECOVER_COMPRESSED_META {
        let Some(handle) = arguments
            .get("handle")
            .and_then(|v| v.as_str())
            .and_then(parse_recovery_handle)
        else {
            return err(
                req_id,
                codes::INVALID_PARAMS,
                "recover_compressed requires `handle`, the identifier from an elision marker",
            );
        };

        // Scope, not just lookup: the handle alone is a bearer token, so the row must also
        // belong to this user and to the flow this call is being made inside. `flow_id` is the
        // traceparent trace-id, which `tools/call` has already proven this agent participates in.
        let Some(flow_id) = traceparent.and_then(crate::protocol::flow_id_of) else {
            return err(
                req_id,
                codes::INVALID_PARAMS,
                "recover_compressed is only available inside a flow",
            );
        };

        let row: Result<Option<(String, String)>, _> = sqlx::query_as(
            "SELECT content, content_type FROM compression_originals \
             WHERE handle = $1 AND owner_id = $2 AND flow_id = $3",
        )
        .bind(handle)
        .bind(user_id)
        .bind(&flow_id)
        .fetch_optional(&state.db)
        .await;

        return match row {
            // Deliberately one message for "no such handle" and "not yours": distinguishing them
            // would turn the handle into an existence oracle across flows.
            Ok(None) => ok(
                req_id,
                json!({
                    "content": [{
                        "type": "text",
                        "text": "No recoverable content for that handle. It may have expired, or belong to a different conversation."
                    }],
                    "isError": true
                }),
            ),
            Ok(Some((content, content_type))) => ok(
                req_id,
                json!({
                    "content": [{ "type": "text", "text": content }],
                    "_meta": { "content_type": content_type }
                }),
            ),
            Err(e) => err(
                req_id,
                codes::INTERNAL_ERROR,
                format!("failed to read the recovery store: {e}"),
            ),
        };
    }

    call_routed_tool(
        state,
        user_id,
        req_id,
        tool_name,
        arguments,
        resolved,
        perms,
        traceparent,
        verified_flow_id,
    )
    .await
}

/// The one path every real tool call takes — routing, both permission
/// layers, the backend call — whether the client named `tool_name` directly
/// or asked `nasiko_call_tool` to run it. Taking the name and arguments
/// rather than the request's `params` is what makes that so: the executor
/// unwraps its inner call and re-enters here, and from this point on
/// routing, `perms.decide`, the `Ask` approval record and its retry match,
/// the backend request and every logged tool name see the inner tool — the
/// wrapper no longer exists.
///
/// `outbound_traceparent` is the only trace context a backend ever receives
/// — never the fallback path's raw `traceparent`. A coding-agent row's
/// owner-fallback call may carry a well-formed (self-generated, or another
/// user's) traceparent even though `verified_flow_id` is `None`; forwarding
/// it as-is would let that row's spans land inside someone else's Tempo
/// trace. `Option::and` collapses to `None` unless BOTH sides are `Some` —
/// i.e. only on the verified path, where `traceparent` and
/// `verified_flow_id` name the same flow anyway.
// Nine parameters: `handle_tools_call`'s eight plus the arguments, which the
// executor supplies from its own `arguments` rather than from `params`.
#[allow(clippy::too_many_arguments)]
async fn call_routed_tool(
    state: &McpState,
    user_id: Uuid,
    req_id: &Value,
    tool_name: &str,
    mut arguments: Value,
    resolved: &ResolvedSession,
    perms: &PermissionContext,
    traceparent: Option<&str>,
    verified_flow_id: Option<&str>,
) -> Value {
    let outbound_traceparent = verified_flow_id.and(traceparent);

    let (server, original) = match router::route_tool(tool_name, &resolved.servers) {
        Ok(pair) => pair,
        Err(e) => {
            if let Some((connector_id, info)) =
                router::unusable_reason_for_prefix(tool_name, &resolved.unusable_connectors)
                && info.reason == ConnectorUnusable::AuthRequired
            {
                // `unusable_connectors` is built from user-level (Layer 1) access only, so this
                // branch used to fire before the Layer-2 per-agent gate below ever ran. A
                // connector the admin disabled FOR THIS AGENT (`perms.is_connector_enabled`
                // false) still reached `handle_auth_required`, which discloses the connector's
                // name/UUID to the agent and files a pending `auth_required` row asking the
                // human to re-authenticate a connector this agent isn't even permitted to use —
                // disclosure plus a spurious notification, even though `perms.decide` below
                // would still correctly block the actual call.
                if !perms.is_connector_enabled(connector_id) {
                    return err(
                        req_id,
                        codes::TOOL_BLOCKED,
                        format!("Tool '{tool_name}' is blocked or disabled for this agent."),
                    );
                }
                return handle_auth_required(
                    state,
                    user_id,
                    req_id,
                    perms.agent_id,
                    connector_id,
                    &info.name,
                    verified_flow_id,
                )
                .await;
            }
            // A bare Composio-slug tool `route_tool` couldn't place (no `{prefix}__`
            // to match `unusable_reason_for_prefix` above, which is generic-connector
            // only anyway) whose toolkit corresponds to a real connector that just
            // isn't an ACTIVE connection for this user yet. `toolkit_to_connector`
            // (what `route_tool` actually searched) is ACTIVE-only by construction —
            // see `session.rs`'s `current_connected_accounts` — so a connector stuck
            // at e.g. `INITIATED` (registered, never finished OAuth) is invisible to
            // it and always falls through to here. Never having connected is
            // functionally the same "a human is needed" signal as a credential that
            // broke after working (`detect_composio_auth_required` below), so it gets
            // the identical AUTH_REQUIRED pause instead of a bare routing error with
            // nothing a human can act on.
            let toolkit = toolkit_from_composio_slug(tool_name);
            if let Ok(Some(connector)) =
                crate::repo::get_composio_connector_by_name(&state.db, &toolkit).await
            {
                // `Err` (a transient DB blip) and `Ok(None)` (genuinely no connection row) are
                // distinct outcomes — collapsing them via `.ok().flatten()` used to treat a
                // momentary DB error as "not connected," filing a spurious `auth_required` pause
                // and telling the user to re-authenticate a connector that's actually fine.
                let connection_result =
                    crate::repo::get_user_connection(&state.db, user_id, connector.id).await;
                if let Err(e) = &connection_result {
                    tracing::warn!(
                        error = %e, connector_id = %connector.id,
                        "handle_tools_call: db error checking connector connection status; \
                         skipping the auth_required check rather than falsely reporting not connected"
                    );
                }
                if needs_auth_required(&connection_result) {
                    return handle_auth_required(
                        state,
                        user_id,
                        req_id,
                        perms.agent_id,
                        connector.id,
                        &connector.name,
                        verified_flow_id,
                    )
                    .await;
                }
            }
            // A bare Composio META-tool (`COMPOSIO_SEARCH_TOOLS`, `COMPOSIO_MULTI_EXECUTE_TOOL`,
            // ...) is unroutable when the user has *zero* active Composio connections at all —
            // Composio's whole backend isn't wired into `resolved.servers` in that state, so even
            // discovery itself fails (verified live: "Unknown tool 'COMPOSIO_SEARCH_TOOLS'").
            // `toolkit_from_composio_slug` extracts `"composio"` from these names (not a real
            // per-integration connector), so the check just above can never catch this — there's
            // no specific toolkit to look up. Fall back to whichever Composio connector this
            // AGENT has actually been granted (`perms.enabled_connectors`, exactly the set
            // `nasiko mcp agent-tools enable` writes to `mcp_agent_connector_access`) but that
            // has no active user connection (cross-checked against `resolved.toolkit_to_connector`,
            // which is active-connections-only by construction — see the check above's own
            // comment). Exactly one such candidate is unambiguously the one needing auth; more
            // than one is a genuine ambiguity this can't guess through, so it falls through to
            // the generic error below, same as today.
            if tool_name.starts_with("COMPOSIO_") {
                let mut candidates = Vec::new();
                for &connector_id in &perms.enabled_connectors {
                    if resolved
                        .toolkit_to_connector
                        .values()
                        .any(|&id| id == connector_id)
                    {
                        continue; // already an active connection — not the gap being diagnosed
                    }
                    if let Ok(Some(connector)) =
                        crate::repo::get_connector_by_id(&state.db, connector_id).await
                        && connector.is_composio()
                    {
                        candidates.push(connector);
                    }
                }
                if let [connector] = candidates.as_slice() {
                    return handle_auth_required(
                        state,
                        user_id,
                        req_id,
                        perms.agent_id,
                        connector.id,
                        &connector.name,
                        verified_flow_id,
                    )
                    .await;
                }
            }
            return err(req_id, codes::INVALID_PARAMS, e.to_string());
        }
    };

    // ── Generic MCP tool: Layer 1 (reachability) then Layer 2 (decide) ─────
    if server.kind == ServerType::Mcp {
        match state
            .authorizer
            .can_access_connector(&state.db, user_id, server.connector_id)
            .await
        {
            Ok(true) => {}
            Ok(false) => {
                return err(
                    req_id,
                    codes::TOOL_BLOCKED,
                    format!("Connector for '{tool_name}' is not available."),
                );
            }
            Err(e) => return err(req_id, e.json_rpc_code(), e.to_json_rpc().message),
        }
        // Same decision `tools/list` filters on — a connector disabled for this
        // agent denies the call even though Layer 1 (owner/grant) still passes.
        match perms.decide(server.connector_id, &original) {
            ToolAccess::Denied => {
                return err(
                    req_id,
                    codes::TOOL_BLOCKED,
                    format!("Tool '{tool_name}' is blocked or disabled for this agent."),
                );
            }
            ToolAccess::Ask => {
                match resolve_tool_approval_retry(
                    state,
                    user_id,
                    perms.agent_id,
                    server.connector_id,
                    &original,
                    verified_flow_id,
                )
                .await
                {
                    RetryOutcome::Proceed => {}
                    RetryOutcome::Denied => {
                        return err(
                            req_id,
                            codes::TOOL_BLOCKED,
                            format!(
                                "Tool '{tool_name}' was denied by the user. Do not retry this call."
                            ),
                        );
                    }
                    RetryOutcome::AskAgain => {
                        return ask_with_hitl_request(
                            state,
                            user_id,
                            perms.agent_id,
                            server.connector_id,
                            &original,
                            &server.name,
                            req_id,
                            verified_flow_id,
                        )
                        .await;
                    }
                }
            }
            ToolAccess::Allowed => {}
        }
    }

    // ── Composio DIRECT tool call: enforce per-toolkit permission ───────────
    // A direct toolkit tool (e.g. GMAIL_SEND_EMAIL) resolves to its connector and
    // is subject to the same decide() as any other tool. Previously only the two
    // batch meta-tools below were checked, so a direct call bypassed enforcement
    // entirely (Round 3). Cross-toolkit meta-tools (COMPOSIO_SEARCH_TOOLS,
    // MANAGE_CONNECTIONS, MULTI_EXECUTE_TOOL) resolve to toolkit "composio", which
    // maps to no connector — they skip this block and are handled below / passed
    // through, exactly as before.
    if server.kind == ServerType::Composio
        && let Some(&cid) = resolved
            .toolkit_to_connector
            .get(&toolkit_from_composio_slug(tool_name))
    {
        match perms.decide(cid, tool_name) {
            ToolAccess::Denied => {
                return err(
                    req_id,
                    codes::TOOL_BLOCKED,
                    format!("Tool '{tool_name}' is blocked or disabled for this agent."),
                );
            }
            ToolAccess::Ask => {
                match resolve_tool_approval_retry(
                    state,
                    user_id,
                    perms.agent_id,
                    cid,
                    tool_name,
                    verified_flow_id,
                )
                .await
                {
                    RetryOutcome::Proceed => {}
                    RetryOutcome::Denied => {
                        return err(
                            req_id,
                            codes::TOOL_BLOCKED,
                            format!(
                                "Tool '{tool_name}' was denied by the user. Do not retry this call."
                            ),
                        );
                    }
                    RetryOutcome::AskAgain => {
                        return ask_with_hitl_request(
                            state,
                            user_id,
                            perms.agent_id,
                            cid,
                            tool_name,
                            "composio",
                            req_id,
                            verified_flow_id,
                        )
                        .await;
                    }
                }
            }
            ToolAccess::Allowed => {}
        }
    }

    // ── Composio meta-tool interception (per-toolkit → connector) ───────────
    if tool_name == "COMPOSIO_MANAGE_CONNECTIONS"
        && let Some(requested) = arguments.get("toolkits").and_then(|v| v.as_array())
    {
        let requested: Vec<String> = requested
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect();
        let blocked: Vec<String> = requested
            .iter()
            .filter(|tk| connector_disabled(resolved, perms, tk))
            .cloned()
            .collect();
        if !blocked.is_empty() {
            let allowed: Vec<String> = requested
                .iter()
                .filter(|tk| !blocked.contains(tk))
                .cloned()
                .collect();
            if allowed.is_empty() {
                return err(
                    req_id,
                    codes::TOOL_BLOCKED,
                    format!("Toolkit(s) are disabled for this agent: {blocked:?}."),
                );
            }
            arguments["toolkits"] = json!(allowed);
        }
    }

    if tool_name == "COMPOSIO_MULTI_EXECUTE_TOOL"
        && let Some(tools_arg) = arguments.get("tools").and_then(|v| v.as_array()).cloned()
        && !tools_arg.is_empty()
    {
        let mut allowed: Vec<Value> = Vec::new();
        let mut blocked_slugs: Vec<String> = Vec::new();
        // A previously resolved `tool_approval` decision found on retry
        // (M8) — rejected, so this slug fails outright rather than being
        // re-asked; tracked separately from `blocked_slugs` since the
        // message ("denied by the user") is distinguishable from a
        // `Stance::Block`/disabled-connector denial.
        let mut denied_slugs: Vec<String> = Vec::new();
        // (connector_id, slug) — the connector id is kept alongside each
        // asked slug so a pending `tool_approval` row can be persisted per
        // tool below; the message text still renders as a bare slug list.
        let mut ask_tools: Vec<(Uuid, String)> = Vec::new();

        for t in &tools_arg {
            let slug = t.get("tool_slug").and_then(|v| v.as_str()).unwrap_or("");
            if slug.is_empty() {
                allowed.push(t.clone());
                continue;
            }
            let toolkit = toolkit_from_composio_slug(slug);
            match resolved.toolkit_to_connector.get(&toolkit) {
                Some(&cid) => match perms.decide(cid, slug) {
                    ToolAccess::Denied => blocked_slugs.push(slug.to_string()),
                    ToolAccess::Ask => {
                        match resolve_tool_approval_retry(
                            state,
                            user_id,
                            perms.agent_id,
                            cid,
                            slug,
                            verified_flow_id,
                        )
                        .await
                        {
                            RetryOutcome::Proceed => allowed.push(t.clone()),
                            RetryOutcome::Denied => denied_slugs.push(slug.to_string()),
                            RetryOutcome::AskAgain => ask_tools.push((cid, slug.to_string())),
                        }
                    }
                    ToolAccess::Allowed => allowed.push(t.clone()),
                },
                None => allowed.push(t.clone()),
            }
        }

        if allowed.is_empty()
            && (!blocked_slugs.is_empty() || !denied_slugs.is_empty() || !ask_tools.is_empty())
        {
            if !ask_tools.is_empty() {
                let mut hitl_request_ids = Vec::with_capacity(ask_tools.len());
                for (cid, slug) in &ask_tools {
                    if let Some(id) = create_tool_approval_id(
                        state,
                        user_id,
                        perms.agent_id,
                        *cid,
                        slug,
                        verified_flow_id,
                    )
                    .await
                    {
                        hitl_request_ids.push(id);
                    }
                }
                let ask_slugs: Vec<&String> = ask_tools.iter().map(|(_, s)| s).collect();
                return err_data(
                    req_id,
                    codes::TOOL_ASK,
                    format!("Tool(s) require user approval for this agent: {ask_slugs:?}."),
                    json!({ "server": "composio", "hitl_request_ids": hitl_request_ids }),
                );
            }
            if blocked_slugs.is_empty() {
                return err(
                    req_id,
                    codes::TOOL_BLOCKED,
                    format!("Tool(s) were denied by the user. Do not retry: {denied_slugs:?}."),
                );
            }
            if denied_slugs.is_empty() {
                return err(
                    req_id,
                    codes::TOOL_BLOCKED,
                    format!("All requested tools are blocked for this agent: {blocked_slugs:?}."),
                );
            }
            return err(
                req_id,
                codes::TOOL_BLOCKED,
                format!(
                    "All requested tools are blocked or were denied for this agent: \
                     blocked={blocked_slugs:?}, denied={denied_slugs:?}."
                ),
            );
        }
        if !blocked_slugs.is_empty() || !denied_slugs.is_empty() || !ask_tools.is_empty() {
            let ask_slugs: Vec<&String> = ask_tools.iter().map(|(_, s)| s).collect();
            tracing::info!(user = %user_id, agent = %perms.agent_id, ?blocked_slugs, ?denied_slugs, ?ask_slugs, forwarding = allowed.len(), "partial composio multi-execute filter");
            arguments["tools"] = json!(allowed);
        }
    }

    // ── Composio MULTI_EXECUTE wrapping ─────────────────────────────────────
    // Direct Composio toolkit tools (GMAIL_SEND_EMAIL etc.) must be wrapped in
    // COMPOSIO_MULTI_EXECUTE_TOOL — the Composio MCP endpoint does not
    // recognize individual tool slugs (POC finding, §4.1 of design doc).
    // Meta-tools (COMPOSIO_SEARCH_TOOLS, COMPOSIO_MANAGE_CONNECTIONS,
    // COMPOSIO_MULTI_EXECUTE_TOOL itself) are NOT wrapped.
    let is_composio_toolkit_tool = server.kind == ServerType::Composio
        && resolved
            .toolkit_to_connector
            .contains_key(&toolkit_from_composio_slug(tool_name));

    let (forward_name, forward_args) = if is_composio_toolkit_tool {
        let wrapped = json!({
            "tools": [{
                "tool_slug": original,
                "arguments": arguments
            }]
        });
        ("COMPOSIO_MULTI_EXECUTE_TOOL".to_string(), wrapped)
    } else {
        (original.clone(), arguments.clone())
    };

    tracing::info!(tool = %tool_name, forwarded_as = %forward_name, kind = ?server.kind, "routing tool call");

    match state
        .providers
        .mcp
        .call_tool(
            server,
            req_id,
            &forward_name,
            &forward_args,
            DEFAULT_CALL_TIMEOUT,
            outbound_traceparent,
        )
        .await
    {
        Ok(response) if server.kind == ServerType::Composio && response.get("error").is_some() => {
            match resolved
                .toolkit_to_connector
                .get(&toolkit_from_composio_slug(tool_name))
            {
                // Meta-tool (COMPOSIO_SEARCH_TOOLS/MANAGE_CONNECTIONS/...) or
                // an unmapped toolkit — nothing to check against.
                None => response,
                Some(&connector_id) => {
                    detect_composio_auth_required(
                        state,
                        user_id,
                        req_id,
                        perms.agent_id,
                        connector_id,
                        verified_flow_id,
                        response,
                    )
                    .await
                }
            }
        }
        Ok(response) if is_composio_toolkit_tool => unwrap_multi_execute_response(response),
        Ok(response) => response,
        Err(e) => {
            // Self-heal: an uploaded_build connector's container can move
            // (restart/redeploy/reboot) between build time and this call. On
            // a connection-level failure (not an application-level MCP
            // error), ask the refresher for the container's current live
            // address and retry exactly once before giving up — mirrors this
            // gateway's own existing precedent for the structurally
            // identical Composio-connection staleness problem (refresh only
            // on-demand, never on every request). `!server.system` excludes
            // the OTHER kind of trusted backend: a system connector has no
            // container at all, so the refresher's
            // `runtime.endpoint(ContainerId::from_uuid(connector_id))` is
            // meaningless for it — and, under `SimulatedRuntime` with
            // `SIM_RESOLVE_ALL` (a real test/dev configuration), it would
            // "succeed" and overwrite the system row's real loopback URL with
            // a bogus one.
            if server.trusted
                && !server.system
                && is_connection_level_failure(&e)
                && let Some(new_url) = state.endpoint_refresher.refresh(server.connector_id).await
            {
                tracing::info!(server = %server.name, connector_id = %server.connector_id, "endpoint stale — retrying tool call against refreshed address");
                let mut refreshed = server.clone();
                refreshed.url = new_url;
                match state
                    .providers
                    .mcp
                    .call_tool(
                        &refreshed,
                        req_id,
                        &original,
                        &arguments,
                        DEFAULT_CALL_TIMEOUT,
                        outbound_traceparent,
                    )
                    .await
                {
                    Ok(response) => return response,
                    Err(e2) => {
                        tracing::warn!(server = %server.name, tool = %original, error = %e2, "backend tool call failed again after endpoint refresh");
                        return err(
                            req_id,
                            codes::INTERNAL_ERROR,
                            format!("Backend '{}' failed to execute '{}'", server.name, original),
                        );
                    }
                }
            }
            tracing::warn!(server = %server.name, tool = %original, error = %e, "backend tool call failed");
            err(
                req_id,
                codes::INTERNAL_ERROR,
                format!("Backend '{}' failed to execute '{}'", server.name, original),
            )
        }
    }
}

/// A composio-routed call's response carried a JSON-RPC `error` — before
/// passing it straight through unchanged, check whether the specific
/// toolkit's own connection is why.
///
/// Composio aggregates every one of a user's connected toolkits into a
/// single shared Tool Router session (`composio_config`'s
/// `connector_id: Uuid::nil()` — see `session.rs`), so unlike a generic
/// connector, whose broken credential is caught before the call ever goes
/// out (`build_generic_servers`'s pre-check populates `unusable_connectors`
/// at `tools/list` time), a broken Composio toolkit can only ever be
/// detected once a call to it actually fails — there is no per-toolkit
/// session to pre-check. This re-verifies the specific toolkit's live
/// status via the same `check_connection_status` call
/// `connect.rs::handle_composio_callback` already uses, and — only if
/// Composio itself confirms the connection isn't ACTIVE, never by guessing
/// from the error's message text (undocumented and unverified against the
/// live Tool Router) — routes through the exact same `handle_auth_required`
/// (M3) a generic connector uses, so persistence, the resolve API, the
/// dispatcher, and auto-resolve-on-reconnect (`connect.rs`'s own ACTIVE
/// branch) are all unconditionally shared, never reimplemented for
/// Composio. Any other tool-level failure (bad arguments, a real backend
/// error, ...) or a connection this check can't resolve falls through to
/// the original, unmodified response — this only ever narrows what counts
/// as `AuthRequired`, never widens it.
async fn detect_composio_auth_required(
    state: &McpState,
    user_id: Uuid,
    req_id: &Value,
    agent_id: Uuid,
    connector_id: Uuid,
    verified_flow_id: Option<&str>,
    original_response: Value,
) -> Value {
    let Some(provider) = &state.providers.composio else {
        return original_response;
    };
    let Ok(Some(connector)) = crate::repo::get_connector_by_id(&state.db, connector_id).await
    else {
        return original_response;
    };
    let Some(auth_config_id) = connector.auth_config_id.as_deref() else {
        return original_response;
    };
    let Ok(check) = provider
        .check_connection_status(&user_id.to_string(), auth_config_id)
        .await
    else {
        return original_response;
    };
    if check.status.eq_ignore_ascii_case("ACTIVE") {
        return original_response;
    }

    tracing::info!(
        connector = %connector.name, %connector_id, status = %check.status,
        "composio tool call failed and the connection is no longer active — treating as auth_required"
    );
    handle_auth_required(
        state,
        user_id,
        req_id,
        agent_id,
        connector_id,
        &connector.name,
        verified_flow_id,
    )
    .await
}

/// A tool call's connector needs the user to (re-)authenticate
/// (`ConnectorUnusable::AuthRequired`, from M1's credential-failure
/// plumbing, or a Composio toolkit that was never connected in the first
/// place — `handle_tools_call`'s routing-failure branch) — persist a pending
/// `hitl_requests` row (M2's store) and return `codes::AUTH_REQUIRED` instead
/// of the generic "connector not available" error, so the agent (and,
/// through it, the human) gets a distinguishable, actionable signal instead
/// of an indistinguishable dead end.
///
/// For a Composio connector, also mints (or reuses) a real, clickable OAuth
/// link via `connect::composio_connect` — the same call `POST /api/mcp/connect`
/// makes — so an inline pause is actually self-service instead of pointing the
/// human at a separate command. Best-effort: a generic (non-Composio) connector,
/// or a failed mint call, still gets the pause, just without a link in `question`.
/// Does not push or auto-retry anything itself — that's the resume dispatcher's
/// job once the human resolves this row.
async fn handle_auth_required(
    state: &McpState,
    user_id: Uuid,
    req_id: &Value,
    agent_id: Uuid,
    connector_id: Uuid,
    connector_name: &str,
    verified_flow_id: Option<&str>,
) -> Value {
    let generic_error = || {
        err(
            req_id,
            codes::INVALID_PARAMS,
            format!(
                "Connector '{connector_name}' is not available for this agent. \
                 It may be disabled in the agent's permission settings."
            ),
        )
    };

    // No verified flow at all means nothing to correlate a resumable
    // conversation against — fall back to today's generic error rather than
    // persist a HITL row no future dispatcher could ever address. Keyed on
    // `verified_flow_id`, never the raw `traceparent`: a coding-agent row's
    // owner-fallback call can carry a well-formed traceparent naming a flow
    // it isn't a participant of, and seeding a HITL row's `context_id` from
    // that would let approving it re-open and join that OTHER flow (see
    // `oss/hitl/src/notifier.rs`'s resume path, and the guard added there
    // against exactly this).
    let Some(context_id) = session::resolve_context_id(state, verified_flow_id).await else {
        tracing::warn!(
            connector = %connector_name, %connector_id,
            "auth_required detected but no verified flow to resolve a context_id from — falling back to generic error"
        );
        return generic_error();
    };

    let mut question = json!({
        "connector_id": connector_id,
        "connector": connector_name,
        "message": format!(
            "Authentication for connector '{connector_name}' is missing or no longer works. \
             A human must re-authenticate before this tool can be used again."
        ),
    });

    // Best-effort: a Composio connector gets a real, clickable re-auth link inline —
    // same call `POST /api/mcp/connect` makes, safe to call again on an already
    // `INITIATED` row (reuses the cached link if still fresh, mints a new one
    // otherwise; never duplicates or errors on retry). A generic (non-Composio)
    // connector, or any failure minting the link, leaves `question` exactly as
    // built above — the pause itself must never be lost over this enrichment.
    if let Ok(Some(connector)) = crate::repo::get_connector_by_id(&state.db, connector_id).await
        && connector.is_composio()
    {
        match crate::connect::composio_connect(state, user_id, &connector, None).await {
            Ok(crate::connect::ConnectOutcome::Initiated {
                oauth_url: Some(url),
                ..
            }) => {
                if let Some(obj) = question.as_object_mut() {
                    obj.insert("auth_url".to_string(), json!(url));
                }
            }
            Ok(_) => {}
            Err(e) => {
                tracing::warn!(
                    connector = %connector_name, %connector_id, error = %e,
                    "failed to mint a composio re-auth link for an inline HITL pause"
                );
            }
        }
    }

    match nasiko_hitl::repo::create_pending_auth_required_with_ttl(
        &state.db,
        nasiko_hitl::NewAuthRequired {
            agent_id,
            owner_user_id: user_id,
            connector_id,
            context_id,
            question,
        },
        state.config.hitl_request_ttl_days,
    )
    .await
    {
        Ok(request) => err_data(
            req_id,
            codes::AUTH_REQUIRED,
            format!(
                "Authentication required for connector '{connector_name}'. A request has been \
                 recorded (id {}); ask the user to re-authenticate, then retry this tool.",
                request.id
            ),
            json!({ "connector": connector_name, "connector_id": connector_id, "hitl_request_id": request.id }),
        ),
        Err(e) => {
            tracing::error!(connector = %connector_name, %connector_id, error = %e, "failed to persist auth_required hitl request");
            generic_error()
        }
    }
}

/// Best-effort: persist a pending `tool_approval` row for one `(connector,
/// tool)` `Stance::Ask` decision (M2's store), returning its id. `None` when
/// there's no verified flow to resolve a `context_id` from (required by
/// `chk_hitl_tool_approval_identity`), or on a DB failure (logged) — either
/// way the caller still returns `TOOL_ASK`; persistence never changes the
/// ask/deny decision itself, only whether a row exists to resolve against
/// later. Shared by the single-tool ask path (`ask_with_hitl_request`) and
/// the `COMPOSIO_MULTI_EXECUTE_TOOL` batch-ask path, which persists one row
/// per asked tool.
///
/// Takes `verified_flow_id`, never the raw `traceparent` — a `context_id`
/// seeded from an unverified trace id would let a coding-agent row's
/// owner-fallback call (no participant of the flow it names) plant a
/// resolvable HITL row against someone else's conversation; see
/// `oss/hitl/src/notifier.rs`'s resume path for what approving that row
/// would otherwise do with it.
async fn create_tool_approval_id(
    state: &McpState,
    user_id: Uuid,
    agent_id: Uuid,
    connector_id: Uuid,
    tool_name: &str,
    verified_flow_id: Option<&str>,
) -> Option<Uuid> {
    let context_id = match session::resolve_context_id(state, verified_flow_id).await {
        Some(id) => id,
        None => {
            tracing::warn!(
                tool = %tool_name, %connector_id,
                "tool_approval ask with no verified flow to resolve a context_id from — skipping hitl persistence"
            );
            return None;
        }
    };

    let question = json!({
        "connector_id": connector_id,
        "tool_name": tool_name,
        "message": format!("Tool '{tool_name}' requires user approval before it can run."),
    });

    match nasiko_hitl::repo::create_pending_tool_approval_with_ttl(
        &state.db,
        nasiko_hitl::NewToolApproval {
            agent_id,
            owner_user_id: user_id,
            connector_id,
            tool_name: tool_name.to_string(),
            context_id,
            question,
        },
        state.config.hitl_request_ttl_days,
    )
    .await
    {
        Ok(request) => Some(request.id),
        Err(e) => {
            tracing::error!(tool = %tool_name, %connector_id, error = %e, "failed to persist tool_approval hitl request");
            None
        }
    }
}

/// Outcome of checking whether an `Ask`-decision tool call has already been
/// resolved by a human, for M7's retry-matching lookup
/// ([`resolve_tool_approval_retry`]).
enum RetryOutcome {
    /// A session grant is active, or a previously approved `once`-scope
    /// request was atomically claimed — proceed with the tool call as if
    /// `Stance::Allow` had matched.
    Proceed,
    /// A previously rejected request was atomically claimed — fail the call
    /// outright instead of asking again.
    Denied,
    /// Nothing to resume against (no session grant, no unconsumed resolved
    /// row, or no `context_id` to look either up by) — ask, same as before M7.
    AskAgain,
}

/// M7's retry-matching lookup: called immediately after `perms.decide()`
/// returns `Ask`, before falling back to the normal ask-and-persist path.
/// Checks, in order: (1) an unexpired `mcp_session_tool_grants` row for this
/// exact `(agent_id, connector_id, tool_name, context_id)` tuple — reusable
/// for the rest of the conversation, never consumed; (2) failing that, an
/// atomic claim ([`nasiko_hitl::repo::claim_resolved_tool_approval`]) of the
/// most recent resolved, unconsumed row for the same tuple — single-use,
/// covers both an approved `once` retry and a rejected retry. A DB error at
/// either step degrades to `AskAgain` (logged) rather than failing the call
/// outright — the pre-M7 behavior (ask again) is always a safe fallback.
/// Takes a single `(connector_id, tool_name)` pair, so M8 reuses it unchanged
/// per-slug inside the `COMPOSIO_MULTI_EXECUTE_TOOL` batch loop, alongside its
/// two pre-existing single-tool call sites.
///
/// Takes `verified_flow_id`, never the raw `traceparent` — see
/// `create_tool_approval_id`'s doc comment for why a `context_id` must never
/// be seeded from a trace id this agent wasn't proven to participate in.
async fn resolve_tool_approval_retry(
    state: &McpState,
    user_id: Uuid,
    agent_id: Uuid,
    connector_id: Uuid,
    tool_name: &str,
    verified_flow_id: Option<&str>,
) -> RetryOutcome {
    let Some(context_id) = session::resolve_context_id(state, verified_flow_id).await else {
        return RetryOutcome::AskAgain;
    };

    // `session`-scope grants are keyed by the stable chat-session identity,
    // not the per-message trace context above (see
    // `repo::resolve_stable_session_context`'s own doc comment for the full
    // reasoning) — `once`-scope claiming below deliberately keeps using
    // `context_id` unchanged.
    let session_context_id = match nasiko_hitl::repo::resolve_stable_session_context(
        &state.db,
        user_id,
        agent_id,
        &context_id,
    )
    .await
    {
        Ok(Some(session_id)) => session_id,
        Ok(None) => context_id.clone(),
        Err(e) => {
            tracing::warn!(
                %agent_id, %user_id, error = %e,
                "stable session lookup failed — falling back to trace context for session-grant matching"
            );
            context_id.clone()
        }
    };

    match nasiko_hitl::repo::has_active_session_grant(
        &state.db,
        user_id,
        agent_id,
        connector_id,
        tool_name,
        &session_context_id,
    )
    .await
    {
        Ok(true) => return RetryOutcome::Proceed,
        Ok(false) => {}
        Err(e) => {
            tracing::error!(
                %agent_id, %connector_id, tool = %tool_name, error = %e,
                "session grant lookup failed — falling back to ask"
            );
            return RetryOutcome::AskAgain;
        }
    }

    match nasiko_hitl::repo::claim_resolved_tool_approval(
        &state.db,
        user_id,
        agent_id,
        connector_id,
        tool_name,
        &context_id,
    )
    .await
    {
        Ok(Some(row)) => {
            let approved = row
                .human_response
                .as_ref()
                .and_then(|r| r.get("decision"))
                .and_then(Value::as_str)
                // `DECISION_APPROVE`, not a literal: this is the site that decides whether a
                // blocked tool call proceeds, so a wire-vocabulary change that missed it would
                // silently read every previously-approved retry as `Denied` (found in review —
                // `notifier.rs` was migrated to the constant, this one was not).
                == Some(nasiko_hitl::DECISION_APPROVE);
            if approved {
                RetryOutcome::Proceed
            } else {
                RetryOutcome::Denied
            }
        }
        Ok(None) => RetryOutcome::AskAgain,
        Err(e) => {
            tracing::error!(
                %agent_id, %connector_id, tool = %tool_name, error = %e,
                "tool_approval retry-claim failed — falling back to ask"
            );
            RetryOutcome::AskAgain
        }
    }
}

/// The single-tool `TOOL_ASK` response, enriched with a `hitl_request_id`
/// when persistence (`create_tool_approval_id`) succeeds. The response shape
/// and code are unchanged from before M4 when persistence doesn't happen —
/// `hitl_request_id` is purely additive in `data`.
#[allow(clippy::too_many_arguments)]
async fn ask_with_hitl_request(
    state: &McpState,
    user_id: Uuid,
    agent_id: Uuid,
    connector_id: Uuid,
    tool_name: &str,
    connector_label: &str,
    req_id: &Value,
    verified_flow_id: Option<&str>,
) -> Value {
    let mut data = json!({ "server": connector_label });
    if let Some(id) = create_tool_approval_id(
        state,
        user_id,
        agent_id,
        connector_id,
        tool_name,
        verified_flow_id,
    )
    .await
    {
        data["hitl_request_id"] = json!(id);
    }
    err_data(
        req_id,
        codes::TOOL_ASK,
        format!("Tool '{tool_name}' requires user approval. Grant access in the agent settings."),
        data,
    )
}

/// A connection-level failure (refused/timeout/DNS) — as opposed to an
/// application-level MCP error (a well-formed error response from a live
/// server) — is the only case worth refreshing the endpoint for; nothing else
/// indicates the address itself is stale.
fn is_connection_level_failure(e: &McpError) -> bool {
    matches!(e, McpError::Http(re) if re.is_connect() || re.is_timeout())
}

/// True when a Composio toolkit maps to a connector that is disabled for the agent.
fn connector_disabled(
    resolved: &ResolvedSession,
    perms: &PermissionContext,
    toolkit: &str,
) -> bool {
    resolved
        .toolkit_to_connector
        .get(&toolkit.to_ascii_lowercase())
        .map(|cid| !perms.is_connector_enabled(*cid))
        .unwrap_or(false)
}

/// Unwrap a `COMPOSIO_MULTI_EXECUTE_TOOL` response back to a normal tool
/// response the agent expects.
///
/// MULTI_EXECUTE returns:
/// ```json
/// { "jsonrpc": "2.0", "id": 1, "result": {
///     "content": [{ "type": "text", "text": "[{\"data\": {...}, ...}]" }]
/// }}
/// ```
///
/// We extract the inner JSON from the first `content[].text` entry and return
/// it as a standard `result.content[].text` with the unwrapped payload.
/// Unwrap a `COMPOSIO_MULTI_EXECUTE_TOOL` response back to the clean tool
/// result the agent expects.
///
/// MULTI_EXECUTE returns:
/// ```text
/// result.content[0].text = JSON string of {
///   "data": { "results": [{ "response": { "successful": bool, "data": {…} }, … }] },
///   "successful": bool
/// }
/// ```
///
/// We extract `data.results[0].response.data` (the actual tool output) and
/// return it as a clean `result.content[0].text`.
fn unwrap_multi_execute_response(response: Value) -> Value {
    let text = response
        .get("result")
        .and_then(|r| r.get("content"))
        .and_then(|c| c.as_array())
        .and_then(|arr| arr.first())
        .and_then(|entry| entry.get("text"))
        .and_then(|t| t.as_str());

    let Some(text) = text else {
        return response;
    };

    // Parse the stringified JSON object from MULTI_EXECUTE.
    let Ok(parsed) = serde_json::from_str::<Value>(text) else {
        return response;
    };

    // Extract the first tool result from data.results[0].
    let first_result = parsed
        .get("data")
        .and_then(|d| d.get("results"))
        .and_then(|r| r.as_array())
        .and_then(|arr| arr.first());

    let inner_text = match first_result {
        Some(result) => {
            let tool_response = result.get("response");
            let successful = tool_response
                .and_then(|r| r.get("successful"))
                .and_then(|s| s.as_bool())
                .unwrap_or(false);

            if successful {
                // Success: return response.data (the actual tool output).
                let data = tool_response
                    .and_then(|r| r.get("data"))
                    .cloned()
                    .unwrap_or(json!({"success": true}));
                serde_json::to_string(&data).unwrap_or_else(|_| text.to_string())
            } else {
                // Failure: return a clean error message.
                let error = result
                    .get("error")
                    .and_then(|e| e.as_str())
                    .or_else(|| {
                        tool_response
                            .and_then(|r| r.get("data"))
                            .and_then(|d| d.get("message"))
                            .and_then(|m| m.as_str())
                    })
                    .unwrap_or("tool execution failed");
                serde_json::to_string(&json!({"error": error})).unwrap_or_else(|_| text.to_string())
            }
        }
        None => text.to_string(),
    };

    // Rebuild as a standard MCP tool response.
    let mut out = response.clone();
    out["result"] = json!({
        "content": [{
            "type": "text",
            "text": inner_text
        }]
    });
    out
}

#[cfg(test)]
mod tests {

    // ── IP-5: recover_compressed ────────────────────────────────────────────

    #[test]
    fn recovery_handle_parses_out_of_a_marker_however_the_model_copies_it() {
        let id = "9f3a1c2e-4b5d-6e7f-8a9b-0c1d2e3f4a5b";
        for raw in [
            id.to_string(),
            format!("nasiko://c/{id}"),
            format!(" nasiko://c/{id} "),
            format!("`{id}`"),
            format!("[{id}]"),
        ] {
            assert_eq!(
                parse_recovery_handle(&raw).map(|u| u.to_string()),
                Some(id.to_string()),
                "failed to parse: {raw}"
            );
        }
    }

    #[test]
    fn a_non_handle_is_rejected_rather_than_guessed() {
        for raw in ["", "nasiko://c/", "not-a-uuid", "../../etc/passwd"] {
            assert!(parse_recovery_handle(raw).is_none(), "accepted: {raw}");
        }
    }

    #[test]
    fn flow_id_is_the_trace_id_of_the_traceparent() {
        assert_eq!(
            flow_id_of("00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01").as_deref(),
            Some("4bf92f3577b34da6a3ce929d0e0e4736")
        );
        // An all-zero trace id is the "no trace" sentinel, not a flow.
        assert!(flow_id_of("00-00000000000000000000000000000000-00f067aa0ba902b7-01").is_none());
        assert!(flow_id_of("garbage").is_none());
    }

    #[test]
    fn recover_compressed_is_offered_to_every_agent() {
        // The handle only ever appears inside an elision marker, so an agent that has not been
        // told the tool exists cannot act on the marker it is given.
        let def = recover_compressed_definition();
        assert_eq!(def["name"], "recover_compressed");
        assert!(def["inputSchema"]["properties"]["handle"].is_object());
        assert_eq!(def["inputSchema"]["required"][0], "handle");
    }

    use super::*;
    use crate::config::{McpConfig, ToolSearchMode};
    use crate::permissions::PermissionRule;
    use crate::provider::{GenericMcpProvider, Providers};
    use crate::types::Stance;
    use std::collections::HashMap;
    use std::sync::Arc;

    #[test]
    fn implements_matches_the_dispatch_match_arms() {
        for method in ["initialize", "ping", "tools/list", "tools/call"] {
            assert!(implements(method), "{method} must report implemented");
        }
        assert!(
            !implements("server/discover"),
            "an unhandled method must report unimplemented so the route layer's \
             MCP-Protocol-Version gate does not block a streamable-http client's \
             fallback probe"
        );
    }

    /// Step 0 (carry-over hardening): an unimplemented method must be rejected
    /// by the `implements()` gate before `load_permission_context`/
    /// `resolve_session` ever run. `test_state()`'s pool is lazily connected
    /// to an unreachable address, so this would hang/error instead of
    /// returning promptly if the gate ran after either DB call.
    #[tokio::test]
    async fn unknown_method_is_rejected_before_permission_or_session_work() {
        let state = test_state();
        let body = json!({"jsonrpc": "2.0", "id": 1, "method": "server/discover"});
        let res = handle_request(&state, Uuid::new_v4(), Uuid::new_v4(), &body, None, None)
            .await
            .expect("a request with an id must produce a response");
        assert_eq!(
            res["error"]["code"],
            json!(codes::METHOD_NOT_FOUND),
            "{res}"
        );
    }

    fn test_state() -> McpState {
        let db = sqlx::PgPool::connect_lazy("postgres://user:pass@127.0.0.1:1/db")
            .expect("lazy pool construction must not touch the network");
        let redis = redis::Client::open("redis://127.0.0.1:1/").expect("lazy redis client");
        McpState {
            db,
            redis,
            http_client: reqwest::Client::new(),
            guarded_http_client: reqwest::Client::new(),
            config: McpConfig {
                composio_api_key: None,
                composio_base_url: "http://localhost".to_string(),
                composio_webhook_secret: None,
                gateway_public_url: None,
                oauth_redirect_base_url: None,
                composio_callback_base_url: None,
                session_ttl_seconds: 60,
                perm_cache_ttl_seconds: 60,
                manifest_ttl_seconds: 60,
                toolcount_ttl_seconds: 3600,
                oauth_state_signing_key: "test".to_string(),
                description_model: "gpt-4o-mini".to_string(),
                hitl_request_ttl_days: 7,
                tool_search_mode: ToolSearchMode::Semantic,
                tool_search_tool_limit: 0,
                tool_search_meta_limit: 0,
                openai_api_key: None,
                embedding_model: "".to_string(),
                gateway_instructions: String::new(),
                identity_signing_key: b"test-identity-signing-key".to_vec(),
            },
            providers: Providers {
                composio: None,
                mcp: GenericMcpProvider::new(reqwest::Client::new(), reqwest::Client::new()),
            },
            authorizer: std::sync::Arc::new(crate::authorizer::OssConnectorAuthorizer),
            endpoint_refresher: std::sync::Arc::new(crate::endpoint_refresh::NoopEndpointRefresher),
            llm: nasiko_orchestrator::providers::LLMProvider::from_env(reqwest::Client::new()),
            search_index: Arc::new(crate::search::NoopSearchIndex),
        }
    }

    /// A resolved session with one Composio backend at `url` and a single
    /// connected `gmail` toolkit mapped to `cid`.
    fn gmail_session(url: &str, cid: Uuid) -> ResolvedSession {
        ResolvedSession {
            servers: vec![MCPServerConfig {
                connector_id: Uuid::nil(),
                kind: ServerType::Composio,
                name: "composio".into(),
                url: url.into(),
                headers: HashMap::new(),
                transport: "streamable_http".into(),
                trusted: false,
                system: false,
                tool_names: vec![],
                instructions: None,
            }],
            connected_toolkits: vec!["gmail".into()],
            toolkit_to_connector: HashMap::from([("gmail".to_string(), cid)]),
            unusable_connectors: HashMap::new(),
        }
    }

    /// `enabled` must be given explicitly — under the default-deny allowlist,
    /// a connector referenced only by `rules` (with no tool rule at all, e.g.
    /// a bare "this connector is on" case) can't be inferred from `rules`
    /// alone.
    fn perms(enabled: &[Uuid], rules: Vec<PermissionRule>) -> PermissionContext {
        PermissionContext {
            agent_id: Uuid::nil(),
            enabled_connectors: enabled.iter().copied().collect(),
            rules,
            hash: "h".into(),
        }
    }

    fn rule(cid: Uuid, pat: &str, stance: Stance) -> PermissionRule {
        PermissionRule {
            connector_id: cid,
            tool_pattern: pat.into(),
            stance,
        }
    }

    /// Layer-1 stub that always allows — the real `OssConnectorAuthorizer`
    /// hits `state.db`, which `test_state()`'s lazily-connected pool can't
    /// actually reach; tests exercising the generic-MCP (`ServerType::Mcp`)
    /// path need this instead.
    struct AllowAllAuthorizer;
    #[async_trait::async_trait]
    impl crate::authorizer::ConnectorAuthorizer for AllowAllAuthorizer {
        async fn can_access_connector(
            &self,
            _db: &sqlx::PgPool,
            _user_id: Uuid,
            _connector_id: Uuid,
        ) -> crate::error::Result<bool> {
            Ok(true)
        }
        async fn list_accessible_connectors(
            &self,
            _db: &sqlx::PgPool,
            _user_id: Uuid,
        ) -> crate::error::Result<Vec<crate::repo::McpConnector>> {
            Ok(vec![])
        }
        async fn list_accessible_mcp_connectors(
            &self,
            _db: &sqlx::PgPool,
            _user_id: Uuid,
        ) -> crate::error::Result<Vec<crate::repo::McpConnector>> {
            Ok(vec![])
        }
        async fn list_access_reasons(
            &self,
            _db: &sqlx::PgPool,
            _connector: &crate::repo::McpConnector,
        ) -> crate::error::Result<Vec<crate::types::AccessReason>> {
            Ok(vec![])
        }
        async fn list_org_grant_consumers(
            &self,
            _db: &sqlx::PgPool,
            _connector_id: Uuid,
        ) -> crate::error::Result<(
            Vec<crate::types::OrgGrantConsumer>,
            Vec<crate::types::OrgGrantConsumer>,
        )> {
            Ok((vec![], vec![]))
        }
    }

    /// Always refreshes to a fixed URL — a fake
    /// [`crate::endpoint_refresh::EndpointRefresher`] standing in for the
    /// real `ContainerRuntime`-backed one (`oss/server`'s
    /// `RuntimeEndpointRefresher`, not constructible from this crate).
    struct FakeRefresher(String);
    #[async_trait::async_trait]
    impl crate::endpoint_refresh::EndpointRefresher for FakeRefresher {
        async fn refresh(&self, _connector_id: Uuid) -> Option<String> {
            Some(self.0.clone())
        }
    }

    /// Like [`FakeRefresher`], but counts invocations — lets a test assert
    /// the refresher was never called at all (the system-server exclusion),
    /// not just that a call's return value went unused.
    struct CountingRefresher {
        url: String,
        calls: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    }
    #[async_trait::async_trait]
    impl crate::endpoint_refresh::EndpointRefresher for CountingRefresher {
        async fn refresh(&self, _connector_id: Uuid) -> Option<String> {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Some(self.url.clone())
        }
    }

    /// A resolved session with one generic MCP backend (`ServerType::Mcp`,
    /// `trusted`) at `url`, namespaced under `cid`'s connector prefix.
    fn mcp_session(url: &str, cid: Uuid, trusted: bool) -> ResolvedSession {
        ResolvedSession {
            servers: vec![MCPServerConfig {
                connector_id: cid,
                kind: ServerType::Mcp,
                name: "uploaded-server".into(),
                url: url.into(),
                headers: HashMap::new(),
                transport: "streamable_http".into(),
                trusted,
                system: false,
                tool_names: vec![],
                instructions: None,
            }],
            connected_toolkits: vec![],
            toolkit_to_connector: HashMap::new(),
            unusable_connectors: HashMap::new(),
        }
    }

    /// Same shape as [`mcp_session`], but a SYSTEM backend (`trusted: true`,
    /// `system: true`) — the prefixed-tool-name routing path (`{prefix}__x`)
    /// works identically for a system server (only bare-name routing treats
    /// `system` specially), so this can reuse the exact connection-failure
    /// scenario the uploaded-build tests above use.
    fn system_mcp_session(url: &str, cid: Uuid) -> ResolvedSession {
        ResolvedSession {
            servers: vec![MCPServerConfig {
                connector_id: cid,
                kind: ServerType::Mcp,
                name: "system-server".into(),
                url: url.into(),
                headers: HashMap::new(),
                transport: "streamable_http".into(),
                trusted: true,
                system: true,
                tool_names: vec![],
                instructions: None,
            }],
            connected_toolkits: vec![],
            toolkit_to_connector: HashMap::new(),
            unusable_connectors: HashMap::new(),
        }
    }

    /// A mockito server answering `POST /mcp` with a successful JSON-RPC
    /// response — the "refreshed, now-reachable" address a retry lands on.
    /// Uses a `localhost`-hostname URL, not mockito's raw `127.0.0.1` form,
    /// per Step 7's own established gotcha (the first attempt to fail
    /// against a loopback URL): reqwest/hyper's normal request path handles
    /// both equally for a real request (unlike the SSRF guard's custom
    /// `Resolve` trait, which only fires for hostnames) — matching that
    /// convention here regardless, for consistency with this crate's other
    /// tests.
    async fn spawn_ok_backend() -> (mockito::ServerGuard, String) {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("POST", "/mcp")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"jsonrpc":"2.0","id":1,"result":{"ok":true}}"#)
            .create_async()
            .await;
        let url = format!("http://localhost:{}/mcp", server.socket_address().port());
        (server, url)
    }

    // ── Step 13: endpoint self-heal on connection failure ────────────────────

    #[tokio::test]
    async fn trusted_backend_connection_failure_retries_against_refreshed_endpoint() {
        let (_guard, fresh_url) = spawn_ok_backend().await;
        let mut state = test_state();
        state.authorizer = std::sync::Arc::new(AllowAllAuthorizer);
        state.endpoint_refresher = std::sync::Arc::new(FakeRefresher(fresh_url));

        let cid = Uuid::new_v4();
        // Port 1 is a well-known refused-connection target — this is a
        // genuine connection-level failure, not an application error.
        let resolved = mcp_session("http://127.0.0.1:1/mcp", cid, true);
        let p = perms(&[cid], vec![]);
        let tool = format!("{}__echo", crate::types::connector_prefix(cid));

        let res = handle_tools_call(
            &state,
            Uuid::new_v4(),
            &json!(1),
            &json!({ "name": tool, "arguments": {} }),
            &resolved,
            &p,
            None,
            None,
        )
        .await;

        assert_eq!(
            res["result"]["ok"],
            json!(true),
            "must succeed after retrying against the refreshed endpoint: {res}"
        );
    }

    #[tokio::test]
    async fn untrusted_backend_connection_failure_never_retries() {
        // An external_url connector (trusted=false) must never trigger a
        // refresh, even if the refresher would happily hand back a working
        // URL — refresh only ever applies to uploaded_build connectors.
        let (_guard, fresh_url) = spawn_ok_backend().await;
        let mut state = test_state();
        state.authorizer = std::sync::Arc::new(AllowAllAuthorizer);
        state.endpoint_refresher = std::sync::Arc::new(FakeRefresher(fresh_url));

        let cid = Uuid::new_v4();
        let resolved = mcp_session("http://127.0.0.1:1/mcp", cid, false);
        let p = perms(&[cid], vec![]);
        let tool = format!("{}__echo", crate::types::connector_prefix(cid));

        let res = handle_tools_call(
            &state,
            Uuid::new_v4(),
            &json!(1),
            &json!({ "name": tool, "arguments": {} }),
            &resolved,
            &p,
            None,
            None,
        )
        .await;

        assert_eq!(
            res["error"]["code"],
            json!(codes::INTERNAL_ERROR),
            "must surface the original failure, never retry: {res}"
        );
    }

    #[tokio::test]
    async fn system_backend_connection_failure_never_triggers_endpoint_refresh() {
        // A system connector's `trusted` is also `true` (loopback, same as an
        // uploaded build) — without the `!server.system` exclusion this would
        // wrongly take the uploaded-build self-heal path and ask the
        // refresher for a `ContainerId` that corresponds to no real
        // container. Asserts the refresher is never even called, not merely
        // that its result goes unused.
        let (_guard, fresh_url) = spawn_ok_backend().await;
        let mut state = test_state();
        state.authorizer = std::sync::Arc::new(AllowAllAuthorizer);
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        state.endpoint_refresher = std::sync::Arc::new(CountingRefresher {
            url: fresh_url,
            calls: calls.clone(),
        });

        let cid = Uuid::new_v4();
        let resolved = system_mcp_session("http://127.0.0.1:1/mcp", cid);
        let p = perms(&[cid], vec![]);
        let tool = format!("{}__echo", crate::types::connector_prefix(cid));

        let res = handle_tools_call(
            &state,
            Uuid::new_v4(),
            &json!(1),
            &json!({ "name": tool, "arguments": {} }),
            &resolved,
            &p,
            None,
            None,
        )
        .await;

        assert_eq!(
            res["error"]["code"],
            json!(codes::INTERNAL_ERROR),
            "must surface the original failure, never retry: {res}"
        );
        assert_eq!(
            calls.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "the refresher must never be invoked for a system backend"
        );
    }

    // ── Round 3: direct Composio tool calls must be permission-enforced ──────

    #[tokio::test]
    async fn composio_direct_tool_with_block_rule_is_denied_before_backend() {
        // url points nowhere reachable — a Denied decision must return before any
        // backend call, so this must not hang or error on the network.
        let cid = Uuid::new_v4();
        let resolved = gmail_session("http://127.0.0.1:9/mcp", cid);
        let p = perms(&[cid], vec![rule(cid, "GMAIL_SEND_*", Stance::Block)]);
        let res = handle_tools_call(
            &test_state(),
            Uuid::new_v4(),
            &json!(1),
            &json!({ "name": "GMAIL_SEND_EMAIL", "arguments": {} }),
            &resolved,
            &p,
            None,
            None,
        )
        .await;
        assert_eq!(res["error"]["code"], json!(codes::TOOL_BLOCKED), "{res}");
    }

    #[tokio::test]
    async fn composio_direct_tool_on_disabled_connector_is_denied() {
        let cid = Uuid::new_v4();
        let resolved = gmail_session("http://127.0.0.1:9/mcp", cid);
        let p = perms(&[], vec![]); // never enabled for the agent
        let res = handle_tools_call(
            &test_state(),
            Uuid::new_v4(),
            &json!(1),
            &json!({ "name": "GMAIL_SEND_EMAIL", "arguments": {} }),
            &resolved,
            &p,
            None,
            None,
        )
        .await;
        assert_eq!(res["error"]["code"], json!(codes::TOOL_BLOCKED), "{res}");
    }

    #[tokio::test]
    async fn composio_direct_tool_with_ask_rule_returns_tool_ask() {
        let cid = Uuid::new_v4();
        let resolved = gmail_session("http://127.0.0.1:9/mcp", cid);
        let p = perms(&[cid], vec![rule(cid, "*", Stance::Ask)]);
        let res = handle_tools_call(
            &test_state(),
            Uuid::new_v4(),
            &json!(1),
            &json!({ "name": "GMAIL_SEND_EMAIL", "arguments": {} }),
            &resolved,
            &p,
            None,
            None,
        )
        .await;
        assert_eq!(res["error"]["code"], json!(codes::TOOL_ASK), "{res}");
        assert!(
            res["error"]["data"].get("hitl_request_id").is_none(),
            "no traceparent means no context_id to persist against — the response must not \
             claim a hitl_request_id that doesn't exist: {res}"
        );
    }

    #[tokio::test]
    async fn composio_allowed_direct_tool_reaches_backend() {
        let mut backend = mockito::Server::new_async().await;
        let hit = backend
            .mock("POST", "/mcp")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"jsonrpc":"2.0","id":1,"result":{"ok":true}}"#)
            .expect(1)
            .create_async()
            .await;

        let cid = Uuid::new_v4();
        let resolved = gmail_session(&format!("{}/mcp", backend.url()), cid);
        let p = perms(&[cid], vec![]); // explicitly enabled, no tool rules
        let res = handle_tools_call(
            &test_state(),
            Uuid::new_v4(),
            &json!(1),
            &json!({ "name": "GMAIL_SEND_EMAIL", "arguments": {} }),
            &resolved,
            &p,
            None,
            None,
        )
        .await;
        assert_eq!(res["result"]["ok"], json!(true), "{res}");
        hit.assert_async().await;
    }

    #[tokio::test]
    async fn composio_cross_toolkit_metatool_is_not_caught_by_per_toolkit_check() {
        // COMPOSIO_SEARCH_TOOLS resolves to toolkit "composio" (no connector), so
        // even with the gmail connector disabled it must NOT be denied by the
        // per-toolkit check — it proceeds to the backend (meta-tools are not
        // connector-scoped; MANAGE_CONNECTIONS/MULTI_EXECUTE do their own filtering).
        let mut backend = mockito::Server::new_async().await;
        let hit = backend
            .mock("POST", "/mcp")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"jsonrpc":"2.0","id":1,"result":{"tools":[]}}"#)
            .expect(1)
            .create_async()
            .await;

        let cid = Uuid::new_v4();
        let resolved = gmail_session(&format!("{}/mcp", backend.url()), cid);
        let p = perms(&[], vec![]); // gmail not enabled — irrelevant to a meta-tool
        let res = handle_tools_call(
            &test_state(),
            Uuid::new_v4(),
            &json!(1),
            &json!({ "name": "COMPOSIO_SEARCH_TOOLS", "arguments": {} }),
            &resolved,
            &p,
            None,
            None,
        )
        .await;
        assert!(
            res.get("error").is_none(),
            "a cross-toolkit meta-tool must not be blocked by the per-toolkit check: {res}"
        );
        hit.assert_async().await;
    }

    // ─── M3: AuthRequired routing ───────────────────────────────────────────

    /// A resolved session with an empty `servers` list and one connector
    /// recorded as unusable — mirrors what `session::resolve_session`
    /// produces when a connector's credential is missing/expired/disabled.
    fn unusable_mcp_session(cid: Uuid, reason: ConnectorUnusable, name: &str) -> ResolvedSession {
        ResolvedSession {
            servers: vec![],
            connected_toolkits: vec![],
            toolkit_to_connector: HashMap::new(),
            unusable_connectors: HashMap::from([(
                cid,
                crate::types::UnusableConnector {
                    reason,
                    name: name.to_string(),
                },
            )]),
        }
    }

    #[tokio::test]
    async fn auth_required_without_traceparent_falls_back_to_generic_error() {
        // No traceparent means nothing to correlate a resumable conversation
        // against; `resolve_context_id` short-circuits before touching the
        // DB, so this stays hermetic even though `test_state()`'s pool can't
        // reach a real Postgres. Confirms the pre-M3 message/code survive
        // unchanged for this fallback.
        let cid = Uuid::new_v4();
        let resolved = unusable_mcp_session(cid, ConnectorUnusable::AuthRequired, "github");
        // Enabled for this agent — this test is about the missing-traceparent fallback, not the
        // per-agent connector gate (see `auth_required_on_disabled_connector_is_denied_before_disclosure`
        // for that case), so a disabled connector must not short-circuit before ever reaching it.
        let p = perms(&[cid], vec![]);
        let tool = format!("{}__list_repos", crate::types::connector_prefix(cid));

        let res = handle_tools_call(
            &test_state(),
            Uuid::new_v4(),
            &json!(1),
            &json!({ "name": tool, "arguments": {} }),
            &resolved,
            &p,
            None,
            None,
        )
        .await;

        assert_eq!(res["error"]["code"], json!(codes::INVALID_PARAMS), "{res}");
        assert!(
            res["error"]["message"]
                .as_str()
                .unwrap()
                .contains("not available"),
            "must fall back to the original generic message: {res}"
        );
    }

    #[tokio::test]
    async fn auth_required_on_disabled_connector_is_denied_before_disclosure() {
        // A connector the admin disabled for this agent must never reach
        // `handle_auth_required` — that would disclose the connector's name/UUID to the agent
        // and file a pending `auth_required` row asking the human to re-authenticate a connector
        // this agent isn't permitted to use, even though `perms.decide` would still correctly
        // block the actual call. `is_connector_enabled` (Layer 2's connector-level gate) must run
        // BEFORE the `AuthRequired` short-circuit, not after.
        let cid = Uuid::new_v4();
        let resolved = unusable_mcp_session(cid, ConnectorUnusable::AuthRequired, "github");
        let p = perms(&[], vec![]); // never enabled for the agent
        let tool = format!("{}__list_repos", crate::types::connector_prefix(cid));

        let res = handle_tools_call(
            &test_state(),
            Uuid::new_v4(),
            &json!(1),
            &json!({ "name": tool, "arguments": {} }),
            &resolved,
            &p,
            None,
            None,
        )
        .await;

        assert_eq!(res["error"]["code"], json!(codes::TOOL_BLOCKED), "{res}");
    }

    #[tokio::test]
    async fn non_auth_required_reason_never_takes_the_auth_required_branch() {
        // NotConfigured/MissingCredential must still produce today's generic
        // "not available" error — only AuthRequired gets the new handling.
        // A (well-formed but unresolvable) traceparent AND a matching
        // `verified_flow_id` are deliberately supplied here: if the reason
        // gate in `handle_tools_call` were ever loosened to match on
        // `Some(_)` instead of `AuthRequired` specifically, this would start
        // touching `state.db` (`session::resolve_context_id`'s
        // `session_traces` lookup) — and, since `test_state()`'s pool can't
        // reach a real Postgres, still degrade to the same generic error
        // (not crash), but the whole point of the gate is to skip that DB
        // work entirely for non-AuthRequired reasons. Passing `None` here would
        // make this a no-op regardless of the gate — `resolve_context_id`
        // short-circuits on `None` before ever reaching `state.db` — so a
        // loosened gate would slip past this test undetected.
        let cid = Uuid::new_v4();
        let resolved = unusable_mcp_session(cid, ConnectorUnusable::NotConfigured, "github");
        let p = perms(&[], vec![]);
        let tool = format!("{}__list_repos", crate::types::connector_prefix(cid));

        let res = handle_tools_call(
            &test_state(),
            Uuid::new_v4(),
            &json!(1),
            &json!({ "name": tool, "arguments": {} }),
            &resolved,
            &p,
            Some("00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01"),
            Some("0af7651916cd43dd8448eb211c80319c"),
        )
        .await;

        assert_eq!(res["error"]["code"], json!(codes::INVALID_PARAMS), "{res}");
    }

    // ─── M4: ToolApproval persistence ───────────────────────────────────────

    #[tokio::test]
    async fn generic_mcp_ask_without_traceparent_returns_tool_ask_without_hitl_id() {
        // Same "no context_id, no DB touch" guarantee as AuthRequired's own
        // fallback (this test would hang/error against test_state()'s
        // unreachable pool if the ask path unconditionally tried to
        // persist) — Layer 2's Ask decision must still return TOOL_ASK even
        // when there's nothing to persist against.
        let mut state = test_state();
        state.authorizer = std::sync::Arc::new(AllowAllAuthorizer);
        let cid = Uuid::new_v4();
        let resolved = mcp_session("http://127.0.0.1:9/mcp", cid, false);
        let p = perms(&[cid], vec![rule(cid, "*", Stance::Ask)]);
        let tool = format!("{}__echo", crate::types::connector_prefix(cid));

        let res = handle_tools_call(
            &state,
            Uuid::new_v4(),
            &json!(1),
            &json!({ "name": tool, "arguments": {} }),
            &resolved,
            &p,
            None,
            None,
        )
        .await;

        assert_eq!(res["error"]["code"], json!(codes::TOOL_ASK), "{res}");
        assert!(
            res["error"]["data"].get("hitl_request_id").is_none(),
            "no traceparent means nothing was persisted: {res}"
        );
    }

    #[tokio::test]
    async fn generic_mcp_block_and_allow_are_unaffected_by_m4() {
        // Preserve-existing-behavior guard: Denied and Allowed decisions
        // never touch the new persistence path at all (only `Ask` does).
        let mut state = test_state();
        state.authorizer = std::sync::Arc::new(AllowAllAuthorizer);
        let cid = Uuid::new_v4();
        let resolved = mcp_session("http://127.0.0.1:9/mcp", cid, false);
        let tool = format!("{}__echo", crate::types::connector_prefix(cid));

        let blocked = handle_tools_call(
            &state,
            Uuid::new_v4(),
            &json!(1),
            &json!({ "name": tool, "arguments": {} }),
            &resolved,
            &perms(&[cid], vec![rule(cid, "*", Stance::Block)]),
            None,
            None,
        )
        .await;
        assert_eq!(
            blocked["error"]["code"],
            json!(codes::TOOL_BLOCKED),
            "{blocked}"
        );

        let disabled = handle_tools_call(
            &state,
            Uuid::new_v4(),
            &json!(1),
            &json!({ "name": tool, "arguments": {} }),
            &resolved,
            &perms(&[], vec![]), // connector never enabled
            None,
            None,
        )
        .await;
        assert_eq!(
            disabled["error"]["code"],
            json!(codes::TOOL_BLOCKED),
            "{disabled}"
        );
    }

    // ─── needs_auth_required (S3: DB error vs. genuinely-not-connected) ────────────────────

    fn user_connection(status: &str) -> crate::repo::McpUserConnection {
        let now = chrono::Utc::now();
        crate::repo::McpUserConnection {
            id: Uuid::new_v4(),
            user_id: Uuid::new_v4(),
            connector_id: Uuid::new_v4(),
            status: status.to_string(),
            connected_account_id: None,
            redirect_url: None,
            oauth_url: None,
            encrypted_credential: None,
            encrypted_refresh_token: None,
            token_expires_at: None,
            scope: None,
            created_at: now,
            updated_at: now,
        }
    }

    #[test]
    fn active_connection_does_not_need_auth() {
        assert!(!needs_auth_required(&Ok(Some(user_connection("ACTIVE")))));
        // Case-insensitive, matching the live check's `eq_ignore_ascii_case`.
        assert!(!needs_auth_required(&Ok(Some(user_connection("active")))));
    }

    #[test]
    fn expired_or_missing_connection_needs_auth() {
        assert!(needs_auth_required(&Ok(Some(user_connection("EXPIRED")))));
        assert!(needs_auth_required(&Ok(None)));
    }

    /// The actual S3 bug: a DB error must never be treated the same as "genuinely not
    /// connected" — collapsing them (the old `.ok().flatten().is_some_and(...)` chain) fired a
    /// spurious `auth_required` pause on a transient blip, telling the user to re-authenticate a
    /// connector that might be perfectly fine.
    #[test]
    fn db_error_does_not_trigger_a_false_auth_required() {
        // `PoolTimedOut`, not `RowNotFound` — this stands in for a real transient blip
        // (`get_user_connection` uses `fetch_optional`, which never produces `RowNotFound`;
        // that variant means "a `fetch_one` found nothing," not a connectivity failure).
        let db_err: crate::error::Result<Option<crate::repo::McpUserConnection>> =
            Err(sqlx::Error::PoolTimedOut.into());
        assert!(!needs_auth_required(&db_err));
    }

    // ─── MULTI_EXECUTE unwrap tests ────────────────────────────────────────

    #[test]
    fn unwrap_multi_execute_extracts_successful_data() {
        let response = json!({
            "jsonrpc": "2.0",
            "id": "test",
            "result": {
                "content": [{
                    "type": "text",
                    "text": "{\"data\":{\"results\":[{\"response\":{\"successful\":true,\"data\":{\"invitations\":[]}},\"tool_slug\":\"GITHUB_LIST_REPO_INVITATIONS_FOR_AUTH_USER\",\"index\":0}],\"total_count\":1,\"success_count\":1,\"error_count\":0},\"error\":null,\"log_id\":\"log_test\",\"successful\":true}"
                }],
                "isError": false
            }
        });

        let unwrapped = unwrap_multi_execute_response(response);
        let text = unwrapped["result"]["content"][0]["text"].as_str().unwrap();
        let parsed: Value = serde_json::from_str(text).unwrap();
        assert_eq!(
            parsed["invitations"],
            json!([]),
            "should extract the inner data"
        );
    }

    #[test]
    fn unwrap_multi_execute_extracts_error() {
        let response = json!({
            "jsonrpc": "2.0",
            "id": "test",
            "result": {
                "content": [{
                    "type": "text",
                    "text": "{\"data\":{\"results\":[{\"response\":{\"successful\":false,\"data\":{\"message\":\"Not Found\",\"status_code\":404}},\"error\":\"Not Found\",\"tool_slug\":\"TEST\",\"index\":0}],\"total_count\":1,\"success_count\":0,\"error_count\":1},\"error\":\"1 out of 1 tools failed\",\"log_id\":\"log_test\",\"successful\":false}"
                }],
                "isError": true
            }
        });

        let unwrapped = unwrap_multi_execute_response(response);
        let text = unwrapped["result"]["content"][0]["text"].as_str().unwrap();
        let parsed: Value = serde_json::from_str(text).unwrap();
        assert!(
            parsed.get("error").is_some(),
            "should extract error: {parsed}"
        );
    }

    #[test]
    fn unwrap_passes_through_when_no_result_content() {
        let response = json!({
            "jsonrpc": "2.0",
            "id": "test",
            "error": { "code": -32600, "message": "Invalid Request" }
        });
        let unwrapped = unwrap_multi_execute_response(response.clone());
        assert_eq!(unwrapped, response);
    }

    // ─── nasiko_call_tool: the executor for fixed-menu clients ─────────────────

    /// `tools/list` with no verified flow — the agent-startup shape every
    /// fixed-menu client sees first — under `mode`. Hermetic: no flow means no
    /// `flows.title` lookup, and `None` mode's manifest cache treats the
    /// unreachable Redis as a miss.
    async fn listed_tool_names(mode: ToolSearchMode) -> Vec<String> {
        let mut state = test_state();
        state.config.tool_search_mode = mode;
        let res = handle_tools_list(
            &state,
            Uuid::new_v4(),
            &json!(1),
            &[],
            &[],
            &perms(&[], vec![]),
            None,
            None,
        )
        .await;
        res["result"]["tools"]
            .as_array()
            .unwrap_or_else(|| panic!("tools array: {res}"))
            .iter()
            .filter_map(|t| t["name"].as_str().map(str::to_string))
            .collect()
    }

    #[tokio::test]
    async fn nasiko_call_tool_is_listed_exactly_where_nasiko_search_tools_is() {
        // A fixed-menu client can only invoke what `tools/list` carries, so
        // wherever the search meta-tool hides the real tools, the executor
        // that runs a found one must sit beside it.
        for mode in [ToolSearchMode::Semantic, ToolSearchMode::Keyword] {
            let names = listed_tool_names(mode).await;
            assert!(
                names.iter().any(|n| n == "nasiko_search_tools"),
                "{mode:?}: {names:?}"
            );
            assert!(
                names.iter().any(|n| n == "nasiko_call_tool"),
                "{mode:?}: {names:?}"
            );
        }
        // `None` lists the real tools themselves — no search, no executor.
        let names = listed_tool_names(ToolSearchMode::None).await;
        assert!(
            !names.iter().any(|n| n == "nasiko_call_tool"),
            "None mode must not advertise the executor: {names:?}"
        );
        assert!(
            !names.iter().any(|n| n == "nasiko_search_tools"),
            "None mode must not advertise the search meta-tool: {names:?}"
        );
    }

    #[tokio::test]
    async fn nasiko_call_tool_with_invalid_params_is_rejected_before_routing() {
        // 127.0.0.1:9 is unreachable — a rejected executor call must never
        // reach the backend, so this neither hangs nor errors on the network.
        let cid = Uuid::new_v4();
        let resolved = mcp_session("http://127.0.0.1:9/mcp", cid, false);
        let p = perms(&[cid], vec![]);
        let tool = format!("{}__echo", crate::types::connector_prefix(cid));
        for (arguments, expected) in [
            (json!({}), "requires `name`"),
            (json!({"name": 42}), "requires `name`"),
            (json!({"arguments": {}}), "requires `name`"),
            (
                json!({"name": tool, "arguments": "not-an-object"}),
                "must be a JSON object",
            ),
        ] {
            let res = handle_tools_call(
                &test_state(),
                Uuid::new_v4(),
                &json!(1),
                &json!({ "name": "nasiko_call_tool", "arguments": arguments }),
                &resolved,
                &p,
                None,
                None,
            )
            .await;
            assert_eq!(res["error"]["code"], json!(codes::INVALID_PARAMS), "{res}");
            let message = res["error"]["message"].as_str().unwrap_or("");
            assert!(message.contains(expected), "{res}");
        }
    }

    #[tokio::test]
    async fn nasiko_call_tool_refuses_to_call_a_gateway_meta_tool() {
        let cid = Uuid::new_v4();
        let resolved = mcp_session("http://127.0.0.1:9/mcp", cid, false);
        let p = perms(&[cid], vec![]);
        for inner in [
            "nasiko_call_tool",
            "nasiko_search_tools",
            "recover_compressed",
        ] {
            let res = handle_tools_call(
                &test_state(),
                Uuid::new_v4(),
                &json!(1),
                &json!({ "name": "nasiko_call_tool", "arguments": { "name": inner } }),
                &resolved,
                &p,
                None,
                None,
            )
            .await;
            assert_eq!(res["error"]["code"], json!(codes::INVALID_PARAMS), "{res}");
            assert_eq!(
                res["error"]["message"],
                json!("nasiko_call_tool cannot call gateway meta-tools; call them directly"),
                "{res}"
            );
        }
    }

    /// Both entry points, same inner call, same `req_id` — so a byte-identical
    /// comparison is meaningful.
    async fn direct_and_via_executor(
        state: &McpState,
        resolved: &ResolvedSession,
        p: &PermissionContext,
        tool: &str,
        arguments: Value,
    ) -> (Value, Value) {
        let user = Uuid::new_v4();
        let direct = handle_tools_call(
            state,
            user,
            &json!(1),
            &json!({ "name": tool, "arguments": arguments }),
            resolved,
            p,
            None,
            None,
        )
        .await;
        let via_executor = handle_tools_call(
            state,
            user,
            &json!(1),
            &json!({
                "name": "nasiko_call_tool",
                "arguments": { "name": tool, "arguments": arguments },
            }),
            resolved,
            p,
            None,
            None,
        )
        .await;
        (direct, via_executor)
    }

    #[tokio::test]
    async fn nasiko_call_tool_is_indistinguishable_from_the_direct_call_at_the_backend() {
        // The backend accepts only the inner tool's un-namespaced name with the
        // inner arguments, twice: once from the direct call, once unwrapped by
        // the executor. Anything else — the wrapper's own name, or arguments
        // nested one level too deep — misses the mock and fails the call.
        let mut backend = mockito::Server::new_async().await;
        let hit = backend
            .mock("POST", "/mcp")
            .match_body(mockito::Matcher::PartialJson(json!({
                "method": "tools/call",
                "params": { "name": "echo", "arguments": { "path": "notes.md" } },
            })))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"jsonrpc":"2.0","id":1,"result":{"ok":true}}"#)
            .expect(2)
            .create_async()
            .await;
        let url = format!("http://localhost:{}/mcp", backend.socket_address().port());

        let mut state = test_state();
        state.authorizer = std::sync::Arc::new(AllowAllAuthorizer);
        let cid = Uuid::new_v4();
        let resolved = mcp_session(&url, cid, true);
        let p = perms(&[cid], vec![]);
        let tool = format!("{}__echo", crate::types::connector_prefix(cid));

        let (direct, via_executor) =
            direct_and_via_executor(&state, &resolved, &p, &tool, json!({ "path": "notes.md" }))
                .await;

        assert_eq!(direct["result"]["ok"], json!(true), "{direct}");
        assert_eq!(
            via_executor, direct,
            "the executor's response must be byte-identical to the direct call's"
        );
        hit.assert_async().await;
    }

    #[tokio::test]
    async fn nasiko_call_tool_is_blocked_exactly_as_the_direct_call_is() {
        // Unreachable backend on purpose: a Denied decision returns before any
        // backend call, so the executor must also never get that far.
        let mut state = test_state();
        state.authorizer = std::sync::Arc::new(AllowAllAuthorizer);
        let cid = Uuid::new_v4();
        let resolved = mcp_session("http://127.0.0.1:9/mcp", cid, false);
        let p = perms(&[cid], vec![rule(cid, "*", Stance::Block)]);
        let tool = format!("{}__echo", crate::types::connector_prefix(cid));

        let (direct, via_executor) =
            direct_and_via_executor(&state, &resolved, &p, &tool, json!({})).await;

        assert_eq!(
            direct["error"]["code"],
            json!(codes::TOOL_BLOCKED),
            "{direct}"
        );
        assert_eq!(via_executor, direct, "{via_executor}");
        let message = via_executor["error"]["message"].as_str().unwrap_or("");
        assert!(
            message.contains(&tool) && !message.contains("nasiko_call_tool"),
            "the denial must name the inner tool, never the wrapper: {via_executor}"
        );
    }

    #[tokio::test]
    async fn nasiko_call_tool_ask_decision_is_the_inner_tools_ask_decision() {
        // No flow, so nothing is persisted (hermetic, same as
        // `generic_mcp_ask_without_traceparent_returns_tool_ask_without_hitl_id`)
        // — but the TOOL_ASK the executor answers must be the inner tool's own:
        // same code, same connector label, the inner name in the message.
        let mut state = test_state();
        state.authorizer = std::sync::Arc::new(AllowAllAuthorizer);
        let cid = Uuid::new_v4();
        let resolved = mcp_session("http://127.0.0.1:9/mcp", cid, false);
        let p = perms(&[cid], vec![rule(cid, "*", Stance::Ask)]);
        let tool = format!("{}__echo", crate::types::connector_prefix(cid));

        let (direct, via_executor) =
            direct_and_via_executor(&state, &resolved, &p, &tool, json!({})).await;

        assert_eq!(direct["error"]["code"], json!(codes::TOOL_ASK), "{direct}");
        assert_eq!(via_executor, direct, "{via_executor}");
        assert_eq!(
            via_executor["error"]["data"]["server"],
            json!("uploaded-server"),
            "{via_executor}"
        );
        // The ask names the un-namespaced inner tool, exactly as the direct
        // path does (`ask_with_hitl_request` takes `original`).
        let message = via_executor["error"]["message"].as_str().unwrap_or("");
        assert!(
            message.contains("'echo'") && !message.contains("nasiko_call_tool"),
            "the ask must name the inner tool, never the wrapper: {via_executor}"
        );
    }

    // The adapter itself — the one parse both the executor's dispatch and the
    // route layer's `invoked_tool_name` go through.

    #[test]
    fn invoked_tool_name_is_the_inner_tool_only_for_an_executor_call_that_dispatches() {
        assert_eq!(
            invoked_tool_name(&json!({ "name": "abcd__echo", "arguments": {} })),
            "abcd__echo"
        );
        assert_eq!(
            invoked_tool_name(&json!({
                "name": "nasiko_call_tool",
                "arguments": { "name": "abcd__echo", "arguments": { "path": "x" } },
            })),
            "abcd__echo"
        );
        // What the parse rejects was never attempted as an inner tool, so the
        // record must say the executor itself failed — not blame a tool the
        // client never validly named.
        for rejected in [
            json!({}),
            json!({ "name": "nasiko_search_tools" }),
            json!({ "name": "abcd__echo", "arguments": "not-an-object" }),
        ] {
            assert_eq!(
                invoked_tool_name(&json!({ "name": "nasiko_call_tool", "arguments": rejected })),
                "nasiko_call_tool"
            );
        }
        assert_eq!(invoked_tool_name(&json!({})), "");
    }

    #[test]
    fn parse_call_tool_target_defaults_omitted_arguments_to_an_empty_object() {
        let omitted = json!({ "name": "abcd__echo" });
        let (name, arguments) = parse_call_tool_target(Some(&omitted)).expect("valid target");
        assert_eq!(name, "abcd__echo");
        assert_eq!(arguments, json!({}));

        let null = json!({ "name": "abcd__echo", "arguments": null });
        let (_, arguments) =
            parse_call_tool_target(Some(&null)).expect("null arguments mean omitted");
        assert_eq!(arguments, json!({}));

        assert!(parse_call_tool_target(None).is_err());
    }

    #[test]
    fn nasiko_call_tool_definition_requires_only_the_inner_name() {
        let def = nasiko_call_tool_definition();
        assert_eq!(def["name"], "nasiko_call_tool");
        assert_eq!(def["inputSchema"]["required"], json!(["name"]));
        assert_eq!(def["inputSchema"]["properties"]["name"]["type"], "string");
        assert_eq!(
            def["inputSchema"]["properties"]["arguments"]["type"],
            "object"
        );
        assert_eq!(
            def["inputSchema"]["properties"]["arguments"]["default"],
            json!({})
        );
    }
}

#[cfg(test)]
mod initialize_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn initialize_echoes_a_supported_client_version() {
        let req = json!({"jsonrpc":"2.0","id":1,"method":"initialize",
            "params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"0"}}});
        let out = handle_initialize(
            &json!(1),
            &req,
            "GATEWAY-INSTR",
            &["CONNECTOR-INSTR".to_string()],
        );
        assert_eq!(out["result"]["protocolVersion"], "2025-06-18");
        assert!(
            out["result"]["instructions"]
                .as_str()
                .unwrap()
                .contains("GATEWAY-INSTR")
        );
        assert!(
            out["result"]["instructions"]
                .as_str()
                .unwrap()
                .contains("CONNECTOR-INSTR")
        );
    }

    /// Backward-compat guarantee: every version this gateway claims to support
    /// — including older ones like 2024-11-05 — is echoed back verbatim, not
    /// silently upgraded to the latest.
    #[test]
    fn initialize_echoes_every_supported_version_verbatim() {
        for &v in SUPPORTED_PROTOCOL_VERSIONS {
            let req = json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":v}});
            let out = handle_initialize(&json!(1), &req, "x", &[]);
            assert_eq!(
                out["result"]["protocolVersion"], v,
                "version {v} should be echoed back unchanged"
            );
        }
    }

    #[test]
    fn initialize_falls_back_to_latest_supported_for_unknown_version() {
        let req = json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2031-01-01"}});
        let out = handle_initialize(&json!(1), &req, "x", &[]);
        assert_eq!(out["result"]["protocolVersion"], LATEST_PROTOCOL_VERSION);
    }

    #[test]
    fn initialize_falls_back_to_latest_supported_for_missing_params() {
        let out = handle_initialize(&json!(1), &json!({"method":"initialize"}), "x", &[]);
        assert_eq!(out["result"]["protocolVersion"], LATEST_PROTOCOL_VERSION);
    }

    /// A non-string `protocolVersion` (a client sending a bare number instead
    /// of a string) must not panic or coerce — `Value::as_str` returns `None`
    /// for it, same fallback path as an unknown or missing version.
    #[test]
    fn initialize_falls_back_to_latest_supported_for_non_string_version() {
        let req =
            json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":2025}});
        let out = handle_initialize(&json!(1), &req, "x", &[]);
        assert_eq!(out["result"]["protocolVersion"], LATEST_PROTOCOL_VERSION);
    }

    #[test]
    fn initialize_omits_instructions_when_empty() {
        let out = handle_initialize(&json!(1), &json!({"method":"initialize"}), "", &[]);
        assert!(out["result"].get("instructions").is_none());
    }

    /// Exact composition: gateway instructions first, then connector
    /// instructions in order, joined by a blank line, each trimmed, and any
    /// whitespace-only entry dropped rather than contributing a stray blank.
    #[test]
    fn initialize_composes_instructions_in_order_trimmed_and_joined() {
        let out = handle_initialize(
            &json!(1),
            &json!({"method":"initialize"}),
            "  G  ",
            &["  ".to_string(), " C ".to_string()],
        );
        assert_eq!(out["result"]["instructions"], json!("G\n\nC"));
    }

    // ─── connector_instructions() — the collection step `handle_request` feeds
    // into `handle_initialize`. `resolve_session`/`load_permission_context`
    // need a real DB this crate's hermetic tests can't provide, so the
    // composition is tested here as its own pure function instead of through
    // `handle_request` end-to-end.

    fn cfg(connector_id: Uuid, instructions: Option<&str>) -> MCPServerConfig {
        MCPServerConfig {
            connector_id,
            kind: ServerType::Mcp,
            name: "test".into(),
            url: "http://127.0.0.1:1/mcp".into(),
            headers: std::collections::HashMap::new(),
            transport: "streamable_http".into(),
            trusted: false,
            system: false,
            tool_names: vec![],
            instructions: instructions.map(str::to_string),
        }
    }

    fn ctx(enabled: &[Uuid]) -> PermissionContext {
        PermissionContext {
            agent_id: Uuid::nil(),
            enabled_connectors: enabled.iter().copied().collect(),
            rules: vec![],
            hash: "h".into(),
        }
    }

    #[test]
    fn connector_instructions_collects_only_enabled_connectors_with_instructions() {
        let enabled_with_instr = Uuid::new_v4();
        let enabled_without_instr = Uuid::new_v4();
        let disabled_with_instr = Uuid::new_v4();
        let servers = vec![
            cfg(enabled_with_instr, Some("WS-INSTR")),
            cfg(enabled_without_instr, None),
            cfg(disabled_with_instr, Some("SHOULD-NOT-APPEAR")),
        ];
        let perms = ctx(&[enabled_with_instr, enabled_without_instr]);
        let got = connector_instructions(&servers, &perms);
        assert_eq!(got, vec!["WS-INSTR".to_string()]);
    }

    #[test]
    fn connector_instructions_is_empty_when_nothing_qualifies() {
        let id = Uuid::new_v4();
        let servers = vec![cfg(id, Some("X"))];
        let perms = ctx(&[]); // never enabled
        assert!(connector_instructions(&servers, &perms).is_empty());
    }

    // ── inject_identity: stamps only system backends ─────────────────────────

    fn system_cfg(connector_id: Uuid) -> MCPServerConfig {
        let mut s = cfg(connector_id, None);
        s.system = true;
        s.trusted = true;
        s
    }

    #[test]
    fn inject_identity_stamps_only_system_servers_with_the_given_header() {
        let key = b"test-identity-key".to_vec();
        let agent_id = Uuid::new_v4();
        let user_id = Uuid::new_v4();
        let signed =
            crate::identity::SignedIdentity::new(agent_id, user_id, Some("flow-abc".to_string()))
                .sign(&key);
        let mut servers = vec![system_cfg(Uuid::new_v4()), cfg(Uuid::new_v4(), None)];

        inject_identity(&mut servers, &signed);

        let system_server = servers.iter().find(|s| s.system).expect("system server");
        let header = system_server
            .headers
            .get(crate::identity::IDENTITY_HEADER)
            .expect("system server must carry the identity header");
        let verified = crate::identity::SignedIdentity::verify(header, &key)
            .expect("the gateway's own signature must verify with the same key");
        assert_eq!(verified.agent_id, agent_id);
        assert_eq!(verified.user_id, user_id);
        assert_eq!(verified.flow_id.as_deref(), Some("flow-abc"));

        let non_system = servers
            .iter()
            .find(|s| !s.system)
            .expect("non-system server");
        assert!(
            !non_system
                .headers
                .contains_key(crate::identity::IDENTITY_HEADER),
            "a non-system backend must never receive the identity header: {non_system:?}"
        );
    }

    #[test]
    fn inject_identity_is_a_no_op_when_there_is_no_system_backend() {
        let key = b"test-identity-key".to_vec();
        let signed =
            crate::identity::SignedIdentity::new(Uuid::new_v4(), Uuid::new_v4(), None).sign(&key);
        let mut servers = vec![cfg(Uuid::new_v4(), None), cfg(Uuid::new_v4(), None)];

        inject_identity(&mut servers, &signed);

        assert!(
            servers
                .iter()
                .all(|s| !s.headers.contains_key(crate::identity::IDENTITY_HEADER)),
            "no backend is system, so nothing should be stamped: {servers:?}"
        );
    }
}

//! `POST /api/mcp` auth — deploy-time agent credential + flow-bound user
//! identity (docs/MCP_GATEWAY_AGENT_AUTH.md §2.4).
//!
//! This route is deliberately NOT behind `require_auth`: a deployed agent
//! never holds the calling user's real session JWT (`agent_proxy.rs` strips
//! `Authorization`/`Cookie` before forwarding to a container on purpose). Its
//! only credential is its own `MCP_GATEWAY_TOKEN` (`Authorization: Bearer`),
//! and the user half is resolved server-side from the request's `traceparent`
//! via the `flows` row + `flow_participants` check. These tests lock in every
//! rule of the authorization ladder, each failing closed.

mod common;

use common::TestServer;
use serial_test::serial;
use uuid::Uuid;

async fn seed_user(server: &TestServer, name: &str) -> Uuid {
    sqlx::query_scalar("INSERT INTO users (username, email) VALUES ($1, $2) RETURNING id")
        .bind(name)
        .bind(format!("{name}@test.local"))
        .fetch_one(&server.db)
        .await
        .expect("seed user")
}

async fn seed_agent(server: &TestServer, owner_id: Uuid, name: &str) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO agents (name, owner_id, image, status) \
         VALUES ($1, $2, 'nasiko/echo:1.0.0', 'running') RETURNING id",
    )
    .bind(name)
    .bind(owner_id)
    .fetch_one(&server.db)
    .await
    .expect("seed agent")
}

fn rpc(method: &str) -> serde_json::Value {
    serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": method,
        "params": {"name": "some__tool", "arguments": {}}})
}

async fn post_mcp(
    server: &TestServer,
    bearer: Option<&str>,
    traceparent: Option<&str>,
    body: &serde_json::Value,
) -> reqwest::Response {
    let mut req = server.client.post(server.url("/api/mcp")).json(body);
    if let Some(t) = bearer {
        req = req.bearer_auth(t);
    }
    if let Some(tp) = traceparent {
        req = req.header("traceparent", tp);
    }
    req.send().await.unwrap()
}

// ─── Rule 1: agent authentication (401s) ─────────────────────────────────────

#[tokio::test]
#[serial]
async fn valid_gateway_token_is_accepted_for_initialize() {
    let server = TestServer::start().await;
    let owner = seed_user(&server, "gw-owner-1").await;
    let agent = seed_agent(&server, owner, "gw-agent-1").await;
    let token = common::mint_gateway_token(&server.db, agent).await;

    let res = post_mcp(&server, Some(&token), None, &rpc("initialize")).await;
    assert_eq!(res.status(), 200);
    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["result"]["serverInfo"]["name"], "MCP Gateway");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn missing_bearer_is_401() {
    let server = TestServer::start().await;
    let res = post_mcp(&server, None, None, &rpc("initialize")).await;
    assert_eq!(res.status(), 401);
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn unknown_or_garbage_tokens_are_401_not_500() {
    let server = TestServer::start().await;
    for bad in ["", "not-a-token", "ngt_deadbeef", &"A".repeat(5000)] {
        let res = post_mcp(&server, Some(bad), None, &rpc("initialize")).await;
        assert_eq!(res.status(), 401, "{bad:?} must be a clean 401");
    }
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn revoked_token_is_401() {
    let server = TestServer::start().await;
    let owner = seed_user(&server, "gw-owner-rev").await;
    let agent = seed_agent(&server, owner, "gw-agent-rev").await;
    let token = common::mint_gateway_token(&server.db, agent).await;
    nasiko_mcp_gateway::agent_tokens::revoke(&server.db, agent)
        .await
        .unwrap();

    let res = post_mcp(&server, Some(&token), None, &rpc("initialize")).await;
    assert_eq!(res.status(), 401, "a tombstoned credential must not work");
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn rotation_keeps_the_previous_token_alive_for_the_grace_window() {
    // `mint` runs before the new workload is known to be live. Until the rollout
    // lands, the *old* container is still serving with the *old* plaintext, so
    // rejecting it immediately turns a slow or failed deploy into a wave of 401s
    // from a healthy agent (ROTATION_GRACE_SECS).
    let server = TestServer::start().await;
    let owner = seed_user(&server, "gw-owner-rot").await;
    let agent = seed_agent(&server, owner, "gw-agent-rot").await;
    let old = common::mint_gateway_token(&server.db, agent).await;
    let new = common::mint_gateway_token(&server.db, agent).await;

    let res = post_mcp(&server, Some(&new), None, &rpc("initialize")).await;
    assert_eq!(res.status(), 200, "the freshly minted credential must work");
    let res = post_mcp(&server, Some(&old), None, &rpc("initialize")).await;
    assert_eq!(
        res.status(),
        200,
        "the superseded credential must survive the rollout window"
    );
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn rotation_grace_expires() {
    // Bounded, not indefinite — otherwise redeploy would never actually revoke
    // anything. Age the rotation past the window rather than sleeping through it.
    let server = TestServer::start().await;
    let owner = seed_user(&server, "gw-owner-rot-exp").await;
    let agent = seed_agent(&server, owner, "gw-agent-rot-exp").await;
    let old = common::mint_gateway_token(&server.db, agent).await;
    let new = common::mint_gateway_token(&server.db, agent).await;

    sqlx::query(
        "UPDATE agent_gateway_tokens
         SET rotated_at = now() - make_interval(secs => $2)
         WHERE agent_id = $1",
    )
    .bind(agent)
    .bind((nasiko_mcp_gateway::agent_tokens::ROTATION_GRACE_SECS + 60) as f64)
    .execute(&server.db)
    .await
    .expect("age the rotation");

    let res = post_mcp(&server, Some(&old), None, &rpc("initialize")).await;
    assert_eq!(res.status(), 401, "grace must expire");
    let res = post_mcp(&server, Some(&new), None, &rpc("initialize")).await;
    assert_eq!(res.status(), 200, "the current credential is unaffected");
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn revoke_kills_the_grace_credential_too() {
    // Destroy must not leave a second, still-accepted hash behind.
    let server = TestServer::start().await;
    let owner = seed_user(&server, "gw-owner-rot-rev").await;
    let agent = seed_agent(&server, owner, "gw-agent-rot-rev").await;
    let old = common::mint_gateway_token(&server.db, agent).await;
    let new = common::mint_gateway_token(&server.db, agent).await;
    nasiko_mcp_gateway::agent_tokens::revoke(&server.db, agent)
        .await
        .unwrap();

    for (label, token) in [("superseded", &old), ("current", &new)] {
        let res = post_mcp(&server, Some(token), None, &rpc("initialize")).await;
        assert_eq!(res.status(), 401, "{label} credential must be dead");
    }
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn re_minting_for_a_revoked_agent_does_not_resurrect_the_old_credential() {
    // A revoked hash must never be carried into prev_token_hash: destroy
    // tombstoned it, and a later re-mint is a new life, not a continuation.
    let server = TestServer::start().await;
    let owner = seed_user(&server, "gw-owner-rot-res").await;
    let agent = seed_agent(&server, owner, "gw-agent-rot-res").await;
    let destroyed = common::mint_gateway_token(&server.db, agent).await;
    nasiko_mcp_gateway::agent_tokens::revoke(&server.db, agent)
        .await
        .unwrap();
    let reborn = common::mint_gateway_token(&server.db, agent).await;

    let res = post_mcp(&server, Some(&destroyed), None, &rpc("initialize")).await;
    assert_eq!(res.status(), 401, "a tombstoned credential stays dead");
    let res = post_mcp(&server, Some(&reborn), None, &rpc("initialize")).await;
    assert_eq!(res.status(), 200);
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn soft_deleting_the_agent_kills_its_gateway_token() {
    // `revoke` is best-effort on the destroy path; if it fails, the soft-delete
    // must still close the door. Otherwise a destroyed agent keeps calling tools
    // for as long as any flow it joined stays live.
    let server = TestServer::start().await;
    let owner = seed_user(&server, "gw-owner-del").await;
    let agent = seed_agent(&server, owner, "gw-agent-del").await;
    let token = common::mint_gateway_token(&server.db, agent).await;

    let res = post_mcp(&server, Some(&token), None, &rpc("initialize")).await;
    assert_eq!(res.status(), 200, "sanity: live agent authenticates");

    // Soft-delete only — deliberately without calling revoke().
    sqlx::query("UPDATE agents SET deleted_at = now() WHERE id = $1")
        .bind(agent)
        .execute(&server.db)
        .await
        .expect("soft delete");

    let res = post_mcp(&server, Some(&token), None, &rpc("initialize")).await;
    assert_eq!(
        res.status(),
        401,
        "deletion must fail closed even when tombstoning did not run"
    );
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_user_session_jwt_is_not_a_gateway_credential() {
    // A real, valid session JWT via Authorization must be rejected: /api/mcp
    // must never silently start trusting user sessions (an agent can never
    // hold one — the proxy strips them by design).
    let server = TestServer::start().await;
    let user_token = common::sign_token(&Uuid::new_v4().to_string(), "someuser", false, "member");
    let res = post_mcp(&server, Some(&user_token), None, &rpc("initialize")).await;
    assert_eq!(res.status(), 401);
    server.cleanup().await;
}

// ─── Rule 2: tools/list works agent-only (no flow required) ──────────────────

#[tokio::test]
#[serial]
async fn tools_list_works_without_a_flow() {
    // Startup-time tool discovery happens outside any flow: agent-only
    // identity (the agent's own connector grants) must be enough for the
    // read-only listing.
    let server = TestServer::start().await;
    let owner = seed_user(&server, "gw-owner-list").await;
    let agent = seed_agent(&server, owner, "gw-agent-list").await;
    let token = common::mint_gateway_token(&server.db, agent).await;

    let res = post_mcp(&server, Some(&token), None, &rpc("tools/list")).await;
    assert_eq!(res.status(), 200);
    let body: serde_json::Value = res.json().await.unwrap();
    assert!(
        body.get("result").is_some() || body.get("error").is_some(),
        "must be a JSON-RPC response, got: {body}"
    );
    server.cleanup().await;
}

// ─── Rules 3+4: tools/call requires a live flow the agent participates in ────

#[tokio::test]
#[serial]
async fn tools_call_without_traceparent_is_403() {
    let server = TestServer::start().await;
    let owner = seed_user(&server, "gw-owner-notp").await;
    let agent = seed_agent(&server, owner, "gw-agent-notp").await;
    let token = common::mint_gateway_token(&server.db, agent).await;

    let res = post_mcp(&server, Some(&token), None, &rpc("tools/call")).await;
    assert_eq!(res.status(), 403);
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn tools_call_with_unknown_trace_id_is_403() {
    // Presence is not the check — resolution is. A well-formed traceparent
    // naming no flow row must be rejected identically; there is nothing an
    // agent can fabricate to pass.
    let server = TestServer::start().await;
    let owner = seed_user(&server, "gw-owner-unk").await;
    let agent = seed_agent(&server, owner, "gw-agent-unk").await;
    let token = common::mint_gateway_token(&server.db, agent).await;

    let fabricated = format!("00-{}-00f067aa0ba902b7-01", Uuid::new_v4().simple());
    let res = post_mcp(&server, Some(&token), Some(&fabricated), &rpc("tools/call")).await;
    assert_eq!(res.status(), 403);
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn tools_call_from_a_non_participant_agent_is_403() {
    // The attack the participant check exists to stop (§3.3): agent B holds a
    // valid credential of its own and replays a traceparent it observed from
    // agent A's flow — it must not be able to act as that flow's user.
    let server = TestServer::start().await;
    let user = seed_user(&server, "gw-user-np").await;
    let owner = seed_user(&server, "gw-owner-np").await;
    let agent_a = seed_agent(&server, owner, "gw-agent-np-a").await;
    let agent_b = seed_agent(&server, owner, "gw-agent-np-b").await;
    let (_, traceparent) = common::open_flow(&server.db, user, agent_a).await;
    let token_b = common::mint_gateway_token(&server.db, agent_b).await;

    let res = post_mcp(
        &server,
        Some(&token_b),
        Some(&traceparent),
        &rpc("tools/call"),
    )
    .await;
    assert_eq!(res.status(), 403);
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn tools_call_on_a_completed_flow_is_403() {
    // Liveness comes from the flow row itself: once complete_flow closes it,
    // the trace id stops authorizing calls (instant revocation).
    let server = TestServer::start().await;
    let user = seed_user(&server, "gw-user-done").await;
    let owner = seed_user(&server, "gw-owner-done").await;
    let agent = seed_agent(&server, owner, "gw-agent-done").await;
    let (flow_id, traceparent) = common::open_flow(&server.db, user, agent).await;
    let token = common::mint_gateway_token(&server.db, agent).await;

    sqlx::query("UPDATE flows SET status = 'completed', completed_at = now() WHERE flow_id = $1")
        .bind(&flow_id)
        .execute(&server.db)
        .await
        .unwrap();

    let res = post_mcp(
        &server,
        Some(&token),
        Some(&traceparent),
        &rpc("tools/call"),
    )
    .await;
    assert_eq!(res.status(), 403);
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn tools_call_inside_a_live_flow_passes_the_auth_gate() {
    // The full happy path through the gate: agent credential + live flow +
    // participant record. With no connectors configured the protocol layer
    // answers with a JSON-RPC error object — but over HTTP 200: authorization
    // succeeded and the request reached protocol handling.
    let server = TestServer::start().await;
    let user = seed_user(&server, "gw-user-ok").await;
    let owner = seed_user(&server, "gw-owner-ok").await;
    let agent = seed_agent(&server, owner, "gw-agent-ok").await;
    let (_, traceparent) = common::open_flow(&server.db, user, agent).await;
    let token = common::mint_gateway_token(&server.db, agent).await;

    let res = post_mcp(
        &server,
        Some(&token),
        Some(&traceparent),
        &rpc("tools/call"),
    )
    .await;
    assert_eq!(res.status(), 200);
    let body: serde_json::Value = res.json().await.unwrap();
    assert!(
        body.get("result").is_some() || body.get("error").is_some(),
        "must be a JSON-RPC response, got: {body}"
    );
    server.cleanup().await;
}

// ─── Coding-agent owner policy ────────────────────────────────────────────────
//
// A local coding agent (Claude Code / Codex / OpenCode, connected by the CLI)
// is never dispatched through the proxy, so it never has a flow — `flow_user`
// always fails for it. Its row is stamped `coding_agent_integration_id`, the
// same predicate the LLM router uses (`oss/llm-router/src/resolver/mod.rs`)
// to bill such calls to the owner with no flow; the gateway must resolve the
// same rows to their owner so the two can never disagree. Every other
// flow-less `tools/call` (a plain deployed agent) stays 403 — the policy is
// narrow, keyed only on the column the platform itself sets.

#[tokio::test]
#[serial]
async fn coding_agent_row_without_flow_resolves_to_owner_for_tools_call() {
    let server = TestServer::start().await;
    let owner = seed_user(&server, "gw-owner-coding").await;
    let agent_id = seed_agent(&server, owner, "gw-agent-coding").await;
    sqlx::query("UPDATE agents SET coding_agent_integration_id = 'claude' WHERE id = $1")
        .bind(agent_id)
        .execute(&server.db)
        .await
        .unwrap();
    let token = common::mint_gateway_token(&server.db, agent_id).await;

    // No traceparent, tools/call: a coding-agent row is admitted to the
    // protocol layer, which answers the bogus tool name with a JSON-RPC
    // error — but over HTTP 200, same "reached protocol handling" signal as
    // `tools_call_inside_a_live_flow_passes_the_auth_gate` above.
    let res = post_mcp(&server, Some(&token), None, &rpc("tools/call")).await;
    assert_eq!(res.status(), 200);
    let body: serde_json::Value = res.json().await.unwrap();
    assert!(
        body.get("error").is_some(),
        "unknown tool must be a JSON-RPC error, got {body}"
    );

    // A plain deployed agent (no coding_agent_integration_id) is still refused.
    let other = seed_agent(&server, owner, "gw-agent-deployed").await;
    let other_token = common::mint_gateway_token(&server.db, other).await;
    let res = post_mcp(&server, Some(&other_token), None, &rpc("tools/call")).await;
    assert_eq!(res.status(), 403);

    server.cleanup().await;
}

/// A minimal MCP backend for the executor test below: answers `tools/call`
/// with a fixed result and records each tool name it was asked to run. The
/// gateway's generic transport sends `tools/call` directly (no `initialize`
/// handshake), so nothing else needs answering.
async fn start_call_log_backend() -> (String, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
    use axum::{Json, Router, extract::State, routing::post};
    type CallLog = std::sync::Arc<std::sync::Mutex<Vec<String>>>;

    async fn handle(
        State(calls): State<CallLog>,
        Json(body): Json<serde_json::Value>,
    ) -> Json<serde_json::Value> {
        let id = body.get("id").cloned().unwrap_or(serde_json::Value::Null);
        let name = body["params"]["name"].as_str().unwrap_or("").to_string();
        calls.lock().unwrap().push(name.clone());
        Json(serde_json::json!({
            "jsonrpc": "2.0", "id": id,
            "result": {"content": [{"type": "text", "text": format!("ran '{name}'")}]},
        }))
    }

    let calls: CallLog = Default::default();
    let app = Router::new()
        .route("/mcp", post(handle))
        .with_state(calls.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://127.0.0.1:{port}/mcp"), calls)
}

#[tokio::test]
#[serial]
async fn coding_agent_row_executes_via_nasiko_call_tool_as_via_the_direct_name() {
    // Rule 3b meets the fixed-menu executor: a local coding agent is exactly
    // the kind of client that can only invoke listed tools, and it never has
    // a flow. Through the owner policy, `nasiko_call_tool` must reach the
    // backend precisely as the direct name does — and must not widen rule 3
    // for a plain deployed row.
    let server = TestServer::start().await;
    let owner = seed_user(&server, "gw-owner-call-tool").await;
    let agent_id = seed_agent(&server, owner, "gw-agent-call-tool").await;
    sqlx::query("UPDATE agents SET coding_agent_integration_id = 'claude' WHERE id = $1")
        .bind(agent_id)
        .execute(&server.db)
        .await
        .unwrap();
    let token = common::mint_gateway_token(&server.db, agent_id).await;

    // A system connector owning `save_file` — the only bare-name routing a
    // generic backend gets (`router::route_tool`'s synced-catalog match).
    let (backend_url, backend_calls) = start_call_log_backend().await;
    let connector_id: Uuid = sqlx::query_scalar(
        "INSERT INTO mcp_connectors (provider_type, source_kind, name, url, auth_type) \
         VALUES ('system', 'system', 'gw-call-tool-connector', $1, 'none') RETURNING id",
    )
    .bind(&backend_url)
    .fetch_one(&server.db)
    .await
    .expect("insert system connector");
    sqlx::query(
        "INSERT INTO mcp_connector_grants (connector_id, grant_type, grantee_id) \
         VALUES ($1, 'public', '*')",
    )
    .bind(connector_id)
    .execute(&server.db)
    .await
    .expect("insert public grant");
    sqlx::query(
        "INSERT INTO mcp_connector_tools (connector_id, tool_name) VALUES ($1, 'save_file')",
    )
    .bind(connector_id)
    .execute(&server.db)
    .await
    .expect("insert synced connector tool");
    sqlx::query(
        "INSERT INTO mcp_agent_connector_access (agent_id, connector_id, enabled) \
         VALUES ($1, $2, true)",
    )
    .bind(agent_id)
    .bind(connector_id)
    .execute(&server.db)
    .await
    .expect("insert agent connector access");

    let direct = serde_json::json!({"jsonrpc": "2.0", "id": 7, "method": "tools/call",
        "params": {"name": "save_file", "arguments": {"path": "a.md"}}});
    let via_executor = serde_json::json!({"jsonrpc": "2.0", "id": 7, "method": "tools/call",
        "params": {"name": "nasiko_call_tool",
                   "arguments": {"name": "save_file", "arguments": {"path": "a.md"}}}});

    // No traceparent either time — the owner policy is the whole admission.
    let res = post_mcp(&server, Some(&token), None, &direct).await;
    assert_eq!(res.status(), 200);
    let direct_body: serde_json::Value = res.json().await.unwrap();
    assert!(
        direct_body.get("error").is_none(),
        "direct save_file call must succeed: {direct_body}"
    );

    let res = post_mcp(&server, Some(&token), None, &via_executor).await;
    assert_eq!(
        res.status(),
        200,
        "the executor must pass the owner policy exactly as the direct name does"
    );
    let executor_body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(
        executor_body, direct_body,
        "the executor's response must be byte-identical to the direct call's"
    );
    assert_eq!(
        backend_calls.lock().unwrap().as_slice(),
        &["save_file".to_string(), "save_file".to_string()],
        "the backend must have seen save_file twice — never nasiko_call_tool"
    );

    // A plain deployed row stays refused without a flow (rule 3) — the
    // executor is not a second door.
    let deployed = seed_agent(&server, owner, "gw-agent-call-tool-deployed").await;
    let deployed_token = common::mint_gateway_token(&server.db, deployed).await;
    let res = post_mcp(&server, Some(&deployed_token), None, &via_executor).await;
    assert_eq!(res.status(), 403);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn soft_deleted_coding_agent_row_is_not_admitted() {
    // A soft-deleted coding-agent row must not be waved through by the owner
    // policy. In practice this is caught one layer up: `agent_tokens::authenticate`
    // (rule 1) joins on `agents.deleted_at IS NULL`, so a deleted agent's
    // gateway token stops authenticating at all — the request never reaches
    // `dispatch`'s coding-agent-owner branch to begin with. `coding_agent_owner`
    // itself repeats the same `deleted_at IS NULL` filter (see its doc comment
    // in `gateway.rs`) as a second, independent safeguard, exactly like
    // `agent_owner`'s pre-existing one — this test locks in the outcome that
    // safeguard exists for, at the only layer an external test can observe it.
    let server = TestServer::start().await;
    let owner = seed_user(&server, "gw-owner-coding-del").await;
    let agent_id = seed_agent(&server, owner, "gw-agent-coding-del").await;
    sqlx::query("UPDATE agents SET coding_agent_integration_id = 'claude' WHERE id = $1")
        .bind(agent_id)
        .execute(&server.db)
        .await
        .unwrap();
    let token = common::mint_gateway_token(&server.db, agent_id).await;

    let res = post_mcp(&server, Some(&token), None, &rpc("tools/call")).await;
    assert_eq!(
        res.status(),
        200,
        "sanity: live coding-agent row is admitted"
    );

    sqlx::query("UPDATE agents SET deleted_at = now() WHERE id = $1")
        .bind(agent_id)
        .execute(&server.db)
        .await
        .expect("soft delete");

    let res = post_mcp(&server, Some(&token), None, &rpc("tools/call")).await;
    assert_eq!(
        res.status(),
        401,
        "a soft-deleted coding-agent row's token must stop authenticating, \
         not fall through to the owner policy"
    );

    server.cleanup().await;
}

// ─── URL-credential form: POST /api/mcp/s/{token} ────────────────────────────
//
// Same credential, same ladder — only the transport differs. These exist to
// prove the URL form is not a weaker door: whatever the header form rejects,
// this must reject identically. The reason it exists at all is that MCP has no
// `OPENAI_API_KEY`-style env convention, so framework clients that expose only
// a `url` have nowhere to put a bearer header.

async fn post_mcp_url(
    server: &TestServer,
    token: &str,
    traceparent: Option<&str>,
    body: &serde_json::Value,
) -> reqwest::Response {
    let mut req = server
        .client
        .post(server.url(&format!("/api/mcp/s/{token}")))
        .json(body);
    if let Some(tp) = traceparent {
        req = req.header("traceparent", tp);
    }
    req.send().await.unwrap()
}

#[tokio::test]
#[serial]
async fn url_credential_is_accepted_for_initialize() {
    let server = TestServer::start().await;
    let owner = seed_user(&server, "gwu-owner-1").await;
    let agent = seed_agent(&server, owner, "gwu-agent-1").await;
    let token = common::mint_gateway_token(&server.db, agent).await;

    let res = post_mcp_url(&server, &token, None, &rpc("initialize")).await;
    assert_eq!(res.status(), 200);
    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["result"]["serverInfo"]["name"], "MCP Gateway");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn url_credential_unknown_token_is_401() {
    let server = TestServer::start().await;
    let res = post_mcp_url(&server, "ngt_not_a_real_token", None, &rpc("initialize")).await;
    assert_eq!(res.status(), 401);
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn url_credential_honours_revocation() {
    // Revocation must reach both forms — they read one row.
    let server = TestServer::start().await;
    let owner = seed_user(&server, "gwu-owner-rev").await;
    let agent = seed_agent(&server, owner, "gwu-agent-rev").await;
    let token = common::mint_gateway_token(&server.db, agent).await;

    sqlx::query("UPDATE agent_gateway_tokens SET revoked_at = now() WHERE agent_id = $1")
        .bind(agent)
        .execute(&server.db)
        .await
        .expect("revoke");

    let res = post_mcp_url(&server, &token, None, &rpc("initialize")).await;
    assert_eq!(res.status(), 401);
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_leaked_url_alone_cannot_call_a_tool() {
    // The property that makes carrying the credential in a URL an acceptable
    // trade — for a deployed agent: the credential proves only *which agent*,
    // and without a traceparent naming a live flow the agent participates in,
    // `tools/call` is still 403, so a URL scraped from a log or a trace cannot
    // invoke anything. This does NOT extend to a CLI-bound coding-agent row
    // (`coding_agent_row_without_flow_resolves_to_owner_for_tools_call` above
    // covers that case, deliberately, on its own).
    let server = TestServer::start().await;
    let owner = seed_user(&server, "gwu-owner-leak").await;
    let agent = seed_agent(&server, owner, "gwu-agent-leak").await;
    let token = common::mint_gateway_token(&server.db, agent).await;

    let res = post_mcp_url(&server, &token, None, &rpc("tools/call")).await;
    assert_eq!(res.status(), 403);
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn url_credential_enforces_flow_participation() {
    // Replay of another agent's traceparent is rejected here exactly as it is
    // on the header form.
    let server = TestServer::start().await;
    let user = seed_user(&server, "gwu-user-np").await;
    let owner = seed_user(&server, "gwu-owner-np").await;
    let agent_a = seed_agent(&server, owner, "gwu-agent-np-a").await;
    let agent_b = seed_agent(&server, owner, "gwu-agent-np-b").await;
    let (_, traceparent) = common::open_flow(&server.db, user, agent_a).await;
    let token_b = common::mint_gateway_token(&server.db, agent_b).await;

    let res = post_mcp_url(&server, &token_b, Some(&traceparent), &rpc("tools/call")).await;
    assert_eq!(res.status(), 403);
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn url_credential_allows_tools_call_inside_its_own_flow() {
    let server = TestServer::start().await;
    let user = seed_user(&server, "gwu-user-ok").await;
    let owner = seed_user(&server, "gwu-owner-ok").await;
    let agent = seed_agent(&server, owner, "gwu-agent-ok").await;
    let (_, traceparent) = common::open_flow(&server.db, user, agent).await;
    let token = common::mint_gateway_token(&server.db, agent).await;

    let res = post_mcp_url(&server, &token, Some(&traceparent), &rpc("tools/call")).await;
    assert_eq!(res.status(), 200);
    let body: serde_json::Value = res.json().await.unwrap();
    assert!(
        body.get("result").is_some() || body.get("error").is_some(),
        "must be a JSON-RPC response, got: {body}"
    );
    server.cleanup().await;
}

// ─── `MCP-Protocol-Version` header validation ─────────────────────────────────
//
// The gateway negotiates `protocolVersion` in `initialize` and advertises 2025-06-18. Per the MCP
// spec, a client that negotiated 2025-06-18 sends `MCP-Protocol-Version` on every subsequent
// request; the gateway must reject a version it doesn't implement, while still treating the
// header as optional (old clients that never negotiated never send it).

#[tokio::test]
#[serial]
async fn unknown_protocol_version_header_is_rejected_with_400() {
    let server = TestServer::start().await;
    let owner = seed_user(&server, "gw-owner-pv").await;
    let agent_id = seed_agent(&server, owner, "pv-agent").await;
    let token = common::mint_gateway_token(&server.db, agent_id).await;

    let resp = server
        .client
        .post(server.url("/api/mcp"))
        .bearer_auth(&token)
        .header("MCP-Protocol-Version", "1999-01-01")
        .json(&rpc("tools/list"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);

    let resp = server
        .client
        .post(server.url("/api/mcp"))
        .bearer_auth(&token)
        .header("MCP-Protocol-Version", "2025-06-18")
        .json(&rpc("tools/list"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn missing_protocol_version_header_still_succeeds() {
    // The header is optional — old clients that never negotiated a protocol
    // version never send it, and must not be locked out.
    let server = TestServer::start().await;
    let owner = seed_user(&server, "gw-owner-pv-none").await;
    let agent_id = seed_agent(&server, owner, "pv-agent-none").await;
    let token = common::mint_gateway_token(&server.db, agent_id).await;

    let resp = server
        .client
        .post(server.url("/api/mcp"))
        .bearer_auth(&token)
        .json(&rpc("tools/list"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn unimplemented_method_probe_is_not_blocked_by_protocol_version_header() {
    // openai-agents, pydantic-ai, and Claude Code's streamable-http clients open
    // a session with a method this gateway doesn't implement (`server/discover`),
    // carrying whatever protocol version they intend to negotiate, and rely on a
    // clean -32601 to trigger their fallback to `initialize`. The header check
    // must not block that first probe just because it names a version we've
    // never negotiated — regression test for the ordering bug in 5ee205ca.
    let server = TestServer::start().await;
    let owner = seed_user(&server, "gw-owner-probe").await;
    let agent_id = seed_agent(&server, owner, "probe-agent").await;
    let token = common::mint_gateway_token(&server.db, agent_id).await;

    let resp = server
        .client
        .post(server.url("/api/mcp"))
        .bearer_auth(&token)
        .header("MCP-Protocol-Version", "2026-07-28")
        .json(&rpc("server/discover"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(
        body["error"]["code"],
        nasiko_mcp_gateway::types::codes::METHOD_NOT_FOUND,
        "unhandled method must fall through to a clean JSON-RPC -32601, got: {body}"
    );
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn initialize_negotiates_even_with_an_unnegotiated_header_value() {
    // `initialize` is the negotiation request itself — it must never be gated
    // by a header value the client couldn't yet have negotiated. Stronger
    // than "some supported version comes back": the header carries a
    // DIFFERENT (but supported) version than `params.protocolVersion`, so
    // this proves negotiation is driven by params, never by the header.
    let server = TestServer::start().await;
    let owner = seed_user(&server, "gw-owner-init-pv").await;
    let agent_id = seed_agent(&server, owner, "init-pv-agent").await;
    let token = common::mint_gateway_token(&server.db, agent_id).await;

    let resp = server
        .client
        .post(server.url("/api/mcp"))
        .bearer_auth(&token)
        .header("MCP-Protocol-Version", "2025-06-18")
        .json(
            &serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {"protocolVersion": "2024-11-05", "capabilities": {},
                       "clientInfo": {"name": "t", "version": "0"}}}),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(
        body["result"]["protocolVersion"], "2024-11-05",
        "negotiation must come from params.protocolVersion, not the header: {body}"
    );
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn implemented_method_still_gates_on_protocol_version_header() {
    // The check must still bite where it should: a method the gateway does
    // implement, carrying a version it doesn't.
    let server = TestServer::start().await;
    let owner = seed_user(&server, "gw-owner-gate").await;
    let agent_id = seed_agent(&server, owner, "gate-agent").await;
    let token = common::mint_gateway_token(&server.db, agent_id).await;

    let resp = server
        .client
        .post(server.url("/api/mcp"))
        .bearer_auth(&token)
        .header("MCP-Protocol-Version", "2026-07-28")
        .json(&rpc("tools/list"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn undecodable_protocol_version_header_is_400() {
    // A header that fails `to_str()` (not valid visible ASCII) must be
    // rejected outright when the check applies, not silently treated as
    // absent.
    let server = TestServer::start().await;
    let owner = seed_user(&server, "gw-owner-badenc").await;
    let agent_id = seed_agent(&server, owner, "badenc-agent").await;
    let token = common::mint_gateway_token(&server.db, agent_id).await;

    let resp = server
        .client
        .post(server.url("/api/mcp"))
        .bearer_auth(&token)
        .header("MCP-Protocol-Version", &b"\xff\xfe"[..])
        .json(&rpc("tools/list"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
    server.cleanup().await;
}

// ─── `/api/mcp` body-size limit ────────────────────────────────────────────
//
// The gateway routes raise axum's blanket 2 MiB default to
// `MCP_GATEWAY_MAX_BODY_BYTES` (8 MiB in this test config, matching
// production's default) — see `oss/server/src/mcp/mod.rs::agent_gateway_router`.

fn rpc_padded(method: &str, pad_bytes: usize) -> serde_json::Value {
    serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": method,
        "params": {"name": "some__tool", "arguments": {}, "pad": "a".repeat(pad_bytes)}})
}

#[tokio::test]
#[serial]
async fn a_body_under_the_raised_limit_is_not_rejected() {
    let server = TestServer::start().await;
    let owner = seed_user(&server, "gw-owner-body-ok").await;
    let agent_id = seed_agent(&server, owner, "body-agent-ok").await;
    let token = common::mint_gateway_token(&server.db, agent_id).await;

    let resp = server
        .client
        .post(server.url("/api/mcp"))
        .bearer_auth(&token)
        .json(&rpc_padded("tools/list", 3 * 1024 * 1024))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        200,
        "a ~3 MiB body must fit under the raised 8 MiB limit — axum's own 2 MiB \
         default would have rejected it"
    );
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_body_over_the_raised_limit_is_413() {
    let server = TestServer::start().await;
    let owner = seed_user(&server, "gw-owner-body-big").await;
    let agent_id = seed_agent(&server, owner, "body-agent-big").await;
    let token = common::mint_gateway_token(&server.db, agent_id).await;

    let resp = server
        .client
        .post(server.url("/api/mcp"))
        .bearer_auth(&token)
        .json(&rpc_padded("tools/list", 9 * 1024 * 1024))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 413);
    server.cleanup().await;
}

// ─── POST/DELETE /api/agents/{id}/mcp-token ──────────────────────────────────
//
// A local coding agent (Claude Code / Codex / OpenCode, bound via `nasiko
// connect`) is never deployed, so it never receives a credential through
// deploy-time wiring (`mcp::wiring::inject_agent_gateway_token`). `POST`
// mints (rotates) that same `agent_gateway_tokens` credential on demand, but
// only for a CLI-bound row (`coding_agent_integration_id` set): minting for a
// deployed agent would leave its running container(s) holding a token the
// row stops recognizing once the rotation grace window elapses, with no way
// to hand them the new one short of a redeploy. Rotation keeps that same
// grace window, so `POST` alone does not cut off a leaked credential —
// that's what `DELETE` is for: a general kill switch, superuser included
// (revoking grants nothing, unlike minting), effective immediately.

async fn post_mcp_token(server: &TestServer, jwt: &str, agent_id: Uuid) -> reqwest::Response {
    server
        .client
        .post(server.url(&format!("/api/agents/{agent_id}/mcp-token")))
        .bearer_auth(jwt)
        .json(&serde_json::json!({}))
        .send()
        .await
        .unwrap()
}

async fn delete_mcp_token(server: &TestServer, jwt: &str, agent_id: Uuid) -> reqwest::Response {
    server
        .client
        .delete(server.url(&format!("/api/agents/{agent_id}/mcp-token")))
        .bearer_auth(jwt)
        .send()
        .await
        .unwrap()
}

/// `post_mcp_token`, asserting the mint succeeded, then extracting just the
/// plaintext token — for tests that only need to use the token afterward,
/// not inspect the rest of the response.
async fn mint_token(server: &TestServer, jwt: &str, agent_id: Uuid) -> String {
    let res = post_mcp_token(server, jwt, agent_id).await;
    assert_eq!(res.status(), 200, "mint must succeed");
    let body: serde_json::Value = res.json().await.unwrap();
    body["data"]["token"]
        .as_str()
        .expect("token present")
        .to_owned()
}

/// Stamp the row as CLI-bound — the predicate `POST /mcp-token` requires
/// before it will mint (`coding_agent_integration_id IS NOT NULL`, the same
/// column `oss/llm-router/src/resolver/mod.rs` and the gateway's rule 3b read).
async fn mark_coding_agent(server: &TestServer, agent_id: Uuid) {
    sqlx::query("UPDATE agents SET coding_agent_integration_id = 'claude' WHERE id = $1")
        .bind(agent_id)
        .execute(&server.db)
        .await
        .expect("mark coding agent");
}

#[tokio::test]
#[serial]
async fn owner_can_mint_mcp_token_and_it_authenticates_at_the_gateway() {
    let server = TestServer::start_with(|cfg| cfg.mcp_gateway_public_url = None).await;
    let owner = seed_user(&server, "mcpt-owner-1").await;
    let agent = seed_agent(&server, owner, "mcpt-agent-1").await;
    mark_coding_agent(&server, agent).await;
    let jwt = common::sign_token(&owner.to_string(), "mcpt-owner-1", false, "member");

    let res = post_mcp_token(&server, &jwt, agent).await;
    assert_eq!(res.status(), 200);
    assert_eq!(
        res.headers()
            .get("cache-control")
            .and_then(|v| v.to_str().ok()),
        Some("no-store")
    );
    let body: serde_json::Value = res.json().await.unwrap();
    let token = body["data"]["token"]
        .as_str()
        .expect("token present")
        .to_owned();
    assert!(token.starts_with("ngt_"), "unexpected token shape: {token}");
    assert!(body["data"]["gateway_url"].is_null(), "body: {body}");
    assert!(body["data"]["connect_url"].is_null(), "body: {body}");

    let mcp_res = post_mcp(&server, Some(&token), None, &rpc("initialize")).await;
    assert_eq!(mcp_res.status(), 200);
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn mint_returns_connect_url_when_gateway_public_url_is_configured() {
    let server = TestServer::start_with(|cfg| {
        cfg.mcp_gateway_public_url = Some("http://gateway.test/api/mcp/".to_string());
    })
    .await;
    let owner = seed_user(&server, "mcpt-owner-url").await;
    let agent = seed_agent(&server, owner, "mcpt-agent-url").await;
    mark_coding_agent(&server, agent).await;
    let jwt = common::sign_token(&owner.to_string(), "mcpt-owner-url", false, "member");

    let res = post_mcp_token(&server, &jwt, agent).await;
    assert_eq!(res.status(), 200);
    let body: serde_json::Value = res.json().await.unwrap();
    let token = body["data"]["token"]
        .as_str()
        .expect("token present")
        .to_owned();
    // `trim_end_matches` strips every trailing slash, so the configured value
    // (with one) comes back trimmed — the route itself is `/api/mcp`, not
    // `/api/mcp/`, which would 404.
    assert_eq!(
        body["data"]["gateway_url"], "http://gateway.test/api/mcp",
        "body: {body}"
    );
    assert_eq!(
        body["data"]["connect_url"],
        format!("http://gateway.test/api/mcp/s/{token}"),
        "body: {body}"
    );
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn minting_again_rotates_and_the_new_token_works() {
    let server = TestServer::start().await;
    let owner = seed_user(&server, "mcpt-owner-rot").await;
    let agent = seed_agent(&server, owner, "mcpt-agent-rot").await;
    mark_coding_agent(&server, agent).await;
    let jwt = common::sign_token(&owner.to_string(), "mcpt-owner-rot", false, "member");

    let first_token = mint_token(&server, &jwt, agent).await;
    let second_token = mint_token(&server, &jwt, agent).await;
    assert_ne!(first_token, second_token, "minting again must rotate");

    let res = post_mcp(&server, Some(&second_token), None, &rpc("initialize")).await;
    assert_eq!(
        res.status(),
        200,
        "the freshly minted token must authenticate"
    );
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn post_on_a_non_coding_agent_is_409() {
    // Minting for a deployed agent would leave its running container (or,
    // under KubeRuntime, every replica) holding a token the row stops
    // recognizing once the rotation grace window lapses, with no way to hand
    // it the new one short of a redeploy — POST is for CLI-bound rows only.
    let server = TestServer::start().await;
    let owner = seed_user(&server, "mcpt-owner-deployed").await;
    let agent = seed_agent(&server, owner, "mcpt-agent-deployed").await;
    let deploy_time_token = common::mint_gateway_token(&server.db, agent).await;
    let jwt = common::sign_token(&owner.to_string(), "mcpt-owner-deployed", false, "member");

    let res = post_mcp_token(&server, &jwt, agent).await;
    assert_eq!(res.status(), 409);
    let body = res.text().await.unwrap();
    assert!(
        body.contains("not a local coding agent"),
        "unexpected body: {body}"
    );

    // The owner gate runs before the row-type check: a non-owner gets 403, not 409.
    let other = seed_user(&server, "mcpt-other-deployed").await;
    let other_jwt = common::sign_token(&other.to_string(), "mcpt-other-deployed", false, "member");
    assert_eq!(
        post_mcp_token(&server, &other_jwt, agent).await.status(),
        403
    );

    // The rejected mint must not have touched the row's existing credential.
    let mcp_res = post_mcp(&server, Some(&deploy_time_token), None, &rpc("initialize")).await;
    assert_eq!(
        mcp_res.status(),
        200,
        "the existing deploy-time token must still authenticate"
    );
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn non_owner_cannot_mint_or_revoke_mcp_token() {
    let server = TestServer::start().await;
    let owner = seed_user(&server, "mcpt-owner-no").await;
    let agent = seed_agent(&server, owner, "mcpt-agent-no").await;
    mark_coding_agent(&server, agent).await;
    let owner_jwt = common::sign_token(&owner.to_string(), "mcpt-owner-no", false, "member");
    let owner_token = mint_token(&server, &owner_jwt, agent).await;

    let other = seed_user(&server, "mcpt-other-no").await;
    let other_jwt = common::sign_token(&other.to_string(), "mcpt-other-no", false, "member");

    let res = post_mcp_token(&server, &other_jwt, agent).await;
    assert_eq!(res.status(), 403);
    let res = delete_mcp_token(&server, &other_jwt, agent).await;
    assert_eq!(res.status(), 403);

    // Neither rejected call touched the owner's real credential.
    let mcp_res = post_mcp(&server, Some(&owner_token), None, &rpc("initialize")).await;
    assert_eq!(mcp_res.status(), 200, "owner's token must still work");
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn non_owning_superuser_cannot_mint_but_can_revoke_mcp_token() {
    // POST stays strictly owner-only (minting would let a superuser
    // impersonate the agent); DELETE is a kill switch a superuser may also
    // pull (revoking grants nothing).
    let server = TestServer::start().await;
    let owner = seed_user(&server, "mcpt-owner-su").await;
    let agent = seed_agent(&server, owner, "mcpt-agent-su").await;
    mark_coding_agent(&server, agent).await;
    let owner_jwt = common::sign_token(&owner.to_string(), "mcpt-owner-su", false, "member");
    let owner_token = mint_token(&server, &owner_jwt, agent).await;

    // Must be a seeded row, not a bare UUID: `validate_session_token` (auth
    // middleware) rejects any session naming a user absent from `users` with
    // 401 before RBAC ever sees the `is_superuser` claim.
    let superuser = seed_user(&server, "mcpt-super-su").await;
    let super_jwt = common::sign_token(&superuser.to_string(), "mcpt-super-su", true, "admin");

    let res = post_mcp_token(&server, &super_jwt, agent).await;
    assert_eq!(
        res.status(),
        403,
        "minting stays owner-only even for a superuser"
    );
    let mcp_res = post_mcp(&server, Some(&owner_token), None, &rpc("initialize")).await;
    assert_eq!(
        mcp_res.status(),
        200,
        "the rejected mint must not have touched the owner's real credential"
    );

    let res = delete_mcp_token(&server, &super_jwt, agent).await;
    assert_eq!(res.status(), 204, "a superuser may revoke");
    let mcp_res = post_mcp(&server, Some(&owner_token), None, &rpc("initialize")).await;
    assert_eq!(mcp_res.status(), 401, "the credential is now revoked");
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn unknown_agent_mcp_token_is_404() {
    let server = TestServer::start().await;
    let user = seed_user(&server, "mcpt-unknown-caller").await;
    let jwt = common::sign_token(&user.to_string(), "mcpt-unknown-caller", false, "member");
    let missing = Uuid::new_v4();

    let res = post_mcp_token(&server, &jwt, missing).await;
    assert_eq!(res.status(), 404);
    let res = delete_mcp_token(&server, &jwt, missing).await;
    assert_eq!(res.status(), 404);
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn soft_deleted_agent_mcp_token_is_404() {
    let server = TestServer::start().await;
    let owner = seed_user(&server, "mcpt-owner-del").await;
    let agent = seed_agent(&server, owner, "mcpt-agent-del").await;
    let jwt = common::sign_token(&owner.to_string(), "mcpt-owner-del", false, "member");

    sqlx::query("UPDATE agents SET deleted_at = now() WHERE id = $1")
        .bind(agent)
        .execute(&server.db)
        .await
        .expect("soft delete");

    let res = post_mcp_token(&server, &jwt, agent).await;
    assert_eq!(res.status(), 404);
    let res = delete_mcp_token(&server, &jwt, agent).await;
    assert_eq!(res.status(), 404);
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn owner_can_revoke_mcp_token_and_the_gateway_rejects_it() {
    let server = TestServer::start().await;
    let owner = seed_user(&server, "mcpt-owner-rev").await;
    let agent = seed_agent(&server, owner, "mcpt-agent-rev").await;
    mark_coding_agent(&server, agent).await;
    let jwt = common::sign_token(&owner.to_string(), "mcpt-owner-rev", false, "member");

    let token = mint_token(&server, &jwt, agent).await;
    let res = post_mcp(&server, Some(&token), None, &rpc("initialize")).await;
    assert_eq!(
        res.status(),
        200,
        "sanity: freshly minted token authenticates"
    );

    let res = delete_mcp_token(&server, &jwt, agent).await;
    assert_eq!(res.status(), 204);

    let res = post_mcp(&server, Some(&token), None, &rpc("initialize")).await;
    assert_eq!(res.status(), 401, "revoked token must stop authenticating");

    // Idempotent: revoking again is still a clean 204, not a 404/500.
    let res = delete_mcp_token(&server, &jwt, agent).await;
    assert_eq!(res.status(), 204);
    server.cleanup().await;
}

// ─── POST /api/agents/{id}/llm-token — shared owner-gate guard ──────────────
//
// `issue_llm_token` shares `require_owner` with `issue_mcp_token`; this one
// test guards that shared gate stays owner-only even for a superuser, since
// no other suite in the tree exercises this route.

#[tokio::test]
#[serial]
async fn issue_llm_token_rejects_non_owning_superuser() {
    let server = TestServer::start().await;
    let owner = seed_user(&server, "llmt-owner-su").await;
    let agent = seed_agent(&server, owner, "llmt-agent-su").await;
    let superuser = seed_user(&server, "llmt-super-su").await;
    let super_jwt = common::sign_token(&superuser.to_string(), "llmt-super-su", true, "admin");

    let res = server
        .client
        .post(server.url(&format!("/api/agents/{agent}/llm-token")))
        .bearer_auth(&super_jwt)
        .json(&serde_json::json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 403);
    server.cleanup().await;
}

// ─── DELETE /api/agents/{id}/llm-token — disconnect stops cached tokens ────
//
// `nasiko disconnect <coding-agent>` revokes the agent's LLM routing tokens so a
// session still holding a cached one stops spending through the router at once.
// A token minted after the revocation (a reconnect) must keep working.

const LLM_REVOKE_JWT_SECRET: &str = "llm-revoke-test-secret";

async fn issue_llm_token(server: &TestServer, jwt: &str, agent: Uuid) -> String {
    let res = server
        .client
        .post(server.url(&format!("/api/agents/{agent}/llm-token")))
        .bearer_auth(jwt)
        .json(&serde_json::json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200, "issuing an LLM routing token");
    let body: serde_json::Value = res.json().await.unwrap();
    body["data"]["token"].as_str().unwrap().to_owned()
}

async fn router_call(server: &TestServer, token: &str) -> (u16, String) {
    let res = server
        .client
        .post(server.url("/v1/chat/completions"))
        .bearer_auth(token)
        .json(&serde_json::json!({
            "model": "gpt-4o-mini",
            "messages": [{"role": "user", "content": "hi"}]
        }))
        .send()
        .await
        .unwrap();
    (res.status().as_u16(), res.text().await.unwrap_or_default())
}

#[tokio::test]
#[serial]
async fn revoking_llm_tokens_refuses_cached_ones_but_not_a_reconnect() {
    // SAFETY: #[serial]; set before the app reads its config at boot, removed at the end.
    unsafe { std::env::set_var("AGENT_JWT_SECRET", LLM_REVOKE_JWT_SECRET) };
    let server =
        TestServer::start_with(|c| c.agent_jwt_secret = LLM_REVOKE_JWT_SECRET.into()).await;
    let owner = seed_user(&server, "llmt-owner-rev").await;
    let agent = seed_agent(&server, owner, "llmt-agent-rev").await;
    mark_coding_agent(&server, agent).await;
    let jwt = common::sign_token(&owner.to_string(), "llmt-owner-rev", false, "member");

    let cached = issue_llm_token(&server, &jwt, agent).await;
    let (_, body) = router_call(&server, &cached).await;
    assert!(
        !body.contains("disconnected"),
        "sanity: not revoked yet: {body}"
    );

    // `iat` has whole-second precision and a same-second token is accepted, so
    // revoke in a later second than the cached token was issued.
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    let res = server
        .client
        .delete(server.url(&format!("/api/agents/{agent}/llm-token")))
        .bearer_auth(&jwt)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 204);

    let (status, body) = router_call(&server, &cached).await;
    assert_eq!(
        status, 401,
        "a token issued before the revoke is refused: {body}"
    );
    assert!(body.contains("disconnected"), "{body}");

    let reconnected = issue_llm_token(&server, &jwt, agent).await;
    let (_, body) = router_call(&server, &reconnected).await;
    assert!(
        !body.contains("disconnected"),
        "a token issued after the revoke works: {body}"
    );

    // Idempotent, and owner-gated.
    let res = server
        .client
        .delete(server.url(&format!("/api/agents/{agent}/llm-token")))
        .bearer_auth(&jwt)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 204);
    let stranger = seed_user(&server, "llmt-stranger-rev").await;
    let stranger_jwt =
        common::sign_token(&stranger.to_string(), "llmt-stranger-rev", false, "member");
    let res = server
        .client
        .delete(server.url(&format!("/api/agents/{agent}/llm-token")))
        .bearer_auth(&stranger_jwt)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 403);

    server.cleanup().await;
    unsafe { std::env::remove_var("AGENT_JWT_SECRET") };
}

use std::collections::HashMap;

use axum::{
    body::Body,
    extract::{Path, Request, State},
    http::StatusCode,
    response::Response,
};
use nasiko_flow::{FlowContext, TRACEPARENT_HEADER};
use nasiko_orchestrator::{ContextTiers, context_selection};
use uuid::Uuid;

use crate::auth::Claims;
use crate::state::AppState;

/// Proxy agent requests: auth → flow guard → resolve endpoint → forward.
///
/// Runs inside the `require_auth` middleware so Claims is always present.
/// Forwards the request to the agent container, propagating traceparent and
/// identity headers so agents know who is calling them.
///
/// Handles both `/agents/{id}` and `/agents/{id}/{*rest}` routes.
pub async fn agent_proxy(
    State(state): State<AppState>,
    Path(params): Path<HashMap<String, String>>,
    req: Request,
) -> Result<Response, StatusCode> {
    let agent_id: Uuid = params
        .get("id")
        .and_then(|s| s.parse().ok())
        .ok_or(StatusCode::BAD_REQUEST)?;
    let claims = req
        .extensions()
        .get::<Claims>()
        .cloned()
        .ok_or(StatusCode::UNAUTHORIZED)?;

    // TEMP DEBUG: dump the inbound request headers so we can see what identifiers
    // (traceparent/trace_id, session_id, flow_id, x-nasiko-*) arrive natively vs.
    // what we mint. Remove once the header audit is done.
    crate::router::a2a_dispatch::log_inbound_headers("agent_proxy (direct agent)", req.headers());

    // Build the forwarded path: everything after /api/agents/{id}
    let full_path = req.uri().path();
    let id_str = agent_id.to_string();
    let forwarded_path = full_path
        .find(&id_str)
        .map(|pos| {
            let after_id = &full_path[pos + id_str.len()..];
            if after_id.is_empty() {
                "/".to_string()
            } else {
                after_id.to_string()
            }
        })
        .unwrap_or_else(|| "/".to_string());

    // Per-agent authorization — mirrors the check `a2a_dispatch.rs` already
    // enforces before forwarding. Without this, any authenticated user could
    // invoke any private agent by UUID (IDOR), and combined with the header
    // leak below, would also hand the agent their platform credentials.
    // 404 (not 403) to avoid confirming a private agent's existence.
    if !crate::acl::can_access_agent(&state, &claims, agent_id).await {
        return Err(StatusCode::NOT_FOUND);
    }

    // Flow guard: prevent infinite A2A cascades
    let traceparent = req
        .headers()
        .get(TRACEPARENT_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(String::from);
    // Real span for this proxy hop. It joins the caller's trace when a
    // traceparent came in (agent→agent cascades keep one flow/trace id),
    // else becomes a fresh root. Either way the traceparent forwarded below
    // names THIS span — which is actually exported — so the target agent's
    // spans parent to a real node instead of a phantom random span id.
    let proxy_span = tracing::info_span!(
        "a2a.proxy",
        otel.kind = "server",
        gen_ai.operation.name = "invoke_agent",
        agent.id = %agent_id,
        session.id = tracing::field::Empty,
        gen_ai.input.messages = tracing::field::Empty,
    );
    if let Some(cx) = traceparent
        .as_deref()
        .and_then(crate::telemetry::remote_context_from_traceparent)
    {
        use tracing_opentelemetry::OpenTelemetrySpanExt as _;
        proxy_span.set_parent(cx);
    }
    let flow_ctx = crate::telemetry::flow_context_from_span(&proxy_span)
        .or_else(|| {
            traceparent
                .as_deref()
                .and_then(FlowContext::from_traceparent)
        })
        .unwrap_or_else(FlowContext::new_root);

    let agent_id_str = agent_id.to_string();
    if let Err(rejection) = state.flow_guard.check(&flow_ctx, &agent_id_str).await {
        tracing::warn!(%rejection, %agent_id_str, "flow cascade rejected");
        return Err(StatusCode::LOOP_DETECTED);
    }
    if let Err(rejection) = state
        .flow_guard
        .record_invocation(&flow_ctx, &agent_id_str)
        .await
    {
        tracing::warn!(%rejection, "flow limit hit");
        return Err(StatusCode::LOOP_DETECTED);
    }

    // Resolve the agent's catalog record. Its `agents.url` is a snapshot taken
    // at the last deploy/restart — stale the moment the container is recreated
    // outside that flow (Docker/Podman assign a new random host port on every
    // recreate), and possibly empty (a k8s deploy that persisted before the
    // pod was Ready). Prefer the live runtime lookup instead (same fix already
    // applied in `resolve_endpoint` in `router/a2a_dispatch.rs`), falling back
    // to the stored snapshot only if the runtime can't resolve the agent (e.g.
    // external agents registered by URL rather than deployed through this
    // platform).
    let agent = nasiko_agent_proxy::resolve(&state.db, agent_id)
        .await
        .map_err(|e| match e {
            nasiko_agent_proxy::ResolveError::NotFound => StatusCode::NOT_FOUND,
            nasiko_agent_proxy::ResolveError::NotRunning(_) => StatusCode::SERVICE_UNAVAILABLE,
            nasiko_agent_proxy::ResolveError::Database(_) => StatusCode::INTERNAL_SERVER_ERROR,
        })?;

    let agent_base = match state
        .runtime
        .endpoint(&nasiko_runtime::ContainerId::from_uuid(agent_id))
        .await
    {
        Ok(live) => live.trim_end_matches('/').to_string(),
        Err(e) => {
            let Some(stored) = &agent.endpoint else {
                tracing::error!(
                    error = %e, %agent_id,
                    "agent proxy: live endpoint lookup failed and no stored agents.url to fall back to"
                );
                return Err(StatusCode::BAD_GATEWAY);
            };
            tracing::warn!(
                error = %e, %agent_id,
                "agent proxy: live endpoint lookup failed, falling back to stored agents.url"
            );
            format!("http://{}:{}", stored.host, stored.port)
        }
    };
    // The forwarded path. A bare `/` (the chat call) must land on the agent's
    // actual A2A mount, which the card advertises as `transport_path` — `/a2a`
    // for the a2a-go agents, root for the a2a-server-lf ones. POSTing to `/`
    // unconditionally 404s every agent mounted elsewhere. Sub-paths (e.g.
    // `/.well-known/agent-card.json`) are forwarded verbatim.
    let forwarded_path = if forwarded_path == "/" {
        match agent.transport_path.as_deref() {
            None | Some("") | Some("/") => "/".to_string(),
            Some(p) => p.to_string(),
        }
    } else {
        forwarded_path
    };
    let target_url = format!("{agent_base}{forwarded_path}");

    // Forward the request
    let method = req.method().clone();
    let headers = req.headers().clone();
    let body_bytes = axum::body::to_bytes(req.into_body(), 10 * 1024 * 1024)
        .await
        .map_err(|_| StatusCode::BAD_REQUEST)?;

    // Every message send must belong to a persisted session (may inject a
    // generated contextId into the body — see `ensure_chat_session`).
    let (body_bytes, persist_info) =
        ensure_chat_session(&state, &claims, agent_id, &agent_base, body_bytes).await?;

    // Persist the user message to chat_messages (fire-and-forget, mirrors CLI
    // behaviour). No trace_id column: session_traces (below) is the
    // authoritative session↔trace mapping now.
    //
    // The 10s dedup guard exists because the CLI's A2A method negotiation
    // (`send_message` in oss/cli/src/commands/chat.rs) retries the same
    // logical message under a different JSON-RPC method name (message/stream
    // -> SendStreamingMessage -> SendMessage -> message/send) when an agent
    // rejects one with "method not found" — each retry is a fresh proxied
    // request and would otherwise persist an identical duplicate row before
    // the agent has even accepted the call.
    if let Some(ref info) = persist_info
        && !info.user_text.is_empty()
    {
        proxy_span.record("session.id", info.session_id.as_str());
        if state.config.otel_capture_content {
            proxy_span.record(
                "gen_ai.input.messages",
                crate::telemetry::genai_text_message("user", &info.user_text).as_str(),
            );
        }
        let db = state.db.clone();
        let session_id = info.session_id.clone();
        let user_text = info.user_text.clone();
        tokio::spawn(async move {
            let _ = sqlx::query(
                "INSERT INTO chat_messages (session_id, role, content) \
                 SELECT $1, $2, $3 WHERE NOT EXISTS ( \
                     SELECT 1 FROM chat_messages \
                     WHERE session_id = $1 AND role = $2 AND content = $3 \
                       AND timestamp > now() - INTERVAL '10 seconds')",
            )
            .bind(&session_id)
            .bind("user")
            .bind(&user_text)
            .execute(&db)
            .await;
        });
    }

    // Register the flow so the model router can classify this request. A direct
    // agent call opens a real flow too: the caller forwards a `traceparent`, but
    // only the orchestrator path (`a2a_dispatch.rs`) ever wrote a `flows` row, so
    // the gateway's `derive_boundary_signals` lookup missed and fell back to
    // `inert` — the classifier never fired and the resolved/default model was
    // used. Registering here with default `{}` metadata (⇒ free-flowing mode)
    // makes the forwarded trace id a *known* flow, so the boundary is fireable,
    // exactly like the orchestrator. This runs before the request is forwarded so
    // the row is guaranteed present before the agent can call back into the LLM
    // gateway.
    //
    // On conflict the flow is *re-opened* rather than left as-is. A client may
    // send several requests under one traceparent — `nasiko chat` mints one per
    // turn and reuses it across its protocol-negotiation attempts — and the first
    // one to return marks the row `completed`. Leaving it that way made every
    // later request under that trace unattributable: strict attribution requires
    // a `running` flow, so the agent's LLM and MCP calls came back 403
    // "traceparent does not resolve to a live flow" even though the platform
    // itself had just opened that flow. Only the liveness columns are touched —
    // `user_id` is never reassigned, so replaying someone else's trace id cannot
    // move billing — and `created_at` still ages the row out of the attribution
    // window normally.
    if let Ok(user_id) = claims.user_uuid() {
        let title = persist_info.as_ref().map(|i| i.user_text.clone());
        // Carry the A2A context_id (the session id `ensure_chat_session` minted
        // or adopted) on the flow row so the LLM gateway can key its decision
        // cache on the *conversation*, not this turn's trace id. The CLI re-mints
        // the traceparent every turn, so a trace-id key would never survive to
        // the next turn; `derive_boundary_signals` reads `metadata->>'context_id'`
        // and uses it as the sticky key, so turn 2+ reuse the model chosen at the
        // turn-1 cold start. Written on the same synchronous, pre-forward insert
        // so the value is guaranteed present before the agent calls back.
        let metadata = match persist_info.as_ref() {
            Some(i) => serde_json::json!({ "context_id": i.session_id }),
            None => serde_json::json!({}),
        };
        let _ = sqlx::query(
            r#"INSERT INTO flows (flow_id, user_id, root_agent_id, root_agent_name, title, status, metadata)
               VALUES ($1, $2, $3, $4, $5, 'running', $6)
               ON CONFLICT (flow_id) DO UPDATE
                  SET status = 'running', completed_at = NULL"#,
        )
        .bind(&flow_ctx.flow_id)
        .bind(user_id)
        .bind(agent_id)
        .bind(&agent.name)
        .bind(title)
        .bind(&metadata)
        .execute(&state.db)
        .await;
        // Membership record for the flow: the MCP gateway / LLM router only
        // authorize this agent's calls if it is a recorded participant of the
        // traceparent-named flow. Rides the same synchronous, pre-forward write
        // as the flows row so it is guaranteed present before the agent can
        // call back into the platform.
        crate::flows::record_participant(&state.db, &flow_ctx.flow_id, agent_id).await;
    }

    // Explicit allowlist, not a denylist: the agent container is unvetted, so
    // anything not named here is dropped rather than forwarded by default.
    // In particular this drops `authorization`/`cookie` (the caller's platform
    // credentials — a hostile agent could otherwise replay them against
    // `/api/*`) and any inbound `x-user-id`/`x-username`/`x-is-superuser`
    // (spoofed identity — reqwest's `.header()` below is `HeaderMap::append`,
    // not replace, so a copied attacker value would sit ahead of the trusted
    // one most header readers return the first occurrence of).
    const FORWARDED_HEADERS: &[&str] = &[
        "content-type",
        "accept",
        "accept-encoding",
        "accept-language",
        "a2a-version",
    ];
    // Agent turns outrun the shared client's short default (long tool calls,
    // multi-step orchestration), so this hop carries the agent-call budget.
    let mut forwarded =
        state
            .http_client
            .request(method, &target_url)
            .timeout(std::time::Duration::from_secs(
                state.config.agent_call_timeout_secs,
            ));
    for (name, value) in headers.iter() {
        if FORWARDED_HEADERS.contains(&name.as_str())
            && let Ok(val_str) = value.to_str()
        {
            forwarded = forwarded.header(name.as_str(), val_str);
        }
    }

    // Propagate trace context and caller identity to the agent
    forwarded = forwarded
        .header("traceparent", crate::telemetry::traceparent_for(&flow_ctx))
        .header("x-user-id", &claims.sub)
        .header("x-username", &claims.username)
        .header(
            "x-is-superuser",
            if claims.is_superuser { "true" } else { "false" },
        );

    // No per-request MCP credential is forwarded: the agent calls /api/mcp with
    // its own deploy-time MCP_GATEWAY_TOKEN, and the gateway resolves the user
    // from the traceparent via the flows row + flow_participants record written
    // above (docs/MCP_GATEWAY_AGENT_AUTH.md).

    // Best-effort: record the session ↔ trace correlation so observability
    // can map Tempo traces back to chat sessions for agents that don't set
    // session.id on their spans (anything not running the Python patch).
    // Fire-and-forget so it never delays the proxy; ensure_chat_session above
    // already guaranteed the chat_sessions row, so the FK holds.
    if !body_bytes.is_empty()
        && let Ok(body_json) = serde_json::from_slice::<serde_json::Value>(&body_bytes)
    {
        let context_id = body_json
            .pointer("/params/message/contextId")
            .or_else(|| body_json.pointer("/params/contextId"))
            .and_then(|v| v.as_str())
            .map(String::from);

        if let Some(ctx_id) = context_id {
            let trace_id = flow_ctx.flow_id.clone();
            let agent_name = agent.name.clone();
            let db = state.db.clone();
            tokio::spawn(async move {
                if let Err(e) = sqlx::query(
                    "INSERT INTO session_traces (session_id, trace_id, agent_id, agent_name)
                     VALUES ($1, $2, $3, $4)
                     ON CONFLICT (session_id, trace_id) DO NOTHING",
                )
                .bind(&ctx_id)
                .bind(&trace_id)
                .bind(agent_id)
                .bind(&agent_name)
                .execute(&db)
                .await
                {
                    tracing::warn!(error = %e, session_id = %ctx_id, %trace_id, "agent proxy: session trace record failed");
                }
            });
        }
    }

    if !body_bytes.is_empty() {
        forwarded = forwarded.body(body_bytes);
    }

    let response = forwarded.send().await.map_err(|e| {
        tracing::error!(error = %e, %agent_id, %target_url, "agent proxy: request to agent failed");
        StatusCode::BAD_GATEWAY
    })?;

    state.flow_guard.record_return(&flow_ctx).await;

    // Build the response. For non-streaming replies we read the bytes once,
    // which lets us persist the agent message to chat_messages before sending.
    let status = response.status();
    let resp_headers = response.headers().clone();
    let is_stream = resp_headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|ct| ct.contains("text/event-stream"));

    let mut builder = Response::builder().status(status.as_u16());
    for (name, value) in resp_headers.iter() {
        builder = builder.header(name, value);
    }

    if is_stream {
        use futures::StreamExt as _;
        let stream = response.bytes_stream();
        // Streamed replies never pass through `extract_agent_reply_text`
        // below, so tap the SSE chunks on their way to the client and persist
        // the collected assistant text when the stream ends (Drop also covers
        // a client disconnect mid-stream).
        let tap = SseReplyTap::new(
            state.db.clone(),
            state.hitl_store.clone(),
            persist_info.as_ref().map(|i| i.session_id.clone()),
            agent_id,
            flow_ctx.flow_id.clone(),
            claims.sub.parse::<Uuid>().ok(),
        );
        // `stream::unfold`, not `take_while`: `take_while`'s predicate only runs when a
        // NEW upstream chunk arrives, so an agent that emits the pausing event and then
        // goes quiet — never sends anything further, never closes the connection
        // (documented live behavior of some a2a-sdk agents) — would never re-evaluate the
        // predicate, and the relay would hang forever: exactly the `nasiko chat -a` hang
        // this change claims to fix, just moved one event later. `unfold` decides to stop
        // right after the chunk that fed the pause is yielded, driven by the *client's*
        // next read of the response body (via hyper/axum), not by another upstream poll —
        // so ending the stream never depends on the agent sending or closing anything else.
        let boxed: std::pin::Pin<
            Box<dyn futures::Stream<Item = reqwest::Result<bytes::Bytes>> + Send>,
        > = Box::pin(stream);
        let body = Body::from_stream(futures::stream::unfold(
            RelayStep::Read(boxed, tap),
            |step| async move {
                match step {
                    RelayStep::Done => None,
                    // One extra chunk carrying the synthetic "type":"hitl" frame, injected
                    // right after the agent's own pause event — see `RelayStep`'s doc
                    // comment for why this can't just be yielded inline in the same step.
                    RelayStep::Inject(extra) => Some((Ok(extra), RelayStep::Done)),
                    RelayStep::Read(mut stream, mut tap) => {
                        let chunk_result = stream.next().await?;
                        if let Ok(bytes) = &chunk_result {
                            tap.collector.feed(bytes);
                        }
                        if !tap.collector.is_paused() {
                            return Some((chunk_result, RelayStep::Read(stream, tap)));
                        }
                        // Persist synchronously here, before the stream is allowed to
                        // close, and inject the id-carrying frame on this same still-open
                        // connection: this used to happen only in `Drop`, after the stream
                        // had already ended, so a client had no id to resolve against at
                        // the moment it needed one.
                        match tap.build_pause_injection().await {
                            Some(extra) => Some((chunk_result, RelayStep::Inject(extra))),
                            None => Some((chunk_result, RelayStep::Done)),
                        }
                    }
                }
            },
        ));
        return builder.body(body).map_err(|e| {
            tracing::error!(error = %e, %agent_id, "agent proxy: failed to build streamed response");
            StatusCode::INTERNAL_SERVER_ERROR
        });
    }

    let bytes = response.bytes().await.map_err(|e| {
        tracing::error!(error = %e, %agent_id, "agent proxy: failed to read agent response body");
        StatusCode::BAD_GATEWAY
    })?;

    // A non-streaming reply can pause too — same check `agent_stream()`'s own non-streaming
    // branch runs, so this proxy doesn't flatten a pause into a bogus "assistant reply" just
    // because the agent happened to answer as plain JSON.
    //
    // Tracked synchronously (the classification below is already synchronous) so the
    // `complete_flow` call further down can skip a flow that just paused: `persist_direct_chat_pause`
    // sets `flows.status = 'paused'` from inside the spawned task below, and calling
    // `complete_flow` (sets `'completed'`) unconditionally right after racing that same update —
    // `completed` usually won since it's the shorter path — made a paused flow read as finished in
    // the UI, with the resumed hop unattributable to it.
    let mut is_paused = false;

    if let Some(ref info) = persist_info
        && let Ok(rpc_val) = serde_json::from_slice::<serde_json::Value>(&bytes)
    {
        let raw_body = rpc_val.to_string();
        if nasiko_types::a2a::classify_stream_disposition(&raw_body)
            == nasiko_types::a2a::StreamDisposition::Paused
        {
            is_paused = true;
            if let Ok(owner_user_id) = claims.sub.parse::<Uuid>() {
                let db = state.db.clone();
                let hitl_store = state.hitl_store.clone();
                let session_id = info.session_id.clone();
                let flow_id = flow_ctx.flow_id.clone();
                tokio::spawn(async move {
                    let question = crate::router::a2a_dispatch::build_pause_question(&raw_body);
                    let message_text = question
                        .get("message")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string();
                    let fallback_task_id = Uuid::new_v4().to_string();

                    // `session_id` here IS the `chat_sessions.session_id` `ensure_chat_session`
                    // above unconditionally upserted (`persist_info.session_id`, threaded through
                    // from there) — passed through as the real `chat_session_id` so a proxy-origin
                    // pause is findable via `GET /api/chat/sessions/{id}/messages` like every other
                    // origin's, not only via the global `/api/hitl/pending` list.
                    if crate::router::a2a_dispatch::persist_direct_chat_pause(
                        &hitl_store,
                        &db,
                        nasiko_hitl::HitlOrigin::AgentProxy,
                        agent_id,
                        owner_user_id,
                        &session_id,
                        &fallback_task_id,
                        Some(&session_id),
                        &flow_id,
                        &raw_body,
                    )
                    .await
                    .is_err()
                    {
                        tracing::error!(%session_id, "agent proxy: failed to persist HITL pause");
                    }
                    if !message_text.is_empty() {
                        let _ = sqlx::query(
                            "INSERT INTO chat_messages (session_id, role, content) VALUES ($1, $2, $3)",
                        )
                        .bind(&session_id)
                        .bind("assistant")
                        .bind(&message_text)
                        .execute(&db)
                        .await;
                    }
                });
            } else {
                tracing::error!(session_id = %info.session_id, "agent proxy: HITL pause detected but caller's user id didn't parse — pause not persisted");
            }
        } else if let Some(agent_text) = extract_agent_reply_text(&rpc_val) {
            let db = state.db.clone();
            let session_id = info.session_id.clone();
            tokio::spawn(async move {
                let _ = sqlx::query(
                    "INSERT INTO chat_messages (session_id, role, content) VALUES ($1, $2, $3)",
                )
                .bind(&session_id)
                .bind("assistant")
                .bind(&agent_text)
                .execute(&db)
                .await;
            });
        }
    }

    // The response body is fully buffered — the flow's work is done, unless it just paused (see
    // `is_paused`'s doc comment above), in which case `persist_direct_chat_pause` owns the flow's
    // terminal-for-now status instead. Without this at all the direct-chat path's rows stayed
    // 'running' forever, which would poison the LLM router's active-flow attribution fallback (it
    // only looks at running flows).
    if !is_paused {
        complete_flow(&state.db, &flow_ctx.flow_id);
    }

    builder.body(Body::from(bytes)).map_err(|e| {
        tracing::error!(error = %e, %agent_id, "agent proxy: failed to build response");
        StatusCode::INTERNAL_SERVER_ERROR
    })
}

/// Mark a direct-chat flow completed (fire-and-forget). `a2a_dispatch` marks
/// its own flows; the proxy path never did, so they accumulated as 'running'.
fn complete_flow(db: &sqlx::PgPool, flow_id: &str) {
    let db = db.clone();
    let flow_id = flow_id.to_string();
    tokio::spawn(async move {
        let _ = sqlx::query(
            r#"UPDATE flows SET status = 'completed',
               duration_ms = EXTRACT(EPOCH FROM (now() - created_at))::bigint * 1000,
               completed_at = now()
               WHERE flow_id = $1"#,
        )
        .bind(&flow_id)
        .execute(&db)
        .await;
    });
}

/// A2A JSON-RPC methods that carry a user message and therefore must be bound
/// to a chat session — both the v1 names and the pre-1.0 ones still spoken by
/// older agent images.
const MESSAGE_SEND_METHODS: &[&str] = &[
    "SendMessage",
    "SendStreamingMessage",
    "message/send",
    "message/stream",
];

/// Session and user-message info extracted during `ensure_chat_session` so the
/// proxy can persist both user and agent messages to `chat_messages` without
/// re-parsing the request body a second time.
struct PersistInfo {
    session_id: String,
    user_text: String,
}

/// Guarantee every agent chat happens inside a persisted `chat_sessions` row.
///
/// A message that arrives with a `contextId` gets that id upserted as a
/// session (clients that pre-create via `POST /api/chat/sessions` — the CLI
/// and web UI — hit the ON CONFLICT no-op). A message without one gets a
/// generated `ses_*` id injected into the forwarded body, so the agent's task
/// — and every response event it emits — echoes the session id back to the
/// client, which can then resume with it.
///
/// Non-message traffic (GetTask, card fetches, …) passes through untouched.
/// Returns `(body_bytes, Some(PersistInfo))` for message-send requests so the
/// caller can persist user + agent messages to `chat_messages`.
async fn ensure_chat_session(
    state: &AppState,
    claims: &Claims,
    agent_id: Uuid,
    _agent_base_url: &str,
    body_bytes: axum::body::Bytes,
) -> Result<(axum::body::Bytes, Option<PersistInfo>), StatusCode> {
    // Not JSON, or not a message send → pass through; the agent is the
    // authority on whether the payload is valid A2A.
    let Ok(mut rpc) = serde_json::from_slice::<serde_json::Value>(&body_bytes) else {
        return Ok((body_bytes, None));
    };
    let is_send = rpc
        .get("method")
        .and_then(|m| m.as_str())
        .is_some_and(|m| MESSAGE_SEND_METHODS.contains(&m));
    if !is_send {
        return Ok((body_bytes, None));
    }
    let Some(message) = rpc.pointer_mut("/params/message") else {
        return Ok((body_bytes, None));
    };

    let user_id: Uuid = claims.sub.parse().map_err(|_| StatusCode::UNAUTHORIZED)?;

    let existing_ctx = message
        .get("contextId")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let (session_id, injected) = match existing_ctx {
        Some(id) => (id, false),
        None => (format!("ses_{}", Uuid::new_v4().simple()), true),
    };

    // First user message doubles as the session title (same convention the
    // web UI uses when it titles a fresh session). Also captured for
    // message persistence in `chat_messages`.
    let user_text: String = message
        .get("parts")
        .and_then(|p| p.as_array())
        .and_then(|parts| {
            parts
                .iter()
                .find_map(|p| p.get("text").and_then(|t| t.as_str()))
        })
        .unwrap_or("")
        .to_string();
    let title = {
        let t = user_text.trim();
        if t.is_empty() {
            "New chat".to_string()
        } else if t.len() > 60 {
            let mut n = 60;
            while !t.is_char_boundary(n) {
                n -= 1;
            }
            format!("{}…", &t[..n])
        } else {
            t.to_string()
        }
    };

    let proxy_url = format!("/api/agents/{agent_id}");
    let inserted = sqlx::query(
        "INSERT INTO chat_sessions (session_id, user_id, agent_id, agent_url, title)
         VALUES ($1, $2, $3, $4, $5)
         ON CONFLICT (session_id) DO NOTHING",
    )
    .bind(&session_id)
    .bind(user_id)
    .bind(agent_id)
    .bind(&proxy_url)
    .bind(&title)
    .execute(&state.db)
    .await
    .map_err(|e| {
        // A dangling user_id FK means the (gateway-verified) JWT references a
        // user that no longer exists — stale credential, not a server fault.
        if let sqlx::Error::Database(ref db_err) = e
            && db_err.constraint() == Some("chat_sessions_user_id_fkey")
        {
            return StatusCode::UNAUTHORIZED;
        }
        tracing::error!(error = %e, %agent_id, %session_id, "agent proxy: session upsert failed");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    // Session already existed — look up who owns it and which agent it was
    // opened against. It must belong to the caller (checked for everyone but
    // superusers), otherwise any authenticated user could graft messages onto
    // someone else's session by guessing/replaying its contextId. The bound
    // agent_id also gates history injection below: `contextId` is
    // caller-supplied and not scoped to an agent, so resuming the same
    // session against a *different* agent must not leak the first agent's
    // conversation into the second agent's prompt — checked regardless of
    // superuser status, since this is about context isolation, not access.
    let same_agent_session = if inserted.rows_affected() == 0 {
        let existing: Option<(Uuid, Uuid)> =
            sqlx::query_as("SELECT user_id, agent_id FROM chat_sessions WHERE session_id = $1")
                .bind(&session_id)
                .fetch_optional(&state.db)
                .await
                .map_err(|e| {
                    tracing::error!(error = %e, %session_id, "agent proxy: session lookup failed");
                    StatusCode::INTERNAL_SERVER_ERROR
                })?;
        match existing {
            Some((owner, bound_agent)) => {
                if !claims.is_superuser && owner != user_id {
                    return Err(StatusCode::FORBIDDEN);
                }
                bound_agent == agent_id
            }
            None => true,
        }
    } else {
        true
    };

    let persist_info = Some(PersistInfo {
        session_id: session_id.clone(),
        user_text: user_text.clone(),
    });

    // Multi-turn continuity: unlike `a2a_dispatch.rs`'s orchestrator path, this
    // proxy forwards the caller's message byte-for-byte, so an agent that (like
    // every example agent) starts a fresh run per call and ignores `contextId`
    // has zero memory of prior turns — even though those turns are sitting right
    // here in `chat_messages` under this same session_id. Stitch them into the
    // outgoing text the same way `a2a_dispatch.rs` already does for the
    // routing/orchestrator path, so direct `--agent`/`-u` chat gets the
    // continuity its own "resume with --session-id" hint implies.
    let mut rewrite_needed = injected;
    if !user_text.is_empty() && same_agent_session {
        let history_store = state.history_vector_store();
        let history = context_selection::fetch_for_user(
            &state.db,
            user_id,
            &session_id,
            &history_store,
            &user_text,
            &ContextTiers::from_config(&state.config),
        )
        .await;
        if !history.is_empty()
            && let Some(part) = message
                .get_mut("parts")
                .and_then(|p| p.as_array_mut())
                .and_then(|parts| parts.iter_mut().find(|p| p.get("text").is_some()))
        {
            part["text"] = serde_json::Value::String(history.with_current_query(&user_text));
            rewrite_needed = true;
        }
    }

    if injected {
        message["contextId"] = serde_json::Value::String(session_id);
    }
    if rewrite_needed {
        let rewritten = serde_json::to_vec(&rpc).map_err(|e| {
            tracing::error!(error = %e, "agent proxy: failed to re-serialize body after contextId/history injection");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
        return Ok((rewritten.into(), persist_info));
    }
    Ok((body_bytes, persist_info))
}

/// Extract the agent's reply text from a JSON-RPC response for persistence.
///
/// Handles both the wrapped `result.task` format (older a2a-sdk) and the flat
/// `result` format, preferring `artifacts[0].parts[].text` then falling back
/// to `status.message.parts[].text`.
fn extract_agent_reply_text(rpc: &serde_json::Value) -> Option<String> {
    task_reply_text(rpc.get("result")?)
}

/// Reply text of a (possibly task-wrapped) A2A result value: the final
/// artifact text, or the terminal status message as a fallback.
pub(crate) fn task_reply_text(result: &serde_json::Value) -> Option<String> {
    // Older a2a-sdk wraps the Task under result.task
    let task = result.get("task").unwrap_or(result);

    // Prefer the final artifact text
    if let Some(text) = task
        .get("artifacts")
        .and_then(|a| a.as_array())
        .and_then(|arr| arr.first())
        .and_then(|artifact| artifact.get("parts"))
        .and_then(|p| p.as_array())
        .and_then(|parts| {
            parts
                .iter()
                .find_map(|p| p.get("text").and_then(|t| t.as_str()))
        })
        .filter(|s| !s.is_empty())
    {
        return Some(text.to_string());
    }

    // Fall back to status.message.parts[].text
    task.pointer("/status/message/parts")
        .and_then(|p| p.as_array())
        .and_then(|parts| {
            parts
                .iter()
                .find_map(|p| p.get("text").and_then(|t| t.as_str()))
        })
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Assembles the assistant's reply text out of an SSE event stream.
///
/// Chunked answers arrive as consecutive `artifactUpdate` (gRPC-style) or
/// `kind: "artifact-update"` (0.3-style) events and are concatenated; a
/// terminal task/status/message event carrying full text is kept only as a
/// fallback, since agents that stream artifact chunks do not repeat the text
/// there — preferring the chunks avoids persisting the answer twice.
#[derive(Default)]
struct SseReplyText {
    line_buf: Vec<u8>,
    artifact_text: String,
    terminal_text: Option<String>,
    /// Set the moment a `Paused`-classified event is fed — the raw `data:` payload, kept for
    /// `pause_kind`/`build_pause_question`/`paused_task_id` to read. Once set, stays set: a
    /// pause is a terminal disposition for this stream, not something a later chunk un-does.
    pause_data: Option<String>,
}

impl SseReplyText {
    fn feed(&mut self, chunk: &[u8]) {
        self.line_buf.extend_from_slice(chunk);
        while let Some(pos) = self.line_buf.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = self.line_buf.drain(..=pos).collect();
            let line = String::from_utf8_lossy(&line);
            let Some(data) = line.trim().strip_prefix("data:") else {
                continue;
            };
            let data = data.trim();
            let Ok(event) = serde_json::from_str::<serde_json::Value>(data) else {
                continue;
            };
            if self.pause_data.is_none()
                && nasiko_types::a2a::classify_stream_disposition(data)
                    == nasiko_types::a2a::StreamDisposition::Paused
            {
                self.pause_data = Some(data.to_string());
            }
            let result = event.get("result").unwrap_or(&event);
            if let Some(text) = artifact_chunk_text(result) {
                self.artifact_text.push_str(&text);
            } else if let Some(text) =
                task_reply_text(result).or_else(|| message_parts_text(result))
            {
                self.terminal_text = Some(text);
            }
        }
    }

    fn is_paused(&self) -> bool {
        self.pause_data.is_some()
    }

    fn finish(&mut self) -> Option<String> {
        if !self.artifact_text.is_empty() {
            return Some(std::mem::take(&mut self.artifact_text));
        }
        self.terminal_text.take()
    }
}

/// Artifact chunk text of one SSE event, in either event dialect.
pub(crate) fn artifact_chunk_text(result: &serde_json::Value) -> Option<String> {
    let artifact = result.pointer("/artifactUpdate/artifact").or_else(|| {
        (result.get("kind").and_then(|k| k.as_str()) == Some("artifact-update"))
            .then(|| result.get("artifact"))
            .flatten()
    })?;
    let parts = artifact.get("parts")?.as_array()?;
    let text: String = parts
        .iter()
        .filter_map(|p| p.get("text").and_then(|t| t.as_str()))
        .collect();
    (!text.is_empty()).then_some(text)
}

/// Text of a bare message event (`{"message": {"parts": [...]}}`).
pub(crate) fn message_parts_text(result: &serde_json::Value) -> Option<String> {
    let parts = result.pointer("/message/parts")?.as_array()?;
    let text: String = parts
        .iter()
        .filter_map(|p| p.get("text").and_then(|t| t.as_str()))
        .collect();
    (!text.is_empty()).then_some(text)
}

/// Step type for the `stream::unfold` relay loop in the streaming branch above. A plain
/// `(stream, tap, done: bool)` tuple (this file's earlier shape) can't express "yield one more,
/// synthetic chunk after the one that triggered the pause" — the extra `"type":"hitl"` frame has
/// to be its own separate item in the outgoing byte stream, not appended onto the agent's own
/// chunk, since a client parses each `data:` line as one JSON event.
enum RelayStep {
    /// Still relaying the agent's own bytes.
    Read(
        std::pin::Pin<Box<dyn futures::Stream<Item = reqwest::Result<bytes::Bytes>> + Send>>,
        SseReplyTap,
    ),
    /// One buffered chunk — the synthetic HITL frame — left to yield before the stream ends.
    Inject(bytes::Bytes),
    Done,
}

/// Persists the collected streamed reply as the session's assistant message when dropped —
/// which happens when the response stream ends, whether the stream completed normally or the
/// client disconnected.
///
/// A pause is handled earlier and synchronously instead, by `build_pause_injection` (called from
/// the relay loop the moment `collector.is_paused()` goes true, before the stream is allowed to
/// close) — unlike `agent_stream()`, this proxy has no prior bookkeeping of its own for this call
/// (no synthetic task id was ever minted; the raw request/response just gets forwarded
/// byte-for-byte), so persisting the `hitl_requests` row happens here too. `Drop`'s own
/// pause-handling branch below still exists as a defensive fallback (matches this same
/// persistence exactly) for the case `build_pause_injection` was never reached at all — it always
/// finds `pause_data` already taken in the normal case, so it's a no-op then.
struct SseReplyTap {
    db: sqlx::PgPool,
    hitl_store: std::sync::Arc<dyn nasiko_hitl::HitlStore>,
    /// Chat session to persist the collected reply into — absent for proxied
    /// requests that carry no user message.
    session_id: Option<String>,
    agent_id: Uuid,
    /// The `flows` row this call registered — needed so a detected pause can flip it to
    /// `'paused'` the same way the direct-chat path (`a2a_dispatch.rs::persist_direct_chat_pause`)
    /// already does, and so completion can close it out otherwise (`complete_flow`); this proxy
    /// previously never touched `flows` again after opening it, leaving the row stuck at
    /// `'running'` forever.
    flow_id: String,
    /// `None` if `claims.sub` didn't parse as a UUID, or if there was no `persist_info` to
    /// correlate a pause to a user in the first place. Guarded rather than unwrapped: the
    /// `hitl_requests` row creation is skipped without a real owner, but the stream is still
    /// correctly truncated either way.
    owner_user_id: Option<Uuid>,
    collector: SseReplyText,
}

impl SseReplyTap {
    fn new(
        db: sqlx::PgPool,
        hitl_store: std::sync::Arc<dyn nasiko_hitl::HitlStore>,
        session_id: Option<String>,
        agent_id: Uuid,
        flow_id: String,
        owner_user_id: Option<Uuid>,
    ) -> Self {
        Self {
            db,
            hitl_store,
            session_id,
            agent_id,
            flow_id,
            owner_user_id,
            collector: SseReplyText::default(),
        }
    }

    /// Called once `collector.is_paused()` goes true: persists the `hitl_requests` row
    /// synchronously (so its id exists in Postgres before any client can observe it — same
    /// ordering guarantee `a2a_dispatch.rs`'s orchestrator/direct-chat paths already give) and
    /// returns the same `"type":"hitl"` id-carrying SSE frame those paths inject, formatted as a
    /// raw wire chunk for this proxy's byte-for-byte relay (which never goes through axum's `Sse`
    /// wrapper, so the "data: ...\n\n" framing has to be built by hand here).
    ///
    /// Takes `pause_data` out of `self.collector` on the way in — `Drop`'s own fallback branch
    /// checks the same field and finds it already gone in the normal case.
    async fn build_pause_injection(&mut self) -> Option<bytes::Bytes> {
        let pause_data = self.collector.pause_data.take()?;
        let question = crate::router::a2a_dispatch::build_pause_question(&pause_data);
        let message_text = question
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        // No Nasiko-synthetic task id exists for this proxy to fall back to (see the struct doc
        // comment) — a fresh one if the payload itself carries none is the best available
        // substitute, same spirit as `agent_stream()`'s own fallback.
        let fallback_task_id = Uuid::new_v4().to_string();

        // Same requirement `Drop`'s own pause-handling branch enforces: without both a session
        // to correlate to and a real caller identity, there's nothing to persist a pause
        // against — logged the same way, so the two paths read as one convention, not two.
        let (Some(session_id), Some(owner_user_id)) = (self.session_id.clone(), self.owner_user_id)
        else {
            tracing::error!(flow_id = %self.flow_id, "agent proxy: HITL pause detected but no session/caller user id to correlate — pause not persisted");
            return None;
        };

        // See the other call site's comment: this proxy has no `chat_sessions` row to correlate
        // to, so `chat_session_id` is `None` here too.
        let row = match crate::router::a2a_dispatch::persist_direct_chat_pause(
            &self.hitl_store,
            &self.db,
            nasiko_hitl::HitlOrigin::AgentProxy,
            self.agent_id,
            owner_user_id,
            &session_id,
            &fallback_task_id,
            None,
            &self.flow_id,
            &pause_data,
        )
        .await
        {
            Ok(row) => row,
            Err(_) => {
                tracing::error!(%session_id, "agent proxy: failed to persist HITL pause");
                return None;
            }
        };

        if !message_text.is_empty() {
            let _ = sqlx::query(
                "INSERT INTO chat_messages (session_id, role, content) VALUES ($1, $2, $3)",
            )
            .bind(&session_id)
            .bind("assistant")
            .bind(&message_text)
            .execute(&self.db)
            .await;
        }

        // `agent`: `None` — this proxy is always a conversation with exactly one agent, already
        // known to whoever is looking at the screen (matches `build_hitl_stream_data`'s own
        // direct-chat convention).
        let task_id = row.task_id.clone().unwrap_or_default();
        let context_id = row.context_id.clone().unwrap_or_default();
        let data = crate::router::a2a_dispatch::build_hitl_stream_data(
            &self.hitl_store,
            &task_id,
            &context_id,
            &row,
            None,
        )
        .await;
        Some(bytes::Bytes::from(format!("data: {data}\n\n")))
    }
}

impl Drop for SseReplyTap {
    fn drop(&mut self) {
        // Drop can run during runtime shutdown, where spawn would panic.
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            return;
        };

        if let Some(pause_data) = self.collector.pause_data.take() {
            let db = self.db.clone();
            let hitl_store = self.hitl_store.clone();
            let session_id = self.session_id.take();
            let agent_id = self.agent_id;
            let flow_id = std::mem::take(&mut self.flow_id);
            let owner_user_id = self.owner_user_id;
            handle.spawn(async move {
                let question = crate::router::a2a_dispatch::build_pause_question(&pause_data);
                let message_text = question
                    .get("message")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();
                // No Nasiko-synthetic task id exists for this proxy to fall back to (see the
                // struct doc comment) — a fresh one if the payload itself carries none is the
                // best available substitute, same spirit as `agent_stream()`'s own fallback.
                let fallback_task_id = Uuid::new_v4().to_string();

                match (&session_id, owner_user_id) {
                    (Some(session_id), Some(owner_user_id)) => {
                        // See the other call site's comment: `session_id` here IS the
                        // `chat_sessions.session_id` row, so it's the real `chat_session_id` too.
                        if crate::router::a2a_dispatch::persist_direct_chat_pause(
                            &hitl_store,
                            &db,
                            nasiko_hitl::HitlOrigin::AgentProxy,
                            agent_id,
                            owner_user_id,
                            session_id,
                            &fallback_task_id,
                            Some(session_id.as_str()),
                            &flow_id,
                            &pause_data,
                        )
                        .await
                        .is_err()
                        {
                            tracing::error!(%session_id, "agent proxy: failed to persist HITL pause");
                        }
                        if !message_text.is_empty() {
                            let _ = sqlx::query(
                                "INSERT INTO chat_messages (session_id, role, content) VALUES ($1, $2, $3)",
                            )
                            .bind(session_id)
                            .bind("assistant")
                            .bind(&message_text)
                            .execute(&db)
                            .await;
                        }
                    }
                    _ => {
                        tracing::error!(%flow_id, "agent proxy: HITL pause detected but no session/caller user id to correlate — pause not persisted");
                    }
                }
            });
            return;
        }

        let db = self.db.clone();
        let session_id = self.session_id.take();
        let text = self.collector.finish();
        handle.spawn(async move {
            if let (Some(session_id), Some(text)) = (session_id, text) {
                let _ = sqlx::query(
                    "INSERT INTO chat_messages (session_id, role, content) VALUES ($1, $2, $3)",
                )
                .bind(&session_id)
                .bind("assistant")
                .bind(&text)
                .execute(&db)
                .await;
            }
        });
        // Stream ended (or the client disconnected): close the flow row.
        complete_flow(&self.db, &self.flow_id);
    }
}

#[cfg(test)]
mod tests {
    use super::SseReplyText;

    #[test]
    fn collects_artifact_chunks_across_split_sse_frames() {
        let mut c = SseReplyText::default();
        // One event split across two network chunks, then a terminal status
        // event that must NOT override the assembled chunks.
        c.feed(br#"data: {"result":{"artifactUpdate":{"artifact":{"parts":[{"text":"Hel"#);
        c.feed("lo\"}]}}}}\n".as_bytes());
        c.feed(
            br#"data: {"result":{"artifactUpdate":{"artifact":{"parts":[{"text":" world"}]}}}}
data: {"result":{"statusUpdate":{"status":{"state":"TASK_STATE_COMPLETED"}}}}
"#,
        );
        assert_eq!(c.finish().as_deref(), Some("Hello world"));
    }

    #[test]
    fn falls_back_to_terminal_task_text_when_nothing_streamed() {
        let mut c = SseReplyText::default();
        c.feed(
            br#"data: {"result":{"task":{"artifacts":[{"parts":[{"text":"final answer"}]}],"status":{"state":"TASK_STATE_COMPLETED"}}}}
"#,
        );
        assert_eq!(c.finish().as_deref(), Some("final answer"));
    }

    #[test]
    fn zero_three_style_artifact_update_events_are_understood() {
        let mut c = SseReplyText::default();
        c.feed(
            br#"data: {"result":{"kind":"artifact-update","artifact":{"parts":[{"text":"pong"}]}}}
"#,
        );
        assert_eq!(c.finish().as_deref(), Some("pong"));
    }

    #[test]
    fn non_reply_streams_persist_nothing() {
        let mut c = SseReplyText::default();
        c.feed(b": keepalive\n\ndata: not-json\n");
        assert_eq!(c.finish(), None);
    }
}

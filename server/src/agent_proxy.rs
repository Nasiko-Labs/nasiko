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
        // Recorded once `ensure_chat_session` below has decided; see `PersistInfo::org_applied`.
        nasiko.prompt_context.org_applied = tracing::field::Empty,
        nasiko.prompt_context.org_policy_chars = tracing::field::Empty,
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

    let (agent_base, forward_headers) = match state
        .runtime
        .forwarding_endpoint(&nasiko_runtime::ContainerId::from_uuid(agent_id))
        .await
    {
        Ok(target) => (target.url.trim_end_matches('/').to_string(), target.headers),
        Err(e) => {
            let Some(stored) = &agent.endpoint else {
                tracing::error!(
                    error = %e, %agent_id,
                    "agent proxy: live endpoint lookup failed and no stored agents.url to fall back to"
                );
                return Err(StatusCode::BAD_GATEWAY);
            };
            // Falls back to the plain stored address, bypassing whatever `forwarding_endpoint()`
            // would otherwise route through (the injection proxy, when memory is enabled) — so
            // this forward gets no L1A. Rare (only on a transient runtime lookup failure) and
            // fails toward availability rather than blocking the call entirely, but worth a line
            // in the logs: a silent L1A skip looks identical to "L1A had nothing to say."
            tracing::warn!(
                error = %e, %agent_id,
                "agent proxy: live endpoint lookup failed, falling back to stored agents.url \
                 (bypasses the injection proxy — this forward gets no L1A)"
            );
            (
                format!("http://{}:{}", stored.host, stored.port),
                Vec::new(),
            )
        }
    };
    // The forwarded path. See `resolve_forwarded_path`'s doc comment for the root-mount case.
    let forwarded_path = resolve_forwarded_path(&forwarded_path, agent.transport_path.as_deref());
    let target_url = format!("{agent_base}{forwarded_path}");

    // Forward the request
    let method = req.method().clone();
    let headers = req.headers().clone();
    let body_bytes = axum::body::to_bytes(req.into_body(), 10 * 1024 * 1024)
        .await
        .map_err(|_| StatusCode::BAD_REQUEST)?;

    // Every message send must belong to a persisted session (may inject a
    // generated contextId into the body — see `ensure_chat_session`).
    let prepared = ensure_chat_session(
        &state,
        &claims,
        agent_id,
        &agent_base,
        body_bytes,
        agent.is_internal,
        !agent.keeps_session_memory,
    )
    .await?;
    let (body_bytes, persist_info) = (prepared.body, prepared.persist);
    if let Some(info) = persist_info.as_ref() {
        proxy_span.record("nasiko.prompt_context.org_applied", info.org_applied);
        proxy_span.record(
            "nasiko.prompt_context.org_policy_chars",
            info.org_policy_chars,
        );
    }

    // What this request's history selection chose from, and what it kept. Recorded here rather
    // than where it was measured: `proxy_span` is the request span the trace page looks for, and
    // only this scope holds it.
    if let (Some(report), Ok(user_id)) = (prepared.context_report.as_ref(), claims.user_uuid()) {
        crate::telemetry::record_context_report(
            &state.db,
            &proxy_span,
            report,
            crate::telemetry::ContextRecord {
                user_id,
                agent_id: Some(agent_id),
                session_id: persist_info.as_ref().map(|p| p.session_id.clone()),
                trace_id: Some(flow_ctx.flow_id.clone()),
                span_id: Some(flow_ctx.parent_span_id.clone()),
            },
        );
    }

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
        // `user_message` is the raw text (before history packing) that the LLM gateway's
        // salience gate and classifier score. An agent→agent hop lands on the conflict
        // branch below and leaves the flow's root user message in place.
        let metadata = match persist_info.as_ref() {
            Some(i) => {
                serde_json::json!({ "context_id": i.session_id, "user_message": i.user_text })
            }
            None => serde_json::json!({}),
        };
        // `session_id` is the same value as `metadata->>'context_id'`, promoted to
        // a column so `session/list` can group and index by it (0058). The JSON
        // stays as it is: `derive_boundary_signals` and the LLM gateway read it
        // from there, and the column is written on this same insert so the two
        // can never disagree.
        let session_id = persist_info.as_ref().map(|i| i.session_id.clone());
        let _ = sqlx::query(
            r#"INSERT INTO flows (flow_id, user_id, root_agent_id, root_agent_name, title, status, metadata, session_id)
               VALUES ($1, $2, $3, $4, $5, 'running', $6, $7)
               ON CONFLICT (flow_id) DO UPDATE
                  SET status = 'running', completed_at = NULL"#,
        )
        .bind(&flow_ctx.flow_id)
        .bind(user_id)
        .bind(agent_id)
        .bind(&agent.name)
        .bind(title)
        .bind(&metadata)
        .bind(session_id)
        .execute(&state.db)
        .await;
        // Membership record for the flow: the MCP gateway / LLM router only
        // authorize this agent's calls if it is a recorded participant of the
        // traceparent-named flow. Rides the same synchronous, pre-forward write
        // as the flows row so it is guaranteed present before the agent can
        // call back into the platform.
        crate::flows::record_participant(&state.db, &flow_ctx.flow_id, agent_id).await;
    }

    // Explicit allowlist, not a denylist: the agent container is unvetted, so anything not named
    // here is dropped rather than forwarded by default. In particular this drops
    // `authorization`/`cookie` (the caller's platform credentials — a hostile agent could
    // otherwise replay them against `/api/*`) and any inbound identity headers (spoofed identity
    // — reqwest's `.header()` below is `HeaderMap::append`, not replace, so a copied attacker
    // value would sit ahead of the trusted one most header readers return the first occurrence
    // of). See `nasiko_agent_proxy::headers`'s module doc for why this is shared with
    // `injection_proxy.rs` rather than a second, independent list.
    //
    // Agent turns outrun the shared client's short default (long tool calls, multi-step
    // orchestration), so this hop carries the agent-call budget.
    let mut forwarded =
        state
            .http_client
            .request(method, &target_url)
            .timeout(std::time::Duration::from_secs(
                state.config.agent_call_timeout_secs,
            ));
    for (name, value) in headers.iter() {
        if nasiko_agent_proxy::headers::CALLER_SAFE_HEADERS.contains(&name.as_str())
            && let Ok(val_str) = value.to_str()
        {
            forwarded = forwarded.header(name.as_str(), val_str);
        }
    }

    // Whatever `forwarding_endpoint()` asked to have attached (the injection proxy's own
    // `Authorization` credential when memory is enabled; empty otherwise). This is consumed by
    // that hop, not the real agent — `injection_proxy.rs`'s own relay explicitly drops
    // `authorization` before continuing on, so it never reaches the agent container.
    for (name, value) in &forward_headers {
        forwarded = forwarded.header(name, value);
    }

    // Propagate trace context and caller identity to the agent
    forwarded = forwarded
        .header(
            nasiko_agent_proxy::headers::HEADER_TRACEPARENT,
            crate::telemetry::traceparent_for(&flow_ctx),
        )
        .header(nasiko_agent_proxy::headers::HEADER_X_USER_ID, &claims.sub)
        .header(
            nasiko_agent_proxy::headers::HEADER_X_USERNAME,
            &claims.username,
        )
        .header(
            nasiko_agent_proxy::headers::HEADER_X_IS_SUPERUSER,
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
        && !(agent.is_internal && is_ephemeral(&body_json))
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

/// Resolves the path segment to append to `agent_base` for this forward. A bare `/` (the chat
/// call, nothing after `/api/agents/{id}` in the request URI) must land on the agent's actual A2A
/// mount, which the card advertises as `transport_path` — `/a2a` for the a2a-go agents, root for
/// the a2a-server-lf ones. POSTing to `/` unconditionally 404s every agent mounted elsewhere.
/// Sub-paths (e.g. `/.well-known/agent-card.json`) are forwarded verbatim.
///
/// A root mount resolves to an EMPTY path segment, not a literal `"/"`: `agent_base` (this
/// function's caller trims it) has no trailing slash, and appending a bare `"/"` produces
/// `{agent_base}/` — fine against a real agent directly (HTTP has no request line shorter than
/// `/` anyway), but when memory is on and `agent_base` is the EE injection proxy's own address,
/// that extra slash lands on a URL neither of the proxy's routes match (its exact route carries
/// no trailing slash; its wildcard route requires a non-empty segment after it) — a 404 before
/// the request ever reaches the agent. An empty path segment concatenates to plain `agent_base`
/// either way, which is correct in both cases.
fn resolve_forwarded_path(forwarded_path: &str, transport_path: Option<&str>) -> String {
    if forwarded_path != "/" {
        return forwarded_path.to_string();
    }
    match transport_path {
        None | Some("") | Some("/") => String::new(),
        Some(p) => p.to_string(),
    }
}

/// Mark a direct-chat flow completed (fire-and-forget). `a2a_dispatch` marks
/// its own flows; the proxy path never did, so they accumulated as 'running'.
///
/// Guarded on `status = 'running'`, so a terminal outcome is never downgraded.
/// An inner call ending can otherwise complete the whole flow (FL-8), and this
/// write races the `status = 'failed'` one in `a2a_dispatch`: whichever landed
/// second won, so a recorded failure could silently become a success. Session
/// status is derived from these rows (`observability::sessions_sql`), which
/// makes the race visible as a session that reads "ok" after a failed request.
fn complete_flow(db: &sqlx::PgPool, flow_id: &str) {
    let db = db.clone();
    let flow_id = flow_id.to_string();
    tokio::spawn(async move {
        let _ = sqlx::query(
            r#"UPDATE flows SET status = 'completed',
               duration_ms = EXTRACT(EPOCH FROM (now() - created_at))::bigint * 1000,
               completed_at = now()
               WHERE flow_id = $1 AND status = 'running'"#,
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

const EPHEMERAL_METADATA_KEY: &str = "nasiko_ephemeral";

fn is_ephemeral(rpc: &serde_json::Value) -> bool {
    rpc.pointer("/params/metadata")
        .and_then(|m| m.get(EPHEMERAL_METADATA_KEY))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

async fn session_binding(
    state: &AppState,
    session_id: &str,
) -> Result<Option<(Uuid, Uuid)>, StatusCode> {
    sqlx::query_as("SELECT user_id, agent_id FROM chat_sessions WHERE session_id = $1")
        .bind(session_id)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, %session_id, "agent proxy: session lookup failed");
            StatusCode::INTERNAL_SERVER_ERROR
        })
}

/// Session and user-message info extracted during `ensure_chat_session` so the
/// proxy can persist both user and agent messages to `chat_messages` without
/// re-parsing the request body a second time.
struct PersistInfo {
    session_id: String,
    user_text: String,
    /// Whether org-wide context was prepended to the forwarded text. Carried out so the caller
    /// can record it on `a2a.proxy` — this function owns the decision, the caller owns the span.
    org_applied: bool,
    /// The rendered org-context block's length, when applied — 0 otherwise. `org_applied` alone
    /// is a boolean, so a block that grew toward the provider's own size cap would otherwise be
    /// invisible in observability.
    org_policy_chars: usize,
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
/// What `ensure_chat_session` worked out about this request: the body to forward, whether to
/// persist the turn, and what history selection carried.
///
/// A struct rather than a 3-tuple because the third element is only meaningful to the caller — it
/// owns the request span the report has to be attached to, and this function does not.
struct PreparedRequest {
    body: axum::body::Bytes,
    persist: Option<PersistInfo>,
    context_report: Option<nasiko_orchestrator::ContextReport>,
}

impl PreparedRequest {
    /// Not an A2A send, so nothing was selected and nothing is persisted.
    fn passthrough(body: axum::body::Bytes) -> Self {
        Self {
            body,
            persist: None,
            context_report: None,
        }
    }
}

async fn ensure_chat_session(
    state: &AppState,
    claims: &Claims,
    agent_id: Uuid,
    _agent_base_url: &str,
    body_bytes: axum::body::Bytes,
    internal_agent: bool,
    stitch_history: bool,
) -> Result<PreparedRequest, StatusCode> {
    // Not JSON, or not a message send → pass through; the agent is the
    // authority on whether the payload is valid A2A.
    let Ok(mut rpc) = serde_json::from_slice::<serde_json::Value>(&body_bytes) else {
        return Ok(PreparedRequest::passthrough(body_bytes));
    };
    // Owned, not borrowed: `rpc.pointer_mut(...)` below needs a mutable borrow, and this platform's
    // real agents (a2a-lf-based) reject an unexpected `"kind"` field on a part — see
    // `nasiko_agent_proxy::context_part`'s module doc — so the method name has to survive to the
    // point org policy is actually appended, not just this early check.
    let method = rpc
        .get("method")
        .and_then(|m| m.as_str())
        .unwrap_or("")
        .to_string();
    if !MESSAGE_SEND_METHODS.contains(&method.as_str()) {
        return Ok(PreparedRequest::passthrough(body_bytes));
    }
    let tagging = nasiko_agent_proxy::context_part::tagging_for_method(&method);
    let user_id: Uuid = claims.sub.parse().map_err(|_| StatusCode::UNAUTHORIZED)?;
    if internal_agent && is_ephemeral(&rpc) {
        let context_id = rpc
            .pointer("/params/message/contextId")
            .and_then(serde_json::Value::as_str)
            .filter(|s| !s.is_empty());
        if let Some(context_id) = context_id
            && let Some((owner, _)) = session_binding(state, context_id).await?
            && !claims.is_superuser
            && owner != user_id
        {
            return Err(StatusCode::FORBIDDEN);
        }
        return Ok(PreparedRequest::passthrough(body_bytes));
    }
    let Some(message) = rpc.pointer_mut("/params/message") else {
        return Ok(PreparedRequest::passthrough(body_bytes));
    };

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
        match session_binding(state, &session_id).await? {
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

    let mut persist_info = PersistInfo {
        session_id: session_id.clone(),
        user_text: user_text.clone(),
        org_applied: false,
        org_policy_chars: 0,
    };

    // Multi-turn continuity: unlike `a2a_dispatch.rs`'s orchestrator path, this
    // proxy forwards the caller's message byte-for-byte, so an agent that (like
    // every example agent) starts a fresh run per call and ignores `contextId`
    // has zero memory of prior turns — even though those turns are sitting right
    // here in `chat_messages` under this same session_id. Stitch them into the
    // outgoing text the same way `a2a_dispatch.rs` already does for the
    // routing/orchestrator path, so direct `--agent`/`-u` chat gets the
    // continuity its own "resume with --session-id" hint implies.
    let mut rewrite_needed = injected;

    // Organization policy rides on top of whatever is actually going out, history or not, and is
    // resolved OUTSIDE the text handling below rather than within it. Two conditions it must not
    // inherit:
    //
    // * `same_agent_session`, which gates the history glue — that is a continuity aid it would be
    //   wrong to stitch across agents, whereas policy applies to every agent invocation.
    // * a non-empty `user_text`. A message can legitimately carry no text at all (a file-only
    //   send through `/api/agents/{id}`), and that is still an agent invocation: the agent reads
    //   the file and answers, and a rule about what it may say back governs that turn exactly as
    //   much as any other. Nesting this under the text check let precisely those requests through
    //   unpoliced, and reported `org_applied=false` while every other path reported true.
    //
    // One case is deliberately excluded: a message carrying a `taskId` is a CONTINUATION of a
    // task the agent is already holding — a human answering the question it paused on. That task
    // was opened by a message which did carry the policy, and the agent must be stateful to have
    // paused at all, so re-sending it buys nothing; meanwhile the payload here is the human's
    // literal answer ("yes", "owner/repo"), and prepending a block to that risks the agent
    // reading the block as the answer. The two server-side resume dispatchers
    // (`hitl/mod.rs::deliver` and the MAF executor's `continue_paused_step`) already behave this
    // way; without this the proxy was the odd one out, so the same deployment policed a resume or
    // not depending on which route delivered it.
    let is_task_continuation = message
        .get("taskId")
        .and_then(|v| v.as_str())
        .is_some_and(|t| !t.is_empty());
    let org_context = if is_task_continuation {
        None
    } else {
        state.prompt_context.org_context().await
    };

    let mut context_report = None;
    if stitch_history && !user_text.is_empty() && same_agent_session {
        // This agent receives the history, so its switch decides IP-4; unreadable reads as off.
        // Via the shared helper rather than an inline query: that one also excludes soft-deleted
        // agents, which an inline `WHERE id = $1` does not.
        let compress_history =
            nasiko_orchestrator::agent_compression_enabled(&state.db, agent_id).await;
        let history_store = state.history_vector_store();
        // A direct chat: this agent's own switch decides history compression.
        let history = context_selection::fetch_for_user(
            &state.db,
            user_id,
            &session_id,
            &history_store,
            &user_text,
            &ContextTiers::from_config(&state.config)
                .with_overrides(&state.db)
                .await,
            compress_history,
        )
        .await;
        context_report = Some(history.report.clone());
        // History glue, independent of org policy below: only touch the text part when the glue
        // actually changed it, so a deployment with no history re-serializes nothing — the
        // pre-existing behaviour, byte for byte.
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

    // Organization policy, appended as its own part rather than spliced into the text part above:
    // that part may carry the history glue's `"Current message: "` marker, and any agent that
    // follows the platform's own documented convention for extracting just the current turn
    // (`oss/llm-router::latest_user_query`) silently discards whatever was prepended ahead of that
    // marker inside the same part. See `nasiko_agent_proxy::context_part`'s module doc.
    // `org_context()` already returns a complete, self-headed block ("## Organization
    // Policy\n\n...") — no extra heading to add here, and this covers the file-only case (empty
    // `user_text`) too: there is nothing to find-and-replace, only a new part to append.
    if let Some(org) = org_context.as_deref()
        && nasiko_agent_proxy::context_part::append_context_part(message, org, tagging).is_some()
    {
        rewrite_needed = true;
        // Set only once the append actually happened, so the span cannot claim policy was applied
        // to a message that had no `parts` array to append to.
        persist_info.org_applied = true;
        persist_info.org_policy_chars = org.len();
    }

    if injected {
        message["contextId"] = serde_json::Value::String(session_id);
    }
    if rewrite_needed {
        let rewritten = serde_json::to_vec(&rpc).map_err(|e| {
            tracing::error!(error = %e, "agent proxy: failed to re-serialize body after contextId/history injection");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
        return Ok(PreparedRequest {
            body: rewritten.into(),
            persist: Some(persist_info),
            context_report,
        });
    }
    Ok(PreparedRequest {
        body: body_bytes,
        persist: Some(persist_info),
        context_report,
    })
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
    use super::{SseReplyText, resolve_forwarded_path};

    #[test]
    fn root_mounted_agent_resolves_to_an_empty_segment_not_a_literal_slash() {
        // The regression this guards: `agent_base` (trimmed of its trailing slash by this
        // function's caller) + a literal "/" produces a URL the EE injection proxy's own routes
        // don't match once memory is on, 404ing before the request reaches the agent. An empty
        // segment concatenates to plain `agent_base`, correct against both the injection proxy
        // and a real agent directly.
        assert_eq!(resolve_forwarded_path("/", None), "");
        assert_eq!(resolve_forwarded_path("/", Some("")), "");
        assert_eq!(resolve_forwarded_path("/", Some("/")), "");
    }

    #[test]
    fn non_root_mounted_agent_resolves_to_its_transport_path() {
        assert_eq!(resolve_forwarded_path("/", Some("/a2a")), "/a2a");
    }

    #[test]
    fn sub_paths_are_forwarded_verbatim_regardless_of_transport_path() {
        assert_eq!(
            resolve_forwarded_path("/.well-known/agent-card.json", None),
            "/.well-known/agent-card.json"
        );
        assert_eq!(
            resolve_forwarded_path("/.well-known/agent-card.json", Some("/a2a")),
            "/.well-known/agent-card.json"
        );
    }

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

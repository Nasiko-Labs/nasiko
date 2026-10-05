//! HITL resume dispatcher. Delivers a human's
//! answer back onto the same A2A `taskId`/`contextId` the agent paused on. Shape mirrors
//! `agents/build_worker.rs`: poll/notify, atomically claim one row, execute in a panic-isolated
//! spawned task.

pub mod continuation;

use std::time::Duration;

use futures::StreamExt;
use nasiko_flow::FlowContext;
use nasiko_hitl::{
    AUTH_OUTCOME_CONFIRMED, AUTH_OUTCOME_DENIED, AUTH_REPLY_AUTHORIZED, FailureKind, HitlOrigin,
    HitlRequest, NewHitlRequest,
};
use nasiko_types::a2a::StreamDisposition;
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::hitl::continuation::ContinuationGuard;
use crate::router::a2a_dispatch::{
    OrchestratorTurn, build_hitl_stream_data, build_pause_question, normalize_agent_event,
    orchestrator_stream, pause_kind, resolve_endpoint,
};
use crate::state::AppState;

/// Delivery attempts before a resume gives up and `resume_status` becomes the terminal `failed`
/// (§3.2) — a pre-flight failure (connection refused, DNS, timeout) releases the lease and
/// retries via the next poll until this cap is hit.
const MAX_RESUME_ATTEMPTS: i32 = 5;
/// Shorthand for every retryable `mark_resume_failed` call site below — see
/// [`FailureKind`]'s own doc comment for why this replaced a bare `MAX_RESUME_ATTEMPTS` argument.
const RETRYABLE: FailureKind = FailureKind::Retryable {
    max_attempts: MAX_RESUME_ATTEMPTS,
};
/// `deliver()`'s own agent request timeout — see `build_req`'s doc comment below. The claim
/// lease (`lease_secs`) must always exceed this by a safety margin, or a slow-but-healthy agent
/// turn lets a second replica steal the lease mid-delivery and re-send the human's answer a
/// second time, double-executing whatever the agent does with it.
const AGENT_RESUME_REQUEST_TIMEOUT_SECS: i64 = 300;
/// How long a claim is honored before another dispatcher process may steal it (§3.2's exact
/// claim query, implemented in `HitlStore::claim_for_resume`). Sourced from the same
/// `HITL_RESUME_LEASE_MINUTES` knob the sibling `mcp_tool` dispatcher uses
/// (`nasiko_hitl::dispatcher::DispatcherConfig::effective_lease_minutes`) — this dispatcher
/// claims exactly once per delivery attempt (unlike that one, which holds a single claim across
/// its own in-process retry loop), so the floor here only needs to clear one request's timeout
/// plus margin, not the sum of every retry.
fn lease_secs(config: &nasiko_config::Config) -> i64 {
    (config.hitl_resume_lease_minutes * 60).max(AGENT_RESUME_REQUEST_TIMEOUT_SECS + 60)
}
/// Concurrent in-flight deliveries, mirroring `build_worker::run`'s own `tasks` cap on the same
/// claim/spawn shape. `deliver()` can drive an entire ReAct turn for an `orchestrator`-origin row
/// (tens of seconds), so awaiting each claimed row before claiming the next — as the drain loop
/// used to — let one user's slow resume block every other pending HITL answer, and delayed the
/// `expire_stale` sweep in the same loop iteration.
const MAX_CONCURRENT_DELIVERIES: usize = 8;

/// RAII close for the `flows` row `deliver()` opens for its own resume `flow_ctx` — same shape as
/// `ContinuationGuard` above: created right after the row is inserted, so every exit path out of
/// `deliver()` closes it, not just the ones an author remembered to. `record_resume_trail` closes
/// the row with the precise disposition-derived status on the normal path; this guard's `Drop` is
/// only ever a backstop for an early return that never reaches that point (a pre-flight HTTP
/// failure, a non-2xx response, an unclassifiable reply) — the `WHERE status = 'running'` makes it
/// a no-op once `record_resume_trail` has already closed the row. Without this, `flows.status =
/// 'running'` (plus its `flow_participants` row) is exactly what authorizes the resumed agent to
/// call `/api/mcp` with this traceparent (`mcp/handlers/gateway.rs`) — an early return used to
/// leave that window open indefinitely.
struct ResumeFlowCloser {
    db: sqlx::PgPool,
    flow_id: String,
}

impl Drop for ResumeFlowCloser {
    fn drop(&mut self) {
        let db = self.db.clone();
        let flow_id = std::mem::take(&mut self.flow_id);
        tokio::spawn(async move {
            let _ = sqlx::query(
                r#"UPDATE flows SET status = 'failed',
                   duration_ms = EXTRACT(EPOCH FROM (now() - created_at))::bigint * 1000,
                   completed_at = now()
                   WHERE flow_id = $1 AND status = 'running'"#,
            )
            .bind(&flow_id)
            .execute(&db)
            .await;
        });
    }
}

/// Spawned once at server startup (`state.rs::from_config_with_db`), same as the build worker.
pub async fn run(state: AppState, mut notify: mpsc::Receiver<()>) {
    tracing::info!("hitl dispatcher: started");
    // Tracks in-flight `deliver()` calls across poll cycles so a slow delivery never blocks
    // claiming (or delivering) everything else — see `MAX_CONCURRENT_DELIVERIES`'s doc comment.
    let mut deliveries: tokio::task::JoinSet<()> = tokio::task::JoinSet::new();
    let lease_secs = lease_secs(&state.config);
    loop {
        tokio::select! {
            msg = notify.recv() => {
                if msg.is_none() {
                    // Sender was dropped — server is shutting down.
                    tracing::info!("hitl dispatcher: notification channel closed, exiting");
                    return;
                }
            }
            _ = tokio::time::sleep(Duration::from_secs(2)) => {}
        }

        match state.hitl_store.expire_stale().await {
            Ok(0) => {}
            Ok(n) => tracing::info!(count = n, "hitl dispatcher: expired stale pending rows"),
            Err(e) => tracing::error!(%e, "hitl dispatcher: expire_stale error"),
        }

        // Drain: keep claiming while a delivery slot is free and the queue has a claimable row,
        // same pattern as `build_worker::run`. Claim runs here in the worker loop (minimal, no
        // panic risk); delivery runs in a spawned task tracked by `deliveries`, concurrently with
        // every other in-flight one, so a panicking or merely slow delivery can't take the
        // dispatcher down or stall the rest of the queue.
        while deliveries.len() < MAX_CONCURRENT_DELIVERIES {
            let claimed = match state.hitl_store.claim_for_resume(lease_secs).await {
                Ok(Some(row)) => row,
                Ok(None) => break,
                Err(e) => {
                    tracing::error!(%e, "hitl dispatcher: claim error");
                    break;
                }
            };

            let state_clone = state.clone();
            deliveries.spawn(async move { deliver(state_clone, claimed).await });
        }

        // Reap whatever has finished without blocking this tick — a still-running delivery is
        // simply left in `deliveries` and picked up on a later iteration. The row's lease is
        // still held on a panic; §3.2's MVP-scope note: a crash mid-window sticks visibly until
        // Phase 9's recovery sweep ships — not silently lost.
        while let Some(result) = deliveries.try_join_next() {
            if let Err(e) = result
                && e.is_panic()
            {
                tracing::error!("hitl dispatcher: delivery task panicked");
            }
        }
    }
}

async fn deliver(state: AppState, row: HitlRequest) {
    // Ties the continuation buffer's lifetime to this whole call frame, from the very first line —
    // every return path below, including the early-return guards ahead of the task_id/context_id
    // check, marks it terminal on drop (see `ContinuationGuard`'s own doc comment). A reconnect
    // that finds a buffer for `row.id` still non-terminal, with none of these early returns having
    // fired, cannot yet distinguish "still in flight" from "never dispatched at all" purely from
    // the buffer's own existence — `reconnect_stream` rejects a still-`pending` row before ever
    // calling `watch()`, so that ambiguity never actually reaches a client.
    let continuation = ContinuationGuard::new(state.continuation_events.clone(), row.id);

    // If this row mirrors a real `mcp_tool` pause (`question.metadata.hitl_request_id` — set
    // when an agent maps an MCP-gateway-detected auth_required/tool_approval onto its own A2A
    // pause, e.g. a `create_issue_via_connector`-style escalation), alias that id onto this
    // row's own buffer too. A reconnecting client uses whichever id `resolve_display_row`
    // showed it — the *real* mcp_tool row's id — but `deliver()` only ever runs on *this* row;
    // without this, that reconnect finds no buffer at all. Doing it here, unconditionally,
    // covers both ways the mcp_tool row can get resolved: the manual `/resolve` endpoint
    // (`router/hitl.rs::auto_resolve_linked_direct_chat_row` already aliases there too — a
    // harmless redundant alias in that case, just earlier) and Nasiko's own OAuth-callback
    // auto-resolve (`oss/hitl/src/repo.rs::resolve_linked_direct_chat_mirror`), which runs
    // inside `nasiko-mcp-gateway` with no access to `continuation_events` at all and could
    // never alias anything itself — that path previously left this row correctly resolved
    // (the agent really does resume) but permanently unreconnectable.
    if let Some(mcp_row_id) = row
        .question
        .pointer("/metadata/hitl_request_id")
        .and_then(|v| v.as_str())
        .and_then(|s| Uuid::parse_str(s).ok())
    {
        // Same guard `resolve_display_row` (`oss/hitl/src/store.rs`) applies before trusting this
        // same agent-controlled pointer — `row.question` is an untrusted A2A response echoed
        // straight from the agent, so without this an agent can stamp any UUID here and alias
        // another user's `hitl_requests.id` onto this row's buffer, letting that user's own
        // reconnect read this agent's output (the #383 mirror-hijack family).
        let linked = state.hitl_store.get(mcp_row_id).await;
        if matches!(
            linked,
            Ok(Some(ref mirror)) if nasiko_hitl::is_valid_mcp_mirror_link(mirror, row.owner_user_id, row.agent_id)
        ) {
            state.continuation_events.alias(mcp_row_id, row.id);
        }
    }

    // `claim_for_resume` has no attempts cap of its own — a row only reaches this many attempts
    // by surviving past every prior attempt's own cap check without a clean completed/failed
    // outcome, i.e. the dispatcher process itself crashed mid-delivery on each one. A clean
    // HTTP/parse failure already self-terminates via `mark_resume_failed`'s cap check, so this
    // guard only ever fires for that crash case — surface it distinctly rather than retrying
    // forever. Strictly `>`, not `>=`: `claim_for_resume` already incremented
    // `resume_dispatch_attempts` for *this* claim, so `>= MAX_RESUME_ATTEMPTS` would bail out
    // on the final legitimate attempt before ever trying it. An ordinary (non-crash) failure on
    // that final attempt still self-terminates via `mark_resume_failed`'s own `>=` cap check,
    // which flips `resume_status` to `failed` and makes the row unclaimable — so attempts can
    // only exceed the cap here via the crash path this guard exists for.
    if row.resume_dispatch_attempts > MAX_RESUME_ATTEMPTS {
        tracing::warn!(
            id = %row.id,
            attempts = row.resume_dispatch_attempts,
            "hitl dispatcher: giving up after repeated crash-interrupted attempts"
        );
        if let Err(e) = state.hitl_store.mark_resume_unknown(row.id).await {
            tracing::error!(
                id = %row.id, %e,
                "hitl dispatcher: mark_resume_unknown itself failed — row stays reclaimable \
                 once its lease expires, reopening the double-delivery this call exists to prevent"
            );
        }
        return;
    }
    if row.origin == HitlOrigin::Maf {
        return deliver_maf(&state, row).await;
    }
    if !matches!(
        row.origin,
        HitlOrigin::DirectChat | HitlOrigin::AgentProxy | HitlOrigin::Orchestrator
    ) {
        // Unreachable today — NOT because nothing creates `mcp_tool` rows (it does:
        // `repo::create_pending_tool_approval`/`create_pending_auth_required`), but because this
        // dispatcher's own `claim_for_resume` (`store.rs`) is scoped to exclude `origin =
        // 'mcp_tool'` in the first place — `oss/hitl`'s own separate dispatcher claims those
        // instead. Defensive, not a real path; stays correct only as long as that query's origin
        // list is never widened to include `mcp_tool` without updating this arm too.
        tracing::warn!(id = %row.id, origin = ?row.origin, "hitl dispatcher: unsupported origin");
        let _ = state
            .hitl_store
            .mark_resume_failed(
                row.id,
                "origin not yet supported by the resume dispatcher",
                FailureKind::Permanent,
            )
            .await;
        return;
    }
    let (Some(task_id), Some(context_id)) = (row.task_id.clone(), row.context_id.clone()) else {
        let _ = state
            .hitl_store
            .mark_resume_failed(
                row.id,
                "row is missing task_id/context_id",
                FailureKind::Permanent,
            )
            .await;
        return;
    };

    // `Err` (a transient DB blip) and `Ok(None)` (the agent row is genuinely gone) are distinct
    // outcomes — collapsing them via `.ok().flatten()` used to record "agent no longer exists" for
    // a momentary connection failure too, a false and undiagnosable reason for what's really a
    // retryable transport problem.
    let agent_name = match sqlx::query_scalar::<_, String>("SELECT name FROM agents WHERE id = $1")
        .bind(row.agent_id)
        .fetch_optional(&state.db)
        .await
    {
        Ok(name) => name,
        Err(e) => {
            let _ = state
                .hitl_store
                .mark_resume_failed(
                    row.id,
                    &format!("database error resolving agent name: {e}"),
                    RETRYABLE,
                )
                .await;
            return;
        }
    };
    let Some(agent_name) = agent_name else {
        let _ = state
            .hitl_store
            .mark_resume_failed(row.id, "agent no longer exists", RETRYABLE)
            .await;
        return;
    };

    let endpoint = match resolve_endpoint(&state, &row.agent_id.to_string(), &agent_name).await {
        Ok(e) => e,
        Err(e) => {
            let _ = state
                .hitl_store
                .mark_resume_failed(row.id, &e, RETRYABLE)
                .await;
            return;
        }
    };

    // Reuse the original flow if it was stashed in `resume_state` at pause time — the
    // agent's OTel auto-instrumentation already carries this trace_id on every outbound
    // call, so reopening the same flow means the agent's MCP and LLM-router calls pass
    // the `traceparent → live flow` check with zero agent-side changes. Falls back to a
    // fresh root flow for rows created before this stash was added.
    let flow_ctx = row
        .resume_state
        .get("flow_id")
        .and_then(|v| v.as_str())
        .map(|fid| FlowContext {
            flow_id: fid.to_string(),
            parent_span_id: FlowContext::generate_span_id(),
        })
        .unwrap_or_else(FlowContext::new_root);
    let agent_id_str = row.agent_id.to_string();
    state.flow_guard.init_flow(&flow_ctx, &agent_name).await;
    if let Err(rejection) = state.flow_guard.check(&flow_ctx, &agent_id_str).await {
        let _ = state
            .hitl_store
            .mark_resume_failed(
                row.id,
                &format!("flow guard rejected resume: {rejection}"),
                RETRYABLE,
            )
            .await;
        return;
    }
    if let Err(rejection) = state
        .flow_guard
        .record_invocation(&flow_ctx, &agent_id_str)
        .await
    {
        let _ = state
            .hitl_store
            .mark_resume_failed(
                row.id,
                &format!("flow guard rejected resume: {rejection}"),
                RETRYABLE,
            )
            .await;
        return;
    }

    // Register this resume as a live flow in the MCP-gateway sense (Postgres `flows` +
    // `flow_participants`) — a completely separate bookkeeping system from the FlowGuard
    // cascade-limit check just above, which never touches these tables. Without this, the
    // resumed agent's own MCP tool calls (e.g. retrying the exact call a human just approved)
    // 403 with "traceparent does not resolve to a live flow": `flow_ctx` is a brand-new root
    // flow that no dispatch path had ever registered here (confirmed live — HITL_PR342-style
    // regression). Mirrors every other dispatch site's flows insert (`agent_proxy.rs`,
    // `a2a_dispatch.rs`, `maf/executor.rs::register_flow`).
    //
    // `metadata.context_id` carries `row.context_id` (the agent's OWN, private A2A context —
    // never a `chat_sessions` row) purely for observability/debugging, mirroring
    // `agent_proxy.rs`'s convention of stamping the sticky key onto `flows.metadata`. It is NOT
    // what makes retry-matching work — that's `session_traces` below, and `session_traces.session_id`
    // has a hard FK to `chat_sessions(session_id)` (`0004_observability.sql`), which `context_id`
    // can never satisfy for an `Orchestrator`-origin row: there, `context_id` is the sub-agent's
    // own per-dispatch A2A context, minted fresh by `trigger_new_orchestrator_turn` on every
    // resume — never a real chat session. `nasiko_mcp_gateway::session::resolve_context_id`
    // resolves a tools/call's session by looking up `session_traces` for the CALLING trace_id,
    // falling back to the trace_id itself only when no row exists. Every resume mints a brand-new
    // `flow_ctx.flow_id`, so without a session_traces row mapping it to something STABLE,
    // `resolve_tool_approval_retry`'s once/session-scope grant lookup keys on a trace_id that's
    // different on every single resume, never matching the original ask's own resolved context —
    // every resumed retry of a tool the human just approved gets asked again, forever (confirmed
    // live: approving the same tool_approval repeatedly, every retry still comes back
    // `ask_required`, and the earlier `session_id = context_id` version of this INSERT was
    // silently failing its FK check on every single orchestrator resume).
    //
    // `row.chat_session_id` is the real, existing `chat_sessions` row every `Orchestrator`-origin
    // mirror in this chain carries forward (`NewHitlRequest::orchestrator`/
    // `persist_direct_chat_pause`) — mapping that resume's fresh flow_id to THAT is what lets
    // retries resolve to the SAME session the original ask did. `AgentProxy`/`DirectChat` rows
    // carry no separate `chat_session_id` at all: for those there's no distinct
    // orchestrator-level session, so `context_id` already IS the stable id — confirmed live the
    // hard way: an earlier version of this fix *skipped* the insert below whenever
    // `chat_session_id` was absent (reasoning "no stable id to map to" for those origins), and a
    // multi-round `agent_proxy` approval immediately showed why that's wrong: round 1's
    // `context_id` was a real `ses_...` `chat_sessions` row, but with no session_traces row
    // written for it, round 2's `resolve_context_id` couldn't find round 1's trace and fell back
    // to a raw, unstable trace id instead — which is what round 2's own `context_id` then carried
    // forward, drifting further from the real session on every subsequent round. Falling back to
    // `context_id` here (rather than skipping the insert) is what keeps `AgentProxy`/`DirectChat`
    // pinned to the same real session on every resume, exactly as it did before this fix existed.
    let stable_session_id = stable_session_id(&row).unwrap_or(&context_id);
    if let Err(e) = sqlx::query(
        r#"INSERT INTO flows (flow_id, user_id, root_agent_id, root_agent_name, title, status, metadata)
           VALUES ($1, $2, $3, $4, $5, 'running', $6)
           ON CONFLICT (flow_id) DO UPDATE
              SET status = 'running', completed_at = NULL"#,
    )
    .bind(&flow_ctx.flow_id)
    .bind(row.owner_user_id)
    .bind(row.agent_id)
    .bind(&agent_name)
    .bind("HITL resume")
    .bind(serde_json::json!({ "context_id": context_id }))
    .execute(&state.db)
    .await
    {
        tracing::warn!(
            error = %e, id = %row.id, flow_id = %flow_ctx.flow_id,
            "hitl resume: failed to register the resume flow — the agent's own MCP calls during \
             this resume may 403"
        );
    }
    // See `ResumeFlowCloser`'s own doc comment — must be created right after the insert above so
    // every return between here and the end of this function closes the row it just opened.
    let _flow_closer = ResumeFlowCloser {
        db: state.db.clone(),
        flow_id: flow_ctx.flow_id.clone(),
    };
    crate::flows::record_participant(&state.db, &flow_ctx.flow_id, row.agent_id).await;
    if let Err(e) = sqlx::query(
        "INSERT INTO session_traces (session_id, trace_id, agent_id, agent_name)
         VALUES ($1, $2, $3, $4)
         ON CONFLICT (session_id, trace_id) DO NOTHING",
    )
    .bind(stable_session_id)
    .bind(&flow_ctx.flow_id)
    .bind(row.agent_id)
    .bind(&agent_name)
    .execute(&state.db)
    .await
    {
        tracing::warn!(
            error = %e, %stable_session_id, flow_id = %flow_ctx.flow_id,
            "hitl resume: session_traces record failed — tool-approval retry matching for this resume may re-ask"
        );
    }

    let answer = answer_text(&row);
    let req_body = nasiko_types::a2a::build_stream_request_for_task(&answer, &context_id, &task_id);

    // Reused for the initial send and (non-streaming path only) the one-shot `message/send`
    // retry — `AGENT_RESUME_REQUEST_TIMEOUT_SECS`, not the shared client's 60s default: agent
    // turns can be slow (mirrors `maf/executor.rs::post_a2a_request`, the other call site that
    // talks to an agent on a human's behalf).
    //
    // No per-request MCP credential is forwarded: the agent authenticates to `/api/mcp` with its
    // own deploy-time MCP_GATEWAY_TOKEN (docs/MCP_GATEWAY_AGENT_AUTH.md).
    //
    // Forwards `traceparent` from `flow_ctx` — every other inter-agent call site
    // (`agent_proxy.rs`, `a2a_dispatch.rs`) does the same. Without it, an agent whose own
    // downstream tool calls key retry-approval matching off the request's trace id (MCP's
    // `resolve_tool_approval_retry`, keyed by `traceparent`'s trace id) never sees the resumed
    // call as a continuation of the original one that was just approved — every retry looks
    // like a brand new, never-before-seen call, so a human approving a tool call for the
    // orchestrator's sub-agent sees the exact same tool-approval prompt again on the very next
    // step, forever.
    let build_req = |body: &nasiko_types::a2a::JsonRpcRequest| {
        state
            .http_client
            .post(&endpoint)
            .timeout(Duration::from_secs(
                AGENT_RESUME_REQUEST_TIMEOUT_SECS as u64,
            ))
            .header("A2A-Version", nasiko_types::a2a::A2A_VERSION_HEADER_VALUE)
            .header("traceparent", crate::telemetry::traceparent_for(&flow_ctx))
            .json(body)
    };

    let response = match build_req(&req_body).send().await {
        Ok(r) => r,
        Err(e) => {
            // Pre-flight failure — never left Nasiko. Retried via the lease under the cap.
            let _ = state
                .hitl_store
                .mark_resume_failed(row.id, &format!("agent request failed: {e}"), RETRYABLE)
                .await;
            return;
        }
    };
    // Matches `agent_proxy.rs`'s own placement: one invocation in, one return out, regardless
    // of the agent's own business outcome (paused/failed/completed) below.
    state.flow_guard.record_return(&flow_ctx).await;
    if !response.status().is_success() {
        let status = response.status();
        let _ = state
            .hitl_store
            .mark_resume_failed(row.id, &format!("agent HTTP {status}"), RETRYABLE)
            .await;
        return;
    }

    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();

    let outcome = if content_type.contains("text/event-stream") {
        consume_sse_to_terminal(response, &task_id, &context_id, &continuation).await
    } else {
        consume_json_to_terminal(
            response,
            &context_id,
            &task_id,
            &answer,
            &build_req,
            &continuation,
        )
        .await
    };

    let Some((disposition, last_data, reply_text)) = outcome else {
        // By this point the HTTP response was already received (`response.status().is_success()`
        // above already passed) — the agent has the resumed message and may already have acted on
        // it before the transport broke mid-body (`consume_sse_to_terminal`'s `chunk.ok()?`) or
        // the reply failed to parse (`consume_json_to_terminal`'s `response.json().await.ok()?`).
        // Unlike the true pre-flight failures above (`build_req(...).send()` erroring, or a
        // non-success status — neither ever got this far), retrying here would re-POST the human's
        // answer to an agent that may have already consumed and acted on it once, double-executing
        // it. Same reasoning as `mark_resume_completed`'s own failure handling just below:
        // `mark_resume_unknown` takes the row out of the claimable pool instead of leaving it
        // reclaimable once the lease expires.
        tracing::error!(
            id = %row.id,
            "hitl dispatcher: agent response could not be classified after the request was \
             already sent — marking delivery outcome unknown rather than retrying, to avoid \
             re-delivering to the agent"
        );
        if let Err(e) = state.hitl_store.mark_resume_unknown(row.id).await {
            tracing::error!(
                id = %row.id, %e,
                "hitl dispatcher: mark_resume_unknown itself failed — row stays reclaimable \
                 once its lease expires, reopening the double-delivery this call exists to prevent"
            );
        }
        return;
    };

    // Delivery succeeded — a response was received and classified — regardless of the agent's
    // own business outcome (§3.2: "peer confirmed receipt").
    if let Err(e) = state.hitl_store.mark_resume_completed(row.id).await {
        // The agent already received and answered this resume — retrying would POST the human's
        // answer to it a second time, same double-delivery risk `deliver_maf` guards against
        // above. `mark_resume_unknown` takes the row out of the claimable pool instead of
        // leaving it reclaimable once the lease expires.
        tracing::error!(
            id = %row.id, %e,
            "hitl dispatcher: resume delivered but mark_resume_completed failed — \
             marking delivery outcome unknown to avoid re-delivering to the agent"
        );
        if let Err(e) = state.hitl_store.mark_resume_unknown(row.id).await {
            tracing::error!(
                id = %row.id, %e,
                "hitl dispatcher: mark_resume_unknown itself failed — row stays reclaimable \
                 once its lease expires, reopening the double-delivery this call exists to prevent"
            );
        }
    }
    record_resume_trail(&state, disposition, &flow_ctx).await;
    // Unconditional on `disposition`: the agent received the answer, which is what ends the pause.
    // A follow-up pause gets its own row and its own step in its own flow.
    close_resumed_flow_step(&state, &row, &agent_name).await;

    if disposition != StreamDisposition::Paused {
        if row.origin == HitlOrigin::Orchestrator {
            // Unlike direct_chat/agent_proxy, the sub-agent's reply isn't the final answer here —
            // it needs to go back through the ReAct loop for the LLM to reason over, not be shown
            // to the user directly.
            trigger_new_orchestrator_turn(&state, &row, &agent_name, reply_text).await;
        } else {
            // A follow-up pause (below) gets its own row and its own future resolution instead —
            // nothing final to show in chat history yet.
            persist_resume_reply(&state, &row, &context_id, reply_text).await;
        }
    }

    if disposition == StreamDisposition::Paused {
        // Sequential HITL (§3.5): the resumed task paused again. New row, same task/context.
        // `last_data` is structurally guaranteed `Some` here — both `consume_sse_to_terminal` and
        // `consume_json_to_terminal` only ever return a `Paused` disposition alongside the exact
        // payload that produced it. Defaulting a genuinely missing payload to `{}` used to build a
        // contentless approval card — an empty prompt shown to the human with a defaulted kind
        // (found in review); skipping the follow-up pause and logging is strictly better than
        // asking a human an empty question.
        let Some(data) = last_data.as_deref() else {
            tracing::error!(
                id = %row.id,
                "hitl dispatcher: Paused disposition with no last_data — this should be \
                 impossible by construction; skipping the follow-up pause rather than showing the \
                 human an empty question"
            );
            return;
        };
        let question = build_pause_question(data);
        let kind = pause_kind(data);
        let new_row = match row.origin {
            HitlOrigin::AgentProxy => Some(NewHitlRequest::agent_proxy(
                kind,
                row.agent_id,
                row.owner_user_id,
                task_id,
                context_id,
                question,
            )),
            HitlOrigin::Orchestrator => match row.chat_session_id.clone() {
                Some(chat_session_id) => Some(NewHitlRequest::orchestrator(
                    kind,
                    row.agent_id,
                    row.owner_user_id,
                    task_id,
                    context_id,
                    chat_session_id,
                    question,
                )),
                // Guaranteed `Some` by construction (`NewHitlRequest::orchestrator` always sets
                // it) — but converting that impossibility into `unwrap_or_default()`'s `""` used
                // to guarantee a *different*, real failure: `hitl_requests.chat_session_id` has an
                // FK to `chat_sessions(session_id)`, so `create` below would fail outright on
                // every single call if this branch were ever actually reached (found in review).
                // An early, logged skip is strictly better than turning an unreachable case into a
                // broken one.
                None => {
                    tracing::error!(
                        id = %row.id,
                        "hitl dispatcher: orchestrator-origin row has no chat_session_id — this \
                         should be impossible by construction; skipping the follow-up pause \
                         rather than writing a row that would violate the chat_sessions FK"
                    );
                    None
                }
            },
            // Propagate `chat_session_id` from the row that just resolved: it belongs to the
            // same web-chat session as the whole multi-turn exchange, and only the very first
            // pause in a chain has it threaded in from `a2a_dispatch.rs` — every later round in
            // the same chain is built here, not there, so this is the only place it can carry
            // forward from.
            _ => Some(
                NewHitlRequest::direct_chat(
                    kind,
                    row.agent_id,
                    row.owner_user_id,
                    task_id,
                    context_id,
                    question,
                )
                .with_chat_session_id(row.chat_session_id.clone()),
            ),
        };
        let Some(new_row) = new_row else {
            return;
        };
        match state.hitl_store.create(new_row).await {
            Ok(created) => {
                // Stash the resume's flow_id so a second resume can reopen the same
                // flow (same rationale as `persist_direct_chat_pause`).
                let _ = sqlx::query(
                    "UPDATE hitl_requests SET resume_state = resume_state || $2 WHERE id = $1",
                )
                .bind(created.id)
                .bind(serde_json::json!({ "flow_id": &flow_ctx.flow_id }))
                .execute(&state.db)
                .await;
                // The frontend discovers HITL #2 from the reconnected A2A stream itself, not by
                // polling `/messages` — same synthetic-frame shape `build_hitl_stream_event`
                // already layers onto a live turn's own SSE (§11.2), reused here via
                // `build_hitl_stream_data` rather than duplicated. Reads `task_id`/`context_id`
                // back off `created` (not the locals above, already moved into the constructor
                // call) — same values either way, the row was built from them.
                let created_task_id = created.task_id.clone().unwrap_or_default();
                let created_context_id = created.context_id.clone().unwrap_or_default();
                // Matches `a2a_dispatch.rs`'s original-pause convention: only the orchestrator
                // origin names the agent (it can delegate to several; direct_chat/agent_proxy are
                // always a conversation with the one agent already on screen, so naming it again
                // would be redundant there, not wrong — but consistency with the first pause in
                // the chain is what this is restoring).
                let agent_for_frame =
                    (row.origin == HitlOrigin::Orchestrator).then_some(agent_name.as_str());
                let data = build_hitl_stream_data(
                    &state.hitl_store,
                    &created_task_id,
                    &created_context_id,
                    &created,
                    agent_for_frame,
                )
                .await;
                continuation.push(data);
            }
            Err(e) => {
                tracing::error!(id = %row.id, %e, "hitl dispatcher: failed to persist the follow-up pause");
            }
        }
    }
}

/// MAF resume. Unlike `deliver()`'s direct-chat
/// path above, this never talks to the agent itself — it hands off to the existing MAF worker
/// (`oss/orchestrator/src/maf/worker.rs`) by re-`XADD`ing to the same Redis stream it already
/// reads, carrying a continuation marker (`resume_step_index`/`resume_task_id`/`resume_answer`).
/// The worker owns the actual agent call, LLM extraction, and continuation logic — "delivered"
/// here means "the continuation job was durably enqueued" (the Redis `XADD` was acked), the
/// stated confirmation-of-receipt for this origin.
async fn deliver_maf(state: &AppState, row: HitlRequest) {
    let (Some(task_id), Some(execution_id)) = (row.task_id.clone(), row.maf_execution_id) else {
        let _ = state
            .hitl_store
            .mark_resume_failed(
                row.id,
                "row is missing task_id/maf_execution_id",
                FailureKind::Permanent,
            )
            .await;
        return;
    };
    let Some(step_index) = row.maf_step_index else {
        let _ = state
            .hitl_store
            .mark_resume_failed(
                row.id,
                "row is missing maf_step_index",
                FailureKind::Permanent,
            )
            .await;
        return;
    };

    // The exact snapshot the original run started with — never the mutable `mafs.maf_json`,
    // which may have changed since (§2.3 #6). Persisted at `POST /maf/workflow/{id}/run` time
    // (`oss/server/src/maf.rs::run_workflow`). `Err` (transient DB blip) is kept distinct from
    // `Ok(None)` (the execution/snapshot is genuinely gone) for the same reason as `deliver()`'s
    // own agent-name lookup above — collapsing them mislabels a retryable connection failure as a
    // permanent, false reason.
    let maf_json = match sqlx::query_scalar::<_, String>(
        "SELECT maf_json::text FROM maf_executions WHERE id = $1",
    )
    .bind(execution_id)
    .fetch_optional(&state.db)
    .await
    {
        Ok(json) => json,
        Err(e) => {
            let _ = state
                .hitl_store
                .mark_resume_failed(
                    row.id,
                    &format!("database error resolving maf execution snapshot: {e}"),
                    RETRYABLE,
                )
                .await;
            return;
        }
    };
    let Some(maf_json) = maf_json else {
        let _ = state
            .hitl_store
            .mark_resume_failed(
                row.id,
                "maf execution or its snapshot no longer exists",
                RETRYABLE,
            )
            .await;
        return;
    };

    let answer = answer_text(&row);

    let mut conn = match state.redis.get_multiplexed_async_connection().await {
        Ok(c) => c,
        Err(e) => {
            let _ = state
                .hitl_store
                .mark_resume_failed(row.id, &format!("redis connection failed: {e}"), RETRYABLE)
                .await;
            return;
        }
    };

    let enqueue: redis::RedisResult<String> = redis::cmd("XADD")
        .arg(nasiko_orchestrator::maf::STREAM_KEY)
        .arg("*")
        .arg("execution_id")
        .arg(execution_id.to_string())
        .arg("maf_json")
        .arg(&maf_json)
        .arg("user_id")
        .arg(row.owner_user_id.to_string())
        .arg("resume_step_index")
        .arg(step_index.to_string())
        .arg("resume_task_id")
        .arg(&task_id)
        .arg("resume_answer")
        .arg(&answer)
        .query_async(&mut conn)
        .await;

    match enqueue {
        Ok(_) => {
            // Interim status while the continuation job is in flight — `worker.rs::process_job`
            // unconditionally sets `running` again on pickup, so this is cosmetic-but-correct,
            // mirroring `worker.rs::re_enqueue`'s own `status='pending'` convention on a
            // retryable failure. Flipped only now, after a confirmed `XADD`, not before it: doing
            // it earlier meant a Redis outage (or any other pre-enqueue failure) left the
            // execution stuck at `pending` — indistinguishable from "queued" to the API/CLI —
            // with no job ever landing in the stream to move it forward.
            let _ = sqlx::query("UPDATE maf_executions SET status = 'pending' WHERE id = $1")
                .bind(execution_id)
                .execute(&state.db)
                .await;
            if let Err(e) = state.hitl_store.mark_resume_completed(row.id).await {
                // The continuation job is already durably enqueued (XADD acked) — retrying this
                // resume would XADD a *second* one for the same `taskId`, re-running every step
                // after the resume point and overwriting whatever the first continuation already
                // produced. `mark_resume_unknown` takes the row out of the claimable pool into
                // the terminal `delivery_outcome_unknown` state instead of leaving it reclaimable
                // once the lease expires, same tool `deliver()` uses for its own
                // crash-mid-delivery case above.
                tracing::error!(
                    id = %row.id, %e,
                    "hitl dispatcher: MAF resume enqueued but mark_resume_completed failed — \
                     marking delivery outcome unknown to avoid a duplicate XADD"
                );
                if let Err(e) = state.hitl_store.mark_resume_unknown(row.id).await {
                    tracing::error!(
                        id = %row.id, %e,
                        "hitl dispatcher: mark_resume_unknown itself failed — row stays \
                         reclaimable once its lease expires, reopening the double-XADD risk \
                         this call exists to prevent"
                    );
                }
            }
        }
        Err(e) => {
            let _ = state
                .hitl_store
                .mark_resume_failed(
                    row.id,
                    &format!("failed to enqueue MAF resume job: {e}"),
                    RETRYABLE,
                )
                .await;
        }
    }
}

/// The text sent back to the agent as the human's reply. `input_required` carries `answer`
/// directly; `auth_required` has no free-text answer. A genuine external-credential `auth_required`
/// (the "reply authorized" convention) only ever reaches this
/// dispatcher via a "confirm" resolve (a "start" resolve leaves the row `pending`,
/// `router/hitl.rs::resolve`) — there's no reject path for that kind, so its `auth_outcome` is
/// always `"confirmed"`, and the literal reply must stay `"authorized"`, not a paraphrase: a
/// deterministic agent may match that reply literally rather than semantically, so the platform
/// echoes back exactly the word it told the human to send. But a `direct_chat`/`agent_proxy`/
/// `maf`/`orchestrator` mirror of an MCP `tool_approval` (also `HitlKind::AuthRequired`, per
/// `pause_kind()`'s mapping) CAN be rejected — `auto_resolve_linked_direct_chat_row`
/// (`router/hitl.rs`) writes `auth_outcome: "denied"` for that case — and reaches this same
/// dispatcher once `claim_for_resume` claims `rejected` rows too. Collapsing that into
/// `"authorized"` would tell the agent the opposite of what happened, so `"denied"` is echoed back
/// literally instead, on the same "echo the word, let the agent determine the real outcome from
/// its own next response" principle (§7's "intent ≠ success").
/// Selectable-options extension (additive to `input_required`, `router/hitl.rs::
/// resolve_structured_answer`): a multi-select `human_response.answer` is a JSON array of the
/// selected option labels, never a plain string — this branch is unreachable for any row that
/// predates the feature or whose question was never structured, since `resolve()` only ever writes
/// an array under `answer` for a `question.multi_select = true` row. Flattened as one label per
/// line rather than comma-joined, since a label itself may contain a comma (§11 of the request) —
/// a newline can't collide with option text the same way, and this keeps the agent's continuation
/// a single plain-text message, exactly like every other resume, with no new wire structure.
/// `custom_answer` (multi-select's "Something else" text) is appended as its own trailing line when
/// present, so a custom-only answer (zero predefined selections) degrades to a single-line
/// message — indistinguishable from a plain single-select or free-text answer to the agent, which
/// is a deliberate, not incidental, property: no agent has to special-case "was this multi-select."
/// The stable, `chat_sessions`-registered session identity for `row`, regardless of origin:
/// `chat_session_id` for an `Orchestrator`-origin row (the top-level session — `context_id`
/// there is the sub-agent's own unstable per-dispatch context, minted fresh on every resume, see
/// `deliver()`'s own `stable_session_id` comment above), or `context_id` itself for
/// `AgentProxy`/`DirectChat`, which have no separate orchestrator-level session and use it as
/// the stable id directly. `None` only for an origin with neither set (defensive — not expected
/// in practice for any row this is called on).
pub(crate) fn stable_session_id(row: &HitlRequest) -> Option<&str> {
    row.chat_session_id.as_deref().or(row.context_id.as_deref())
}

pub(crate) fn answer_text(row: &HitlRequest) -> String {
    let response = row.human_response.as_ref();
    if let Some(items) = response
        .and_then(|r| r.get("answer"))
        .and_then(|v| v.as_array())
    {
        let mut lines: Vec<String> = items
            .iter()
            .filter_map(|v| v.as_str())
            .map(str::to_string)
            .collect();
        if let Some(custom) = response
            .and_then(|r| r.get("custom_answer"))
            .and_then(|v| v.as_str())
        {
            lines.push(custom.to_string());
        }
        return lines.join("\n");
    }
    if let Some(answer) = response
        .and_then(|r| r.get("answer"))
        .and_then(|v| v.as_str())
    {
        return answer.to_string();
    }
    // Explicit match, not a `Some(_)` catch-all: `"confirmed"`/`"denied"` are the only two values
    // anything in this codebase ever writes (`router/hitl.rs::resolve`'s two-click confirm/cancel
    // flow, `repo::resolve_pending_auth_required_for_connector`'s bulk OAuth-reconnect path) — a
    // `Some(_)` default of "authorized" told the agent it was authorized for ANY unrecognized
    // future value, the wrong default direction for an authorization signal (found in review).
    // `None` (missing `auth_outcome` entirely) previously built an A2A request with an empty text
    // part instead of failing the resume; failing closed to "denied" here is safe either way — a
    // human never actually said "go ahead" in either case.
    match response
        .and_then(|r| r.get("auth_outcome"))
        .and_then(|v| v.as_str())
    {
        Some(AUTH_OUTCOME_CONFIRMED) => AUTH_REPLY_AUTHORIZED.to_string(),
        Some(AUTH_OUTCOME_DENIED) => AUTH_OUTCOME_DENIED.to_string(),
        Some(other) => {
            tracing::warn!(auth_outcome = %other, "answer_text: unrecognized auth_outcome value, failing closed to denied");
            AUTH_OUTCOME_DENIED.to_string()
        }
        None => {
            tracing::warn!(
                "answer_text: auth_required row has no auth_outcome, failing closed to denied"
            );
            AUTH_OUTCOME_DENIED.to_string()
        }
    }
}

/// Reads the agent's streaming reply to its first terminal or paused event — no client to relay
/// to, so unlike `agent_stream()` this only classifies, it doesn't yield SSE frames. Returns the
/// disposition, the raw `data:` payload of the event that produced it (used to extract a failure
/// message or build a follow-up pause's `question`), and the reply text accumulated across
/// *every* event seen, not just the terminal one — confirmed live that a real agent's answer
/// commonly arrives as `artifactUpdate` chunks classified `Continue`, with the terminal
/// `TASK_STATE_COMPLETED` event itself carrying no text at all; capturing only the terminal
/// event's own data would silently lose the reply for exactly that (common) case. Reuses
/// `agent_proxy.rs`'s own accumulation logic (`artifact_chunk_text`/`task_reply_text`/
/// `message_parts_text`) rather than re-deriving it. `None` only on a transport error mid-stream;
/// a stream that closes with no explicit terminal event is treated as `Completed`, mirroring
/// `agent_stream`'s own "stream closed" == done assumption for well-behaved agents.
async fn consume_sse_to_terminal(
    response: reqwest::Response,
    task_id: &str,
    context_id: &str,
    continuation: &ContinuationGuard,
) -> Option<(StreamDisposition, Option<String>, Option<String>)> {
    let mut byte_stream = response.bytes_stream();
    let mut buffer = String::new();
    let mut last_data: Option<String> = None;
    let mut artifact_text = String::new();
    let mut terminal_text: Option<String> = None;

    while let Some(chunk) = byte_stream.next().await {
        let chunk = chunk.ok()?;
        buffer.push_str(&String::from_utf8_lossy(&chunk));

        while let Some(line_end) = buffer.find('\n') {
            let line = buffer[..line_end].trim_end_matches('\r').to_string();
            buffer = buffer[line_end + 1..].to_string();

            // `strip_prefix("data:")`, not `"data: "` — the space is spec-optional (a
            // spec-legal `data:{...}` frame with no space was silently skipped here, so a pause
            // riding one was never detected), same lenient match `agent_proxy.rs`'s own SSE tap
            // already uses.
            let Some(data) = line.strip_prefix("data:") else {
                continue;
            };
            let data = data.trim();
            if data.is_empty() {
                continue;
            }
            last_data = Some(data.to_string());
            // The real agent event, verbatim — normalized to the exact same wire shape a live
            // turn's own `agent_stream()` already emits, so a reconnecting client's existing SSE
            // parser needs no reconnect-specific branch.
            continuation.push(normalize_agent_event(data, task_id, context_id));

            if let Ok(event) = serde_json::from_str::<serde_json::Value>(data) {
                let result = event.get("result").unwrap_or(&event);
                if let Some(text) = crate::agent_proxy::artifact_chunk_text(result) {
                    artifact_text.push_str(&text);
                } else if let Some(text) = crate::agent_proxy::task_reply_text(result)
                    .or_else(|| crate::agent_proxy::message_parts_text(result))
                {
                    terminal_text = Some(text);
                }
            }

            let disposition = nasiko_types::a2a::classify_stream_disposition(data);
            if disposition != StreamDisposition::Continue {
                let reply_text = if !artifact_text.is_empty() {
                    Some(artifact_text)
                } else {
                    terminal_text
                };
                return Some((disposition, last_data, reply_text));
            }
        }
    }

    let reply_text = if !artifact_text.is_empty() {
        Some(artifact_text)
    } else {
        terminal_text
    };
    Some((StreamDisposition::Completed, last_data, reply_text))
}

/// Non-streaming (`message/send`-style) reply path. On a JSON-RPC `error`, retries once with
/// plain `message/send` (via `build_send_request_for_task`) — mirrors `a2a_dispatch.rs`'s own
/// dispatch-time fallback, which this originally lacked. Still `error` after the retry (or the
/// retry itself fails to send/parse) is treated as a failed delivery outcome, same as before.
async fn consume_json_to_terminal(
    response: reqwest::Response,
    context_id: &str,
    task_id: &str,
    answer: &str,
    build_req: impl Fn(&nasiko_types::a2a::JsonRpcRequest) -> reqwest::RequestBuilder,
    continuation: &ContinuationGuard,
) -> Option<(StreamDisposition, Option<String>, Option<String>)> {
    let mut body: serde_json::Value = response.json().await.ok()?;

    if body.get("error").is_some() {
        let retry_body =
            nasiko_types::a2a::build_send_request_for_task(answer, context_id, task_id);
        if let Ok(retry_response) = build_req(&retry_body).send().await
            && let Ok(retry_json) = retry_response.json::<serde_json::Value>().await
        {
            body = retry_json;
        }
    }

    let data = body.to_string();
    // Single reply, not a chunked stream — one push covers the whole non-streaming path, same
    // normalization as the streaming path above so a reconnect sees a consistent shape either way.
    continuation.push(normalize_agent_event(&data, task_id, context_id));
    if body.get("error").is_some() {
        return Some((StreamDisposition::Failed, Some(data), None));
    }

    let disposition = match nasiko_types::a2a::classify_stream_disposition(&data) {
        StreamDisposition::Continue => StreamDisposition::Completed,
        other => other,
    };
    // Non-streaming replies are self-contained (no chunked artifactUpdate to accumulate across),
    // so `extract_text` on the whole body — same helper `agent_stream()`'s own non-streaming
    // branch uses — is enough here, unlike the streaming path above.
    let reply_text = nasiko_types::a2a::extract_text(body.get("result").unwrap_or(&body));
    Some((disposition, Some(data), reply_text))
}

/// Flip the `flow_steps` row this pause interrupted out of `awaiting_human` once the answer has
/// reached the agent. `'resumed'`, not `completed`/`failed`: no `ToolResult` ever arrives for a
/// paused step, so either would misreport the original call's own outcome. Only
/// `orchestrator_stream` writes `awaiting_human` steps, hence the origin guard.
///
/// Keyed through `session_traces`, not this resume's `flow_ctx`: the paused step belongs to the
/// turn that paused, and `deliver()` opens a fresh root flow, so the two ids never match.
/// Owner-scoped for free — both columns reference `chat_sessions(session_id)`.
async fn close_resumed_flow_step(state: &AppState, row: &HitlRequest, agent_name: &str) {
    if row.origin != HitlOrigin::Orchestrator {
        return;
    }
    let Some(chat_session_id) = row.chat_session_id.as_deref() else {
        return;
    };
    // `flow_steps.agent_name` holds the display-folded form, never the raw registry name here.
    let display_name = nasiko_react_agent::A2aTool::agent_display_name(agent_name);
    // Narrowed to the single most recently paused row, matching `orchestrator_stream`'s own
    // `AwaitingHuman` close-out (`a2a_dispatch.rs`'s `ORDER BY step_order DESC LIMIT 1` subquery)
    // — `step_order` alone can't serve here since it only orders steps *within* one flow_id, and
    // this session's `session_traces` mapping can span several. `created_at DESC` is the
    // equivalent global ordering across flow_ids. Without this limit, an orchestrator session
    // that delegates twice to the same sub-agent with both calls paused had answering the first
    // one flip *both* `awaiting_human` rows to `resumed`, falsely reporting a still-paused step
    // as resumed in the flow timeline (found in review).
    if let Err(e) = sqlx::query(
        "UPDATE flow_steps SET status = 'resumed'
          WHERE id = (
              SELECT id FROM flow_steps
               WHERE status = 'awaiting_human' AND agent_name = $2
                 AND flow_id IN (SELECT trace_id FROM session_traces WHERE session_id = $1)
               ORDER BY created_at DESC
               LIMIT 1
          )",
    )
    .bind(chat_session_id)
    .bind(&display_name)
    .execute(&state.db)
    .await
    {
        // Not swallowed: a row left `awaiting_human` stays stuck forever.
        tracing::warn!(
            id = %row.id, %chat_session_id, error = %e,
            "hitl resume: failed to close the paused flow_steps row"
        );
    }
}

/// Minimal observable trail for the resumed call — required by the plan's own Governing
/// Principle (§1: a `hitl_requests` row is "associated with, never a replacement for" the owning
/// subsystem's own durable state; for direct chat that's `flows`/`session_traces`. The agent's
/// actual reply text is a separate concern, handled by `persist_resume_reply` below —
/// deliberately not folded in here, since this function runs for every disposition and that one
/// only makes sense for a real terminal reply. Still not duplicated here: OTel span content
/// capture, token-usage summarization — presentation/telemetry polish, not correctness.
///
/// Closes out `deliver()`'s own `flows` row (`flow_ctx.flow_id`, opened right before this resume's
/// agent call) with the precise disposition-derived terminal status — it does not insert a second
/// row. An earlier version minted its own separate `flows` row for this and a second,
/// `context_id`-keyed `session_traces` row alongside it; both were dead weight once `deliver()`
/// itself started registering `flow_ctx.flow_id` as the live flow up front (the only thing MCP
/// retry-matching actually reads), and the `session_traces` insert additionally violated
/// `session_traces.session_id`'s FK to `chat_sessions(session_id)` on every single call —
/// `context_id` is never a real chat session, so it silently failed under its own `let _ =` every
/// time (`deliver()`'s own `session_traces` insert above, keyed on `row.chat_session_id`, is the
/// one that actually works).
async fn record_resume_trail(
    state: &AppState,
    disposition: StreamDisposition,
    flow_ctx: &FlowContext,
) {
    let status = match disposition {
        StreamDisposition::Completed | StreamDisposition::Continue => "completed",
        StreamDisposition::Failed => "failed",
        StreamDisposition::Paused => "paused",
    };
    if let Err(e) = sqlx::query(
        r#"UPDATE flows SET status = $2,
           duration_ms = EXTRACT(EPOCH FROM (now() - created_at))::bigint * 1000,
           completed_at = CASE WHEN $2 IN ('completed', 'failed') THEN now() ELSE completed_at END
           WHERE flow_id = $1"#,
    )
    .bind(&flow_ctx.flow_id)
    .bind(status)
    .execute(&state.db)
    .await
    {
        // Not just cosmetic: a failure here leaves this flow's `status = 'running'`, which is
        // exactly what keeps the resumed agent's traceparent authorized to call `/api/mcp`
        // (`ResumeFlowCloser`'s own doc comment) — `_flow_closer`'s `Drop` fallback will still
        // close it, but only once `deliver()` returns, so it's worth knowing this path failed.
        tracing::warn!(error = %e, flow_id = %flow_ctx.flow_id, "record_resume_trail: failed to close the resume flow");
    }
}

/// After an orchestrator-origin pause resumes successfully (not another pause), the sub-agent's
/// reply is not the final answer — unlike direct_chat/agent_proxy, it needs to go back through the
/// ReAct loop for the orchestrating LLM to reason over. Triggers a brand-new orchestrator turn on
/// the same chat session, seeded with a continuation message describing what the sub-agent said.
///
/// Must actively drain the resulting stream to a terminal event itself: `orchestrator_stream`'s
/// response body is an `async_stream::stream!` generator, and none of its side effects (persisting
/// the reply, closing out flow_steps, handling a chained pause) happen unless something polls it —
/// the same reason a disconnected browser client would otherwise silently lose them. There is no
/// live client for this call, so this function is that consumer; the bytes themselves go nowhere.
async fn trigger_new_orchestrator_turn(
    state: &AppState,
    row: &HitlRequest,
    agent_name: &str,
    reply_text: Option<String>,
) {
    let Some(chat_session_id) = row.chat_session_id.clone() else {
        // Guaranteed by construction (`NewHitlRequest::orchestrator` always sets it) —
        // defensive, not a real path.
        tracing::error!(id = %row.id, "hitl dispatcher: orchestrator row missing chat_session_id");
        return;
    };

    // Framed as an already-done status report, not a fresh ask — and deliberately sent with NO
    // prior history glued in front of it (contrast every other `orchestrator_stream` caller, which
    // does via `SessionHistory::with_current_query`): reframing this text alone wasn't enough,
    // because gluing the full transcript back in put the original, still-verbatim "please do X"
    // request right back in front of the orchestrating LLM, which then re-read it as outstanding
    // and re-invoked the same tool — pausing for approval again, forever
    // (docs/HITL_ORCHESTRATOR_BRANCH_STATUS.md:225-233, :288-293). This continuation message is
    // self-contained (it names the agent and repeats what it did), so the orchestrating LLM needs
    // nothing else to relay it — and history is not lost, only skipped for *this* synthesis call:
    // it's still persisted as `raw_text` below, so the next real user turn sees it normally.
    let continuation = match reply_text.filter(|t| !t.is_empty()) {
        Some(text) => format!(
            "The {agent_name} agent already completed the previously requested action and replied: {text}\n\nRelay this result to the user. The action has already been performed — do not call the same tool or repeat the action again."
        ),
        // auth_required has no free-text reply — same "intent, not success" framing `answer_text`
        // above already uses for the agent-facing side of this same resume.
        None => format!(
            "The {agent_name} agent has already completed the previously requested step. Tell the user it's done — do not repeat the action."
        ),
    };

    // The orchestrator's own system prompt (`react_loop.rs`) reads every turn as "analyze the
    // user's request, determine which agent can help" — reasonable for a real user message, but
    // `continuation` above is not one: it's a delegated agent's own reply to a call the
    // orchestrator already made, being fed back in because a paused sub-agent call has no way to
    // resume the ORIGINAL in-progress ReAct turn's own tool-call state (§Step 7's known
    // limitation — ContextManager/SessionHistory only carry flat user/assistant text, not an
    // interrupted tool-call transcript). Read as a plain "user" turn, the model has no signal
    // that it's the OUTCOME of an action it already took rather than a new one to take — and
    // confirmed live, it can and does call the same agent again for the same completed action
    // (e.g. "created the issue" read as "please create the issue"). This instruction is reasoning
    // input only, not shown to the human: `raw_text` below (what actually gets persisted to
    // `chat_messages`) stays the plain `continuation` text, unchanged.
    let reasoning_query = format!(
        "{continuation}\n\n\
         (System note: this is the result of an action you already delegated, not a new request \
         from the user. If it fully answers the original request, respond to the user with the \
         result now as plain text — do not call {agent_name}, or any other agent, again for the \
         same action.)"
    );

    let history = nasiko_orchestrator::SessionHistory::fetch(&chat_session_id, &state.db, 20).await;
    let query = history.with_current_query(&reasoning_query);
    let new_task_id = Uuid::new_v4().to_string();

    // Never assume the original turn's privilege level — apply the resumed user's real, current
    // grants rather than risk over-broad or under-broad agent visibility on a stale assumption.
    // `owner_user_id` is a NOT NULL FK with ON DELETE CASCADE (0007_hitl.sql), so this row can only
    // exist while its owner still does; a lookup failure is defensive, not a real path, and falls
    // back to the safe (non-superuser) default rather than aborting the resume over it.
    let is_superuser: bool = sqlx::query_scalar("SELECT is_superuser FROM users WHERE id = $1")
        .bind(row.owner_user_id)
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten()
        .unwrap_or(false);

    let result = orchestrator_stream(
        state,
        OrchestratorTurn {
            query: &query,
            raw_text: &continuation,
            task_id: &new_task_id,
            context_id: &chat_session_id,
            user_id: row.owner_user_id,
            is_superuser,
            client_owns_transcript: false,
            // Not `"user"`: `continuation` is written by the platform, not typed by the human.
            // As a user-role row it drew a bubble in the transcript quoting the sub-agent back
            // at them ("The archive agent replied: …"). It still belongs in the session's own
            // history — a re-paused turn leaves no assistant reply behind — so it is persisted
            // under a role the transcript does not render.
            transcript_role: crate::router::a2a_dispatch::INTERNAL_TRANSCRIPT_ROLE,
            file_parts: Vec::new(),
            // `reasoning_query` above explicitly instructs the model NOT to call any
            // agent again, because the call this turn reports on already succeeded in
            // an earlier turn. A policy that demands an agent call per turn would see
            // a turn with no call and replace the agent's own result with a refusal —
            // breaking every HITL resume — so it is told what kind of turn this is.
            kind: crate::orchestrator_policy::TurnKind::Continuation,
        },
    )
    .await;

    match result {
        Ok(response) => {
            // `orchestrator_stream`'s response body is the exact same SSE-framed byte stream a
            // live browser connection would read (§4/A of the reconnect investigation) — capture
            // each real `data:` payload into the continuation buffer instead of discarding it, so
            // a reconnected `POST /api/orchestrator/a2a` sees the actual re-entered ReAct turn,
            // not a synthesized summary of it. Already in the correct wire shape (this IS the
            // live-turn generator), so no `normalize_agent_event` pass is needed here.
            let mut body = response.into_body().into_data_stream();
            let mut buffer = String::new();
            while let Some(chunk) = body.next().await {
                let Ok(chunk) = chunk else { continue };
                buffer.push_str(&String::from_utf8_lossy(&chunk));
                while let Some(line_end) = buffer.find('\n') {
                    let line = buffer[..line_end].trim_end_matches('\r').to_string();
                    buffer = buffer[line_end + 1..].to_string();
                    // See the other call site's comment: the space in `"data:"` is optional.
                    if let Some(data) = line.strip_prefix("data:") {
                        let data = data.trim();
                        if !data.is_empty() {
                            state.continuation_events.append(row.id, data.to_string());
                        }
                    }
                }
            }
        }
        Err(e) => {
            // The resume itself already succeeded and was marked `resume_status = completed`
            // above — this failure only means the follow-up turn didn't run. Not retried: the
            // same at-least-once, not-exactly-once gap already accepted for the two-phase resume
            // more generally, not a new one introduced here.
            tracing::error!(
                id = %row.id, error = ?e,
                "hitl dispatcher: failed to trigger the resumed orchestrator turn"
            );
        }
    }
}

/// Persists the agent's final reply after a successful (non-`Paused`) resume into
/// `chat_messages`, so any caller — not just a future UI — can read what the agent actually
/// said, not just that delivery succeeded (`resume_status: completed` only ever meant "a valid
/// terminal response was received," never "here's what it was").
///
/// Keys on `row.chat_session_id` when the row has one, not `context_id`: `context_id` is the
/// agent's own private A2A context, which on the web-chat path (`agent_proxy.rs`'s
/// `ensure_chat_session`) is a *different* id from the `chat_sessions.session_id` the UI actually
/// reads `/api/chat/sessions/{id}/messages` against. Keying on `context_id` unconditionally wrote
/// the reply into a session the caller never opened — silence in the UI the human was actually
/// watching, and a phantom "New chat" row accumulating unseen replies instead. Falls back to
/// `context_id` only when `chat_session_id` is absent (rows from an origin that never threaded
/// one through), matching this function's own pre-existing behavior for that case.
///
/// `agent_stream()`'s direct-chat branch never creates a `chat_sessions` row itself — only
/// `orchestrator_stream()`'s `ensure_orchestrator_chat_session` does, and only for the
/// routing-engine path. A session row for a direct-chat conversation exists today only if the
/// caller (e.g. the web UI, via its own `/api/chat/sessions/{id}/messages` call) separately made
/// one. Every request this dispatcher handles is a resume of an *existing* pause, so no such row
/// may exist at all — a bare `INSERT INTO chat_messages` would fail outright on the FK
/// (`chat_messages.session_id REFERENCES chat_sessions(session_id)`). The idempotent upsert below
/// mirrors `ensure_orchestrator_chat_session`'s exact pattern (`a2a_dispatch.rs`) and is safe
/// whether or not a session row already exists.
async fn persist_resume_reply(
    state: &AppState,
    row: &HitlRequest,
    context_id: &str,
    reply_text: Option<String>,
) {
    let Some(text) = reply_text.filter(|t| !t.is_empty()) else {
        return;
    };
    let session_id = row.chat_session_id.as_deref().unwrap_or(context_id);

    if let Err(e) = sqlx::query(
        "INSERT INTO chat_sessions (session_id, user_id, agent_id, agent_url, title, session_type) \
         VALUES ($1, $2, $3, '/api/orchestrator/a2a', 'New chat', 'orchestrator') \
         ON CONFLICT (session_id) DO NOTHING",
    )
    .bind(session_id)
    .bind(row.owner_user_id)
    .bind(row.agent_id)
    .execute(&state.db)
    .await
    {
        tracing::warn!(
            error = %e, id = %row.id, %session_id,
            "persist_resume_reply: failed to ensure chat_sessions row — the reply insert below \
             will fail its FK if this session row doesn't already exist"
        );
    }

    // The agent's actual reply text — not just optional bookkeeping. A failure here silently
    // loses the one thing this function exists to persist, with nothing else to fall back on.
    if let Err(e) = sqlx::query(
        "INSERT INTO chat_messages (session_id, role, content) VALUES ($1, 'assistant', $2)",
    )
    .bind(session_id)
    .bind(&text)
    .execute(&state.db)
    .await
    {
        tracing::error!(
            error = %e, id = %row.id, %session_id,
            "persist_resume_reply: failed to persist the agent's reply — it is now lost"
        );
    }
}

#[cfg(test)]
mod answer_text_tests {
    use super::*;
    use nasiko_hitl::{HitlKind, HitlOrigin, HitlStatus, ResumeStatus};

    fn row_with_response(human_response: Option<serde_json::Value>) -> HitlRequest {
        let now = chrono::Utc::now();
        HitlRequest {
            id: Uuid::new_v4(),
            kind: HitlKind::InputRequired,
            origin: HitlOrigin::DirectChat,
            status: HitlStatus::Resolved,
            resume_status: ResumeStatus::NotStarted,
            agent_id: Uuid::new_v4(),
            owner_user_id: Uuid::new_v4(),
            resolved_by: None,
            task_id: Some("task-1".into()),
            context_id: Some("ctx-1".into()),
            chat_session_id: None,
            maf_execution_id: None,
            maf_step_index: None,
            connector_id: None,
            tool_name: None,
            arguments_hash: None,
            consumed_at: None,
            question: serde_json::Value::Null,
            human_response,
            resume_state: serde_json::Value::Null,
            resume_claimed_at: None,
            resume_dispatch_attempts: 0,
            resume_last_error: None,
            created_at: now,
            updated_at: now,
            expires_at: None,
            resolved_at: None,
        }
    }

    #[test]
    fn plain_string_answer_is_unchanged() {
        let row = row_with_response(Some(serde_json::json!({ "answer": "production" })));
        assert_eq!(answer_text(&row), "production");
    }

    #[test]
    fn single_select_predefined_answer_is_the_label_verbatim() {
        let row = row_with_response(Some(serde_json::json!({ "answer": "Summary" })));
        assert_eq!(answer_text(&row), "Summary");
    }

    #[test]
    fn single_select_custom_answer_is_the_raw_text_verbatim() {
        let row = row_with_response(Some(
            serde_json::json!({ "answer": "Give me a concise executive summary" }),
        ));
        assert_eq!(answer_text(&row), "Give me a concise executive summary");
    }

    #[test]
    fn multi_select_answers_join_by_newline_not_comma() {
        // The whole point of not comma-joining: a label containing a comma must round-trip
        // unambiguously.
        let row = row_with_response(Some(serde_json::json!({
            "answer": ["Introduction, architecture and design", "Security, privacy and compliance"]
        })));
        assert_eq!(
            answer_text(&row),
            "Introduction, architecture and design\nSecurity, privacy and compliance"
        );
    }

    #[test]
    fn multi_select_with_custom_answer_appends_it_as_a_trailing_line() {
        let row = row_with_response(Some(serde_json::json!({
            "answer": ["Introduction", "Security"],
            "custom_answer": "Also include deployment risks",
        })));
        assert_eq!(
            answer_text(&row),
            "Introduction\nSecurity\nAlso include deployment risks"
        );
    }

    #[test]
    fn multi_select_with_only_custom_answer_is_a_single_line() {
        let row = row_with_response(Some(serde_json::json!({
            "answer": [],
            "custom_answer": "Only discuss security implications",
        })));
        assert_eq!(answer_text(&row), "Only discuss security implications");
    }

    #[test]
    fn auth_outcome_confirmed_is_unchanged() {
        let row = row_with_response(Some(serde_json::json!({ "auth_outcome": "confirmed" })));
        assert_eq!(answer_text(&row), "authorized");
    }

    #[test]
    fn auth_outcome_denied_is_unchanged() {
        let row = row_with_response(Some(serde_json::json!({ "auth_outcome": "denied" })));
        assert_eq!(answer_text(&row), "denied");
    }

    #[test]
    fn unrecognized_auth_outcome_fails_closed_to_denied() {
        let row = row_with_response(Some(serde_json::json!({ "auth_outcome": "expired" })));
        assert_eq!(answer_text(&row), "denied");
    }

    #[test]
    fn missing_auth_outcome_fails_closed_to_denied() {
        let row = row_with_response(Some(serde_json::json!({})));
        assert_eq!(answer_text(&row), "denied");
    }

    // Previously asserted the empty string here — that was the bug: a missing `human_response`
    // used to build an A2A request with an empty text part rather than failing closed.
    // `answer_text` can't tell "an `input_required` row with no answer yet" apart from "an
    // `auth_required` row with no recognized `auth_outcome`" once `human_response` itself is
    // `None`, so it fails closed to "denied" for both rather than sending nothing.
    #[test]
    fn no_human_response_fails_closed_to_denied() {
        let row = row_with_response(None);
        assert_eq!(answer_text(&row), "denied");
    }
}

use axum::{
    extract::{Multipart, State},
    http::StatusCode,
    response::{
        sse::{Event, Sse},
        IntoResponse, Response,
    },
    Json,
};
use futures::StreamExt;
use serde_json::json;
use std::convert::Infallible;
use std::time::Instant;
use uuid::Uuid;

use nasiko_types::a2a::{self as a2a, JsonRpcRequest, PartContent, StreamResponse};
use nasiko_react_agent::{
    AgentInfo, AgentSkill as OrcAgentSkill, Orchestrator, OrchestratorConfig, OrchestratorEvent,
    RegistrySource,
};

use nasiko_router::{AgentSelector, SessionHistory};

use nasiko_flow::FlowContext;

use crate::acl::CpCallGuard;
use crate::auth::Claims;
use crate::state::AppState;
use crate::usage::TokenUsageBuilder;

// TODO: Implement `AgentExecutor` trait from a2a-server-lf to replace manual Axum routing.
// The trait has two methods: execute(&self, ctx: ExecutorContext) -> BoxStream<StreamResponse>
// and cancel(&self, ctx: ExecutorContext) -> BoxStream<StreamResponse>.
// Our handler already returns a stream of StreamResponse — wrap it in the trait impl and let
// a2a-server handle JSON-RPC envelope, SSE serialization, and /.well-known/agent-card.json.

/// Server-side A2A dispatch endpoint. Accepts JSONRPC `message/send` or `message/stream`.
/// Dispatches to the routing engine (no agent_id), ReAct orchestrator (agent_id=orchestrator),
/// or a specific agent directly.
///
/// Note: the gateway has its own `orchestrator.rs` for the production A2A path (with auth,
/// rate limiting, CORS). This handler lives in the server for direct access and integration
/// testing without the gateway layer.
pub async fn a2a_dispatch_handler(
    State(state): State<AppState>,
    claims: Claims,
    Json(req): Json<JsonRpcRequest>,
) -> Result<Response, A2aDispatchError> {
    let params: nasiko_types::a2a::SendMessageRequest = serde_json::from_value(
        req.params.clone().ok_or_else(|| A2aDispatchError::InvalidRequest("missing params".into()))?,
    )
    .map_err(|e| A2aDispatchError::InvalidRequest(format!("bad params: {e}")))?;

    let text = params
        .message
        .parts
        .iter()
        .filter_map(|p| match &p.content {
            PartContent::Text(t) => Some(t.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");

    if text.is_empty() {
        return Err(A2aDispatchError::InvalidRequest(
            "message must contain at least one text part".into(),
        ));
    }

    let task_id = Uuid::new_v4().to_string();
    let context_id = params
        .message
        .context_id
        .clone()
        .unwrap_or_else(|| Uuid::new_v4().to_string());

    let agent_id = params.metadata
        .as_ref()
        .and_then(|m| m.get("agent_id"))
        .and_then(|v| v.as_str())
        .map(String::from);
    let session_id = params.metadata
        .as_ref()
        .and_then(|m| m.get("session_id"))
        .and_then(|v| v.as_str())
        .map(String::from);
    let is_orchestrator = agent_id.is_none() || agent_id.as_deref() == Some("orchestrator");

    let user_id: Uuid = claims.sub.parse().unwrap_or(Uuid::nil());

    let history = if let Some(ref sid) = session_id {
        SessionHistory::fetch(sid, &state.db, 20).await
    } else {
        SessionHistory::default()
    };

    let query = history.with_current_query(&text);

    if is_orchestrator {
        orchestrator_stream(&state, &query, &task_id, &context_id, user_id, claims.is_superuser).await
    } else {
        let target = agent_id.as_deref().unwrap();
        if let Ok(target_uuid) = target.parse::<Uuid>() {
            if !claims.is_superuser && !crate::acl::user_can_access_agent(&state.db, user_id, target_uuid).await {
                return Err(A2aDispatchError::Forbidden);
            }
        }
        agent_stream(&state, target, &query, &task_id, &context_id, user_id).await
    }
}

// ─── Orchestrator Path ───────────────────────────────────────────────────────

async fn orchestrator_stream(
    state: &AppState,
    query: &str,
    task_id: &str,
    context_id: &str,
    user_id: Uuid,
    is_superuser: bool,
) -> Result<Response, A2aDispatchError> {
    let all_agents = AgentSelector::fetch_active_agents(&state.db)
        .await
        .map_err(|e| A2aDispatchError::Internal(e.to_string()))?;

    // Filter to agents the requesting user can access.
    let agent_summaries = if is_superuser {
        all_agents
    } else {
        let mut accessible = Vec::new();
        for summary in all_agents {
            if crate::acl::user_can_access_agent(&state.db, user_id, summary.id).await {
                accessible.push(summary);
            }
        }
        accessible
    };

    let mut agents: Vec<AgentInfo> = Vec::new();
    for summary in &agent_summaries {
        let endpoint = match resolve_endpoint(state, &summary.name).await {
            Ok(url) => url,
            Err(_) => continue,
        };
        agents.push(AgentInfo {
            id: summary.id.to_string(),
            name: summary.name.clone(),
            description: summary.description.clone(),
            endpoint,
            skills: summary
                .skills
                .iter()
                .enumerate()
                .map(|(i, s)| OrcAgentSkill {
                    id: format!("{}-skill-{}", summary.name, i),
                    name: s.name.clone(),
                    description: s.description.clone(),
                    tags: summary.tags.clone(),
                })
                .collect(),
        });
    }

    if agents.is_empty() {
        return Err(A2aDispatchError::NoAgents);
    }

    let config = OrchestratorConfig {
        model: std::env::var("OPENAI_MODEL").unwrap_or_else(|_| "deepseek-v4-flash".into()),
        base_url: std::env::var("OPENAI_BASE_URL").ok(),
        api_key: std::env::var("OPENAI_API_KEY").ok(),
        max_turns: 10,
        temperature: Some(0.2),
        ..Default::default()
    };

    let flow_ctx = FlowContext::new_root();
    let flow_id = flow_ctx.flow_id.clone();
    let traceparent = flow_ctx.to_traceparent();
    state.flow_guard.init_flow(&flow_ctx, "orchestrator").await;

    let _ = sqlx::query(
        r#"INSERT INTO flows (flow_id, user_id, root_agent_name, title, status, metadata)
           VALUES ($1, $2, 'orchestrator', $3, 'running', '{}'::jsonb)
           ON CONFLICT (flow_id) DO NOTHING"#,
    )
    .bind(&flow_id)
    .bind(user_id)
    .bind(query)
    .execute(&state.db)
    .await;

    let caller_uuid: Option<Uuid> = None;
    let guard = CpCallGuard::new(
        state.db.clone(),
        state.flow_guard.clone(),
        flow_ctx,
        caller_uuid,
    );

    let a2a_client = nasiko_react_agent::A2aClient::new()
        .with_headers(vec![("traceparent".to_string(), traceparent)]);

    let mut orchestrator = Orchestrator::new(config, RegistrySource::Static(agents))
        .with_a2a_client(a2a_client)
        .with_guard(guard);
    orchestrator
        .init()
        .await
        .map_err(|e| A2aDispatchError::Internal(e.to_string()))?;

    let mut rx = orchestrator.run_stream(query);
    let task_id = task_id.to_string();
    let context_id = context_id.to_string();
    let artifact_id = Uuid::new_v4().to_string();
    let flow_events = state.flow_events.clone();
    let mut flow_rx = state.flow_events.subscribe(&flow_id).await;
    let flow_id_cleanup = flow_id.clone();
    let db = state.db.clone();
    let usage_tracker = state.usage_tracker.clone();
    let genai_metrics = state.genai_metrics.clone();
    let orchestrator_model = std::env::var("OPENAI_MODEL").unwrap_or_else(|_| "deepseek-v4-flash".into());
    let orchestrator_start = Instant::now();

    let stream = async_stream::stream! {
        yield Ok::<_, Infallible>(to_sse(a2a::status_event(a2a::working(&task_id, &context_id))));

        // Emit trace_id so the UI can link this response to its distributed trace.
        {
            let meta_msg = a2a::agent_message(&context_id, &task_id, a2a::data_part(json!({
                "type": "trace_meta", "trace_id": flow_id,
            })));
            yield Ok(to_sse(a2a::status_event(a2a::working_with_message(&task_id, &context_id, meta_msg))));
        }

        let mut content_started = false;

        loop {
            tokio::select! {
                biased;

                maybe_event = rx.recv() => {
                    let Some(event) = maybe_event else { break };
                    match event {
                        OrchestratorEvent::Thinking { content } => {
                            let msg = a2a::agent_message(&context_id, &task_id, a2a::data_part(json!({"type": "thinking", "content": content})));
                            yield Ok(to_sse(a2a::status_event(a2a::working_with_message(&task_id, &context_id, msg))));
                        }
                        OrchestratorEvent::ToolCall { agent, message, turn } => {
                            let _ = sqlx::query(
                                r#"INSERT INTO flow_steps (flow_id, step_order, depth, agent_name, caller_agent_name, input_summary, status, created_at)
                                   VALUES ($1, $2, 1, $3, 'orchestrator', $4, 'running', now())"#,
                            )
                            .bind(&flow_id_cleanup)
                            .bind(turn as i32)
                            .bind(&agent)
                            .bind(&message)
                            .execute(&db)
                            .await;

                            // Record agent invocation in OTel
                            genai_metrics.record_invocation(&agent, "");

                            let msg = a2a::agent_message(&context_id, &task_id, a2a::data_part(json!({
                                "type": "tool_call",
                                "agent": agent,
                                "message": message,
                                "turn": turn,
                            })));
                            yield Ok(to_sse(a2a::status_event(a2a::working_with_message(&task_id, &context_id, msg))));
                        }
                        OrchestratorEvent::ToolResult { agent, result, success, turn } => {
                            let status_str = if success { "completed" } else { "failed" };
                            let _ = sqlx::query(
                                r#"UPDATE flow_steps SET status = $3, output_summary = $4,
                                   latency_ms = EXTRACT(EPOCH FROM (now() - created_at))::integer * 1000,
                                   completed_at = now()
                                   WHERE flow_id = $1 AND step_order = $2"#,
                            )
                            .bind(&flow_id_cleanup)
                            .bind(turn as i32)
                            .bind(status_str)
                            .bind(&result)
                            .execute(&db)
                            .await;

                            let msg = a2a::agent_message(&context_id, &task_id, a2a::data_part(json!({
                                "type": "tool_result",
                                "agent": agent,
                                "result": result,
                                "success": success,
                                "turn": turn,
                            })));
                            yield Ok(to_sse(a2a::status_event(a2a::working_with_message(&task_id, &context_id, msg))));
                        }
                        OrchestratorEvent::PolicyRejected { agent, reason, turn } => {
                            let msg = a2a::agent_message(&context_id, &task_id, a2a::data_part(json!({
                                "type": "policy_rejected",
                                "agent": agent,
                                "reason": reason,
                                "turn": turn,
                            })));
                            yield Ok(to_sse(a2a::status_event(a2a::working_with_message(&task_id, &context_id, msg))));
                        }
                        OrchestratorEvent::Usage { input_tokens, output_tokens, model } => {
                            // Fire-and-forget: track token usage in DB
                            let tracker = usage_tracker.clone();
                            let uid = user_id;
                            let fid = flow_id_cleanup.clone();
                            let m = model.clone();
                            tokio::spawn(async move {
                                let usage = TokenUsageBuilder::new(
                                    uid,
                                    "orchestrator",
                                    "openai",
                                    &m,
                                )
                                .tokens(input_tokens as i32, output_tokens as i32)
                                .session_id(&fid)
                                .streaming(false)
                                .build();
                                if let Err(e) = tracker.track_tokens(usage).await {
                                    tracing::warn!(error = %e, "failed to track orchestrator token usage");
                                }
                            });

                            // Also record in OTel GenAI metrics
                            genai_metrics.record_tokens(
                                input_tokens,
                                output_tokens,
                                &model,
                                "orchestrator",
                                "",
                            );
                        }
                        OrchestratorEvent::Content { content } => {
                            yield Ok(to_sse(a2a::artifact_event(a2a::text_chunk(
                                &task_id, &context_id, &artifact_id, &content, content_started, false,
                            ))));
                            content_started = true;
                        }
                        OrchestratorEvent::Done { .. } => {
                            if content_started {
                                yield Ok(to_sse(a2a::artifact_event(a2a::text_chunk(
                                    &task_id, &context_id, &artifact_id, "", true, true,
                                ))));
                            }

                            // Record overall operation duration in OTel
                            genai_metrics.record_operation(
                                orchestrator_start.elapsed().as_secs_f64(),
                                "orchestrate",
                                &orchestrator_model,
                                "orchestrator",
                                "",
                            );

                            yield Ok(to_sse(a2a::status_event(a2a::completed(&task_id, &context_id))));
                            break;
                        }
                        OrchestratorEvent::Error { message } => {
                            yield Ok(to_sse(a2a::status_event(a2a::failed(&task_id, &context_id, &message))));
                            break;
                        }
                    }
                }

                flow_event = flow_rx.recv() => {
                    let Ok(fe) = flow_event else { continue };
                    let msg = a2a::agent_message(&context_id, &task_id, a2a::data_part(serde_json::to_value(&fe).unwrap_or_default()));
                    yield Ok(to_sse(a2a::status_event(a2a::working_with_message(&task_id, &context_id, msg))));
                }
            }
        }

        let _ = sqlx::query(
            r#"UPDATE flows SET status = 'completed',
               duration_ms = EXTRACT(EPOCH FROM (now() - created_at))::bigint * 1000,
               completed_at = now()
               WHERE flow_id = $1"#,
        )
        .bind(&flow_id_cleanup)
        .execute(&db)
        .await;

        flow_events.remove(&flow_id_cleanup).await;
    };

    Ok(Sse::new(stream).into_response())
}

// ─── Direct Agent Path ───────────────────────────────────────────────────────

async fn agent_stream(
    state: &AppState,
    target: &str,
    query: &str,
    task_id: &str,
    context_id: &str,
    user_id: Uuid,
) -> Result<Response, A2aDispatchError> {
    let agent = sqlx::query_as::<_, AgentRow>(
        "SELECT id, name, status FROM agents WHERE (id::text = $1 OR name = $1) AND status = 'running'",
    )
    .bind(target)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| A2aDispatchError::Internal(e.to_string()))?
    .ok_or_else(|| A2aDispatchError::AgentNotFound(target.to_string()))?;

    let endpoint = resolve_endpoint(state, &agent.name)
        .await
        .map_err(A2aDispatchError::Internal)?;

    let flow_ctx = FlowContext::new_root();
    let flow_id = flow_ctx.flow_id.clone();
    state.flow_guard.init_flow(&flow_ctx, &agent.name).await;

    let _ = sqlx::query(
        r#"INSERT INTO flows (flow_id, user_id, root_agent_name, title, status, metadata)
           VALUES ($1, $2, $3, $4, 'running', '{}'::jsonb)
           ON CONFLICT (flow_id) DO NOTHING"#,
    )
    .bind(&flow_id)
    .bind(user_id)
    .bind(&agent.name)
    .bind(query)
    .execute(&state.db)
    .await;

    let req_body = nasiko_types::a2a::build_stream_request(query, Some(context_id));

    let response = state
        .http_client
        .post(&endpoint)
        .header("A2A-Version", "1.0")
        .header("traceparent", flow_ctx.to_traceparent())
        .json(&req_body)
        .send()
        .await
        .map_err(|e| A2aDispatchError::Internal(format!("agent request failed: {e}")))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(A2aDispatchError::Internal(format!(
            "agent HTTP {}: {}",
            status, body
        )));
    }

    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();

    let task_id = task_id.to_string();
    let context_id = context_id.to_string();
    let flow_events = state.flow_events.clone();
    let mut flow_rx = state.flow_events.subscribe(&flow_id).await;
    let db = state.db.clone();

    if content_type.contains("text/event-stream") {
        let byte_stream = response.bytes_stream();
        let flow_id_cleanup = flow_id;

        let stream = async_stream::stream! {
            yield Ok::<_, Infallible>(to_sse(a2a::status_event(a2a::working(&task_id, &context_id))));

            // Emit trace_id so the UI can link this response to its distributed trace.
            {
                let meta_msg = a2a::agent_message(&context_id, &task_id, a2a::data_part(json!({
                    "type": "trace_meta", "trace_id": flow_id_cleanup,
                })));
                yield Ok(to_sse(a2a::status_event(a2a::working_with_message(&task_id, &context_id, meta_msg))));
            }

            let mut buffer = String::new();
            let mut pinned = std::pin::pin!(byte_stream);
            let mut agent_done = false;

            loop {
                if agent_done { break; }

                tokio::select! {
                    biased;

                    chunk_result = pinned.next() => {
                        let Some(chunk_result) = chunk_result else {
                            agent_done = true;
                            continue;
                        };
                        let chunk = match chunk_result {
                            Ok(c) => c,
                            Err(_) => { agent_done = true; continue; }
                        };
                        buffer.push_str(&String::from_utf8_lossy(&chunk));

                        while let Some(line_end) = buffer.find('\n') {
                            let line = buffer[..line_end].trim_end_matches('\r').to_string();
                            buffer = buffer[line_end + 1..].to_string();

                            if let Some(data) = line.strip_prefix("data: ") {
                                if data.trim().is_empty() {
                                    continue;
                                }
                                let normalized = normalize_agent_event(data, &task_id, &context_id);
                                yield Ok(Event::default().data(normalized));
                            }
                        }
                    }

                    flow_event = flow_rx.recv() => {
                        let Ok(fe) = flow_event else { continue };
                        let msg = a2a::agent_message(&context_id, &task_id, a2a::data_part(serde_json::to_value(&fe).unwrap_or_default()));
                        yield Ok(to_sse(a2a::status_event(a2a::working_with_message(&task_id, &context_id, msg))));
                    }
                }
            }

            yield Ok(to_sse(a2a::status_event(a2a::completed(&task_id, &context_id))));

            let _ = sqlx::query(
                r#"UPDATE flows SET status = 'completed',
                   duration_ms = EXTRACT(EPOCH FROM (now() - created_at))::bigint * 1000,
                   completed_at = now()
                   WHERE flow_id = $1"#,
            ).bind(&flow_id_cleanup).execute(&db).await;
            flow_events.remove(&flow_id_cleanup).await;
        };

        Ok(Sse::new(stream).into_response())
    } else {
        // Non-streaming JSON — wrap as A2A stream
        let resp_body: serde_json::Value = response
            .json()
            .await
            .map_err(|e| A2aDispatchError::Internal(format!("invalid agent JSON: {e}")))?;

        let text = nasiko_types::a2a::extract_text(
            resp_body.get("result").unwrap_or(&resp_body),
        )
        .unwrap_or_else(|| "No response".into());

        let artifact_id = Uuid::new_v4().to_string();
        let flow_id_cleanup = flow_id;

        let stream = async_stream::stream! {
            yield Ok::<_, Infallible>(to_sse(a2a::status_event(a2a::working(&task_id, &context_id))));

            // Emit trace_id so the UI can link this response to its distributed trace.
            {
                let meta_msg = a2a::agent_message(&context_id, &task_id, a2a::data_part(json!({
                    "type": "trace_meta", "trace_id": flow_id_cleanup,
                })));
                yield Ok(to_sse(a2a::status_event(a2a::working_with_message(&task_id, &context_id, meta_msg))));
            }

            yield Ok(to_sse(a2a::artifact_event(a2a::text_chunk(
                &task_id, &context_id, &artifact_id, &text, false, true,
            ))));

            yield Ok(to_sse(a2a::status_event(a2a::completed(&task_id, &context_id))));

            let _ = sqlx::query(
                r#"UPDATE flows SET status = 'completed',
                   duration_ms = EXTRACT(EPOCH FROM (now() - created_at))::bigint * 1000,
                   completed_at = now()
                   WHERE flow_id = $1"#,
            ).bind(&flow_id_cleanup).execute(&db).await;
            flow_events.remove(&flow_id_cleanup).await;
        };

        Ok(Sse::new(stream).into_response())
    }
}

// ─── Router Stats ─────────────────────────────────────────────────────────────

/// `GET /api/router/stats` — admin-only.
/// Returns aggregated rows from the `agent_selection_stats` materialized view.
pub async fn router_stats_handler(
    State(state): State<AppState>,
    _claims: Claims,
) -> Result<axum::Json<serde_json::Value>, (StatusCode, String)> {
    let rows = sqlx::query_as::<_, StatsRow>(
        r#"SELECT
            selected_agent_name AS agent_name,
            selection_count,
            successful_calls,
            failed_calls,
            avg_agent_latency_ms,
            avg_selection_latency_ms,
            avg_stage1_candidates,
            avg_stage2_candidates,
            date::text AS date
        FROM agent_selection_stats
        ORDER BY date DESC, selection_count DESC
        LIMIT 200"#,
    )
    .fetch_all(&state.db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("db error: {e}")))?;

    let data: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|r| {
            serde_json::json!({
                "agent_name":              r.agent_name,
                "selection_count":         r.selection_count,
                "successful_calls":        r.successful_calls,
                "failed_calls":            r.failed_calls,
                "avg_agent_latency_ms":    r.avg_agent_latency_ms.map(|v| v.to_string()),
                "avg_selection_latency_ms":r.avg_selection_latency_ms.map(|v| v.to_string()),
                "avg_stage1_candidates":   r.avg_stage1_candidates.map(|v| v.to_string()),
                "avg_stage2_candidates":   r.avg_stage2_candidates.map(|v| v.to_string()),
                "date":                    r.date,
            })
        })
        .collect();

    Ok(axum::Json(serde_json::json!({ "data": data, "total": data.len() })))
}

#[derive(sqlx::FromRow)]
struct StatsRow {
    agent_name: Option<String>,
    selection_count: Option<i64>,
    successful_calls: Option<i64>,
    failed_calls: Option<i64>,
    avg_agent_latency_ms: Option<rust_decimal::Decimal>,
    avg_selection_latency_ms: Option<rust_decimal::Decimal>,
    avg_stage1_candidates: Option<rust_decimal::Decimal>,
    avg_stage2_candidates: Option<rust_decimal::Decimal>,
    date: Option<String>,
}

// ─── Upload Handler ───────────────────────────────────────────────────────────

/// `POST /api/a2a/upload` — multipart/form-data A2A dispatch entry point.
///
/// Accepts:
/// - `query`   (text field, required) — the user's question
/// - Any number of additional fields treated as file attachments
///
/// Each file is base64-encoded and forwarded alongside the query to the orchestrator.
pub async fn a2a_upload_handler(
    State(state): State<AppState>,
    claims: Claims,
    mut multipart: Multipart,
) -> Result<Response, A2aDispatchError> {
    let mut query = String::new();
    let mut file_count = 0usize;

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| A2aDispatchError::InvalidRequest(format!("multipart error: {e}")))?
    {
        let field_name = field.name().unwrap_or("").to_string();

        let bytes = field
            .bytes()
            .await
            .map_err(|e| A2aDispatchError::InvalidRequest(format!("field read error: {e}")))?;

        if field_name == "query" {
            query = String::from_utf8(bytes.to_vec())
                .map_err(|_| A2aDispatchError::InvalidRequest("query must be valid UTF-8".into()))?;
        } else {
            file_count += 1;
        }
    }

    if query.trim().is_empty() {
        return Err(A2aDispatchError::InvalidRequest(
            "multipart must include a non-empty 'query' text field".into(),
        ));
    }

    let task_id = Uuid::new_v4().to_string();
    let context_id = Uuid::new_v4().to_string();
    let user_id: Uuid = claims.sub.parse().unwrap_or(Uuid::nil());

    tracing::info!(
        user_id = %user_id,
        file_count,
        "a2a dispatch upload: orchestrating query with {} file(s)",
        file_count
    );

    orchestrator_stream(&state, &query, &task_id, &context_id, user_id, claims.is_superuser).await
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

fn to_sse(event: StreamResponse) -> Event {
    Event::default().data(a2a::to_sse_data(&event))
}

async fn resolve_endpoint(state: &AppState, agent_name: &str) -> Result<String, String> {
    // Prefer live runtime endpoint (Docker port mapping can change on restart).
    let container_id = nasiko_runtime::ContainerId::new(agent_name.to_owned());
    match state.runtime.endpoint(&container_id).await {
        Ok(endpoint) => {
            let base = endpoint.trim_end_matches('/');
            return Ok(format!("{base}/jsonrpc"));
        }
        Err(_) => {
            // Container not reachable via runtime — check if it's actually stopped.
            if let Ok(status) = state.runtime.status(&container_id).await {
                if status.state != nasiko_runtime::RuntimeState::Running {
                    // Mark as stopped so future routing skips it.
                    let _ = sqlx::query(
                        "UPDATE agents SET status = 'stopped' WHERE name = $1 AND status = 'running'",
                    )
                    .bind(agent_name)
                    .execute(&state.db)
                    .await;
                    return Err(format!("agent '{agent_name}' is not running"));
                }
            }
        }
    }

    // Fall back to stored URL (e.g. external agents, K8s with stable DNS).
    let stored_url: Option<String> = sqlx::query_scalar(
        "SELECT url FROM agents WHERE name = $1 AND status = 'running'",
    )
    .bind(agent_name)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| format!("db lookup: {e}"))?
    .flatten();

    if let Some(ref url) = stored_url
        && !url.is_empty() {
            let u = url.trim_end_matches('/');
            return Ok(format!("{u}/jsonrpc"));
        }

    Err(format!("no endpoint found for agent '{agent_name}'"))
}

/// Normalize a Python a2a-sdk JSONRPC event to CP native StreamResponse format.
fn normalize_agent_event(data: &str, task_id: &str, context_id: &str) -> String {
    let Ok(parsed) = serde_json::from_str::<serde_json::Value>(data) else {
        return data.to_string();
    };

    // Already in the expected format (no JSON-RPC wrapper)
    if parsed.get("statusUpdate").is_some() || parsed.get("artifactUpdate").is_some()
        || parsed.get("task").is_some() || parsed.get("message").is_some()
    {
        return data.to_string();
    }

    let Some(result) = parsed.get("result") else {
        return data.to_string();
    };

    let kind = result.get("kind").and_then(|k| k.as_str()).unwrap_or("");

    match kind {
        "artifact-update" => {
            let artifact = result.get("artifact").cloned().unwrap_or(json!({}));
            let append = result.get("append").and_then(|a| a.as_bool()).unwrap_or(false);
            let last_chunk = result.get("final").and_then(|f| f.as_bool()).unwrap_or(false);

            let normalized = json!({
                "artifactUpdate": {
                    "taskId": task_id,
                    "contextId": context_id,
                    "artifact": artifact,
                    "append": append,
                    "lastChunk": last_chunk,
                }
            });
            serde_json::to_string(&normalized).unwrap_or_else(|_| data.to_string())
        }
        "status-update" => {
            let status = result.get("status").cloned().unwrap_or(json!({}));
            let normalized = json!({
                "statusUpdate": {
                    "taskId": task_id,
                    "contextId": context_id,
                    "status": status,
                }
            });
            serde_json::to_string(&normalized).unwrap_or_else(|_| data.to_string())
        }
        _ => data.to_string(),
    }
}

// ─── Types & Errors ──────────────────────────────────────────────────────────

#[derive(sqlx::FromRow)]
#[allow(dead_code)]
struct AgentRow {
    id: Uuid,
    name: String,
    status: String,
}

#[derive(Debug)]
pub enum A2aDispatchError {
    InvalidRequest(String),
    NoAgents,
    Forbidden,
    AgentNotFound(String),
    Internal(String),
}

impl IntoResponse for A2aDispatchError {
    fn into_response(self) -> Response {
        let (status, code, message) = match self {
            Self::InvalidRequest(e) => (StatusCode::BAD_REQUEST, -32602, e),
            Self::NoAgents => (
                StatusCode::SERVICE_UNAVAILABLE,
                -32603,
                "no agents available".into(),
            ),
            Self::Forbidden => (StatusCode::FORBIDDEN, -32605, "access to this agent is not permitted".into()),
            Self::AgentNotFound(name) => (
                StatusCode::NOT_FOUND,
                -32604,
                format!("agent '{}' not found or not running", name),
            ),
            Self::Internal(e) => (StatusCode::INTERNAL_SERVER_ERROR, -32603, e),
        };

        let body = Json(json!({
            "jsonrpc": "2.0",
            "id": null,
            "error": { "code": code, "message": message }
        }));

        (status, body).into_response()
    }
}
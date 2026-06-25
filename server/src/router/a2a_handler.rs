use axum::{
    extract::State,
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
use uuid::Uuid;

use nasiko_types::a2a::{self as a2a, JsonRpcRequest, PartContent, StreamResponse};
use nasiko_react_agent::{
    AgentInfo, AgentSkill as OrcAgentSkill, Orchestrator, OrchestratorConfig, OrchestratorEvent,
    RegistrySource,
};

use crate::acl::CpCallGuard;
use crate::auth::Claims;
use crate::flow::FlowContext;
use crate::state::AppState;

use super::selector::AgentSelector;

// TODO: Implement `AgentExecutor` trait from a2a-server-lf to replace manual Axum routing.
// The trait has two methods: execute(&self, ctx: ExecutorContext) -> BoxStream<StreamResponse>
// and cancel(&self, ctx: ExecutorContext) -> BoxStream<StreamResponse>.
// Our handler already returns a stream of StreamResponse — wrap it in the trait impl and let
// a2a-server handle JSON-RPC envelope, SSE serialization, and /.well-known/agent-card.json.

/// Client-facing A2A endpoint. Accepts JSONRPC `message/send` or `message/stream`.
/// Routes to the ReAct orchestrator or a specific agent based on `agentId` in the message.
pub async fn a2a_handler(
    State(state): State<AppState>,
    claims: Claims,
    Json(req): Json<JsonRpcRequest>,
) -> Result<Response, A2aHandlerError> {
    let params: nasiko_types::a2a::SendMessageRequest = serde_json::from_value(
        req.params.clone().ok_or_else(|| A2aHandlerError::InvalidRequest("missing params".into()))?,
    )
    .map_err(|e| A2aHandlerError::InvalidRequest(format!("bad params: {e}")))?;

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
        return Err(A2aHandlerError::InvalidRequest(
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
    let is_orchestrator = agent_id.as_deref() == Some("orchestrator") || agent_id.is_none();

    let user_id: Uuid = claims.sub.parse().unwrap_or(Uuid::nil());

    // Fetch chat history from session if provided
    let history = if let Some(ref sid) = session_id {
        fetch_session_history(&state.db, sid).await
    } else {
        Vec::new()
    };

    // Build full query with history context
    let query = if history.is_empty() {
        text
    } else {
        let hist_text = history
            .iter()
            .map(|m| format!("{}: {}", m.0, m.1))
            .collect::<Vec<_>>()
            .join("\n");
        format!("{}\n\nCurrent message: {}", hist_text, text)
    };

    if is_orchestrator {
        orchestrator_stream(&state, &query, &task_id, &context_id, user_id).await
    } else {
        agent_stream(&state, agent_id.as_deref().unwrap(), &query, &task_id, &context_id, user_id).await
    }
}

// ─── Orchestrator Path ───────────────────────────────────────────────────────

async fn orchestrator_stream(
    state: &AppState,
    query: &str,
    task_id: &str,
    context_id: &str,
    user_id: Uuid,
) -> Result<Response, A2aHandlerError> {
    let agent_summaries = AgentSelector::fetch_active_agents(&state.db)
        .await
        .map_err(|e| A2aHandlerError::Internal(e.to_string()))?;


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
        return Err(A2aHandlerError::NoAgents);
    }

    let config = OrchestratorConfig {
        model: std::env::var("OPENAI_MODEL").unwrap_or_else(|_| "deepseek-v4-flash".into()),
        base_url: std::env::var("OPENAI_BASE_URL").ok(),
        api_key: std::env::var("OPENAI_API_KEY").ok(),
        max_turns: 10,
        temperature: Some(0.2),
        ..Default::default()
    };

    // Build call guard: enforces ACL + flow limits on every agent invocation
    let flow_ctx = FlowContext::new_root();
    let flow_id = flow_ctx.flow_id.clone();
    let traceparent = flow_ctx.to_traceparent();
    state.flow_guard.init_flow(&flow_ctx, "orchestrator").await;

    // Persist flow to Postgres for the Flows UI page
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

    // Create A2A client with traceparent header for OTel propagation
    let a2a_client = nasiko_react_agent::A2aClient::new()
        .with_headers(vec![("traceparent".to_string(), traceparent)]);

    let mut orchestrator = Orchestrator::new(config, RegistrySource::Static(agents))
        .with_a2a_client(a2a_client)
        .with_guard(guard);
    orchestrator
        .init()
        .await
        .map_err(|e| A2aHandlerError::Internal(e.to_string()))?;

    let mut rx = orchestrator.run_stream(query);
    let task_id = task_id.to_string();
    let context_id = context_id.to_string();
    let artifact_id = Uuid::new_v4().to_string();
    let flow_events = state.flow_events.clone();
    let mut flow_rx = state.flow_events.subscribe(&flow_id).await;
    let flow_id_cleanup = flow_id.clone();
    let db = state.db.clone();

    let stream = async_stream::stream! {
        yield Ok::<_, Infallible>(to_sse(a2a::status_event(a2a::working(&task_id, &context_id))));

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
                            // Persist step to DB
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

                            let msg = a2a::agent_message(&context_id, &task_id, a2a::data_part(json!({
                                    "type": "tool_call",
                                    "agent": agent,
                                    "message": message,
                                    "turn": turn,
                                })));
                            yield Ok(to_sse(a2a::status_event(a2a::working_with_message(&task_id, &context_id, msg))));
                        }
                        OrchestratorEvent::ToolResult { agent, result, success, turn } => {
                            // Update step in DB
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
                        OrchestratorEvent::Content { content } => {
                            yield Ok(to_sse(a2a::artifact_event(a2a::text_chunk(
                                    &task_id, &context_id, &artifact_id, &content, content_started, true,
                                ))));
                            content_started = true;
                        }
                        OrchestratorEvent::Done { .. } => {
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

        // Persist flow completion to Postgres
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
) -> Result<Response, A2aHandlerError> {
    let agent = sqlx::query_as::<_, AgentRow>(
        "SELECT id, name, status FROM agents WHERE (id::text = $1 OR name = $1) AND status = 'running'",
    )
    .bind(target)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| A2aHandlerError::Internal(e.to_string()))?
    .ok_or_else(|| A2aHandlerError::AgentNotFound(target.to_string()))?;

    let endpoint = resolve_endpoint(state, &agent.name)
        .await
        .map_err(A2aHandlerError::Internal)?;

    // Create a flow context and set traceparent so OTel propagates it
    let flow_ctx = FlowContext::new_root();
    let flow_id = flow_ctx.flow_id.clone();
    state.flow_guard.init_flow(&flow_ctx, &agent.name).await;

    // Persist flow to Postgres
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
        .header("traceparent", flow_ctx.to_traceparent())
        .json(&req_body)
        .send()
        .await
        .map_err(|e| A2aHandlerError::Internal(format!("agent request failed: {e}")))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(A2aHandlerError::Internal(format!(
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
        // True SSE streaming — read chunks and re-emit, merged with flow bus events
        let byte_stream = response.bytes_stream();
        let flow_id_cleanup = flow_id;

        let stream = async_stream::stream! {
            yield Ok::<_, Infallible>(to_sse(a2a::status_event(a2a::working(&task_id, &context_id))));

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
        // Non-streaming JSON response — wrap as A2A stream
        let resp_body: serde_json::Value = response
            .json()
            .await
            .map_err(|e| A2aHandlerError::Internal(format!("invalid agent JSON: {e}")))?;

        let text = nasiko_types::a2a::extract_text(
            resp_body.get("result").unwrap_or(&resp_body),
        )
        .unwrap_or_else(|| "No response".into());

        let artifact_id = Uuid::new_v4().to_string();
        let flow_id_cleanup = flow_id;

        let stream = async_stream::stream! {
            yield Ok::<_, Infallible>(to_sse(a2a::status_event(a2a::working(&task_id, &context_id))));

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

// ─── Helpers ─────────────────────────────────────────────────────────────────

fn to_sse(event: StreamResponse) -> Event {
    Event::default().data(a2a::to_sse_data(&event))
}

async fn resolve_endpoint(state: &AppState, agent_name: &str) -> Result<String, String> {
    // Check the DB-stored URL first (set during deploy, works in Docker networks)
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
            return Ok(format!("{u}/"));
        }

    // Fallback: ask runtime for the endpoint (works in dev where CP runs on host)
    let container_id = nasiko_runtime::ContainerId::new(agent_name);
    let endpoint = state
        .runtime
        .endpoint(&container_id)
        .await
        .map_err(|e| format!("runtime endpoint: {e}"))?;

    let e = endpoint.trim_end_matches('/');
    Ok(format!("{e}/"))
}

/// Fetch prior messages from a chat session for context.
/// Returns Vec<(role, content)> in chronological order.
async fn fetch_session_history(db: &sqlx::PgPool, session_id: &str) -> Vec<(String, String)> {
    sqlx::query_as(
        "SELECT role, content FROM chat_messages WHERE session_id = $1 ORDER BY timestamp ASC LIMIT 20",
    )
    .bind(session_id)
    .fetch_all(db)
    .await
    .unwrap_or_default()
}

/// Normalize a Python a2a-sdk JSONRPC event to CP native StreamResponse format.
/// Input: `{"jsonrpc":"2.0","result":{"kind":"artifact-update","artifact":{...},"append":true,...}}`
/// Output: `{"artifactUpdate":{"taskId":"...","artifact":{...},"append":true,...}}`
/// If not JSONRPC format, returns the data unchanged.
fn normalize_agent_event(data: &str, task_id: &str, context_id: &str) -> String {
    let Ok(parsed) = serde_json::from_str::<serde_json::Value>(data) else {
        return data.to_string();
    };

    // Already in CP format
    if parsed.get("statusUpdate").is_some() || parsed.get("artifactUpdate").is_some() {
        return data.to_string();
    }

    // Python a2a-sdk JSONRPC format
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
pub enum A2aHandlerError {
    InvalidRequest(String),
    NoAgents,
    AgentNotFound(String),
    Internal(String),
}

impl IntoResponse for A2aHandlerError {
    fn into_response(self) -> Response {
        let (status, code, message) = match self {
            Self::InvalidRequest(e) => (StatusCode::BAD_REQUEST, -32602, e),
            Self::NoAgents => (
                StatusCode::SERVICE_UNAVAILABLE,
                -32603,
                "no agents available".into(),
            ),
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

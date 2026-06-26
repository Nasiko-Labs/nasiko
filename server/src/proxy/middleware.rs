use axum::{
    body::Body,
    extract::{Request, State},
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
};
use http_body_util::BodyExt;
use sqlx::PgPool;
use std::time::Instant;
use tracing::{Instrument, warn};
use uuid::Uuid;

use crate::auth::Claims;
use crate::flow::{FlowContext, FlowEvent};
use crate::state::AppState;

use super::models::ProxyCallLog;

/// Middleware that intercepts calls to `/agents/{id}/*` and proxies them to the actual container.
/// Flow tracking uses W3C Trace Context (traceparent header) — auto-propagated by OTel.
pub async fn agent_proxy_middleware(
    State(state): State<AppState>,
    claims: Claims,
    req: Request,
    next: Next,
) -> Result<Response, ProxyError> {
    let uri = req.uri().clone();
    let path = uri.path();

    if !path.starts_with("/agents/") {
        return Ok(next.run(req).await);
    }

    let segments: Vec<&str> = path.trim_start_matches("/agents/").split('/').collect();
    if segments.is_empty() {
        return Ok(next.run(req).await);
    }

    let agent_id = match Uuid::parse_str(segments[0]) {
        Ok(id) => id,
        Err(_) => return Ok(next.run(req).await),
    };

    let forwarded_path = if segments.len() > 1 {
        format!("/{}", segments[1..].join("/"))
    } else {
        "/".to_string()
    };

    let forwarded_path = if let Some(query) = uri.query() {
        format!("{}?{}", forwarded_path, query)
    } else {
        forwarded_path
    };

    let start = Instant::now();

    // 1. Access control
    let caller_id: Uuid = claims.sub.parse().map_err(|_| ProxyError::AccessDenied)?;
    if !claims.is_superuser
        && !crate::acl::user_can_access_agent(&state.db, caller_id, agent_id).await {
            return Err(ProxyError::AccessDenied);
        }

    // 2. Extract flow context from traceparent header (auto-propagated by OTel)
    let flow_ctx = FlowContext::from_headers(req.headers())
        .ok_or(ProxyError::MissingFlowContext)?;

    let agent_id_str = agent_id.to_string();

    // 3. Check flow limits (all state in Redis, keyed by trace_id)
    if let Err(rejection) = state.flow_guard.check(&flow_ctx, &agent_id_str).await {
        warn!(%rejection, agent_id = %agent_id_str, "flow cascade rejected");
        state.genai_metrics.record_cascade_rejection(
            &format!("{:?}", rejection),
            &agent_id_str,
        );
        return Err(ProxyError::CascadeRejected(rejection.to_string()));
    }

    // 4. Record invocation (increment depth, fan-out, append to call_chain)
    if let Err(rejection) = state.flow_guard.record_invocation(&flow_ctx, &agent_id_str).await {
        warn!(%rejection, "flow limit hit");
        return Err(ProxyError::CascadeRejected(rejection.to_string()));
    }

    state.genai_metrics.record_invocation(&agent_id_str, &claims.team_id.clone().unwrap_or_default());

    // 5. Resolve agent → container
    let (node_ip, port, agent_name) = resolve_agent_endpoint(&state, agent_id).await?;

    // Publish flow event for live UI updates
    state.flow_events.publish(
        &flow_ctx.flow_id,
        FlowEvent::AgentInvoke {
            caller_agent: "agent".to_string(),
            target_agent: agent_name.clone(),
            depth: 0, // depth tracked in Redis, not needed here for the UI event
        },
    ).await;

    // 6. Forward request with traceparent (already present in headers — OTel propagates)
    let target_url = format!("http://{}:{}{}", node_ip, port, forwarded_path);
    let (parts, body) = req.into_parts();

    let body_bytes = body
        .collect()
        .await
        .map_err(|e| ProxyError::UpstreamError(format!("failed to read request body: {}", e)))?
        .to_bytes();

    let mut forwarded_req = state.http_client.request(parts.method.clone(), &target_url);

    for (name, value) in parts.headers.iter() {
        if name != "host" && name != "content-length"
            && let Ok(val_str) = value.to_str() {
                forwarded_req = forwarded_req.header(name.as_str(), val_str);
            }
    }

    if !body_bytes.is_empty() {
        forwarded_req = forwarded_req.body(body_bytes.to_vec());
    }

    let proxy_span = tracing::info_span!(
        "a2a.agent_proxy",
        agent.id  = %agent_id_str,
        agent.name = %agent_name,
        http.method = %parts.method,
        http.target = %forwarded_path,
        http.url    = %target_url,
    );

    let response = forwarded_req
        .send()
        .instrument(proxy_span)
        .await
        .map_err(|e| ProxyError::UpstreamError(e.to_string()))?;

    let status = response.status();
    let headers = response.headers().clone();
    let is_sse = headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .map(|ct| ct.contains("text/event-stream"))
        .unwrap_or(false);

    // 7. Record return (decrement depth) — for SSE we record immediately since we can't wait
    state.flow_guard.record_return(&flow_ctx).await;

    // Publish flow event: agent call completed
    state.flow_events.publish(
        &flow_ctx.flow_id,
        FlowEvent::AgentResult {
            caller_agent: "agent".to_string(),
            target_agent: agent_name,
            depth: 0,
            success: status.is_success(),
            latency_ms: start.elapsed().as_millis() as u64,
        },
    ).await;

    // Record metrics
    let team_id = claims.team_id.as_deref().unwrap_or("");
    state.genai_metrics.record_operation(
        start.elapsed().as_secs_f64(),
        "agent_call",
        "a2a",
        &agent_id_str,
        team_id,
    );

    // 8. Log interaction (fire-and-forget)
    let log = ProxyCallLog {
        caller_id,
        target_agent_id: agent_id,
        method: parts.method.to_string(),
        timestamp: chrono::Utc::now(),
        latency_ms: start.elapsed().as_millis() as u64,
        status: status.as_u16(),
        error: if status.is_server_error() || status.is_client_error() {
            Some(format!("HTTP {}", status.as_u16()))
        } else {
            None
        },
    };
    tokio::spawn(log_proxy_call(state.db.clone(), log));

    // 9. Return response — stream SSE through, buffer regular responses
    let mut response_builder = axum::response::Response::builder().status(status);
    for (name, value) in headers.iter() {
        response_builder = response_builder.header(name, value);
    }

    if is_sse {
        let stream = response.bytes_stream();
        let body = Body::from_stream(stream);
        // Remove content-length to ensure chunked transfer for SSE
        let mut resp = response_builder
            .body(body)
            .unwrap_or_else(|_| Response::new(Body::empty()));
        resp.headers_mut().remove("content-length");
        Ok(resp)
    } else {
        let response_bytes = response
            .bytes()
            .await
            .map_err(|e| ProxyError::UpstreamError(e.to_string()))?;
        Ok(response_builder
            .body(Body::from(response_bytes))
            .unwrap_or_else(|_| Response::new(Body::empty())))
    }
}

async fn resolve_agent_endpoint(state: &AppState, agent_id: Uuid) -> Result<(String, u16, String), ProxyError> {
    let agent = sqlx::query_as::<_, AgentRow>(
        "SELECT name, status, url FROM agents WHERE id = $1",
    )
    .bind(agent_id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| ProxyError::DatabaseError(e.to_string()))?
    .ok_or(ProxyError::AgentNotFound)?;

    if agent.status != "running" {
        return Err(ProxyError::AgentNotRunning);
    }

    // Use stored URL if available (works in Docker where container names resolve)
    if let Some(ref url) = agent.url
        && !url.is_empty() {
            let stripped = url.trim_start_matches("http://").trim_start_matches("https://");
            let host_port = stripped.split('/').next().unwrap_or(stripped);
            let (host, port) = if let Some((h, p)) = host_port.rsplit_once(':') {
                (h.to_string(), p.parse::<u16>().unwrap_or(8000))
            } else {
                (host_port.to_string(), 8000)
            };
            return Ok((host, port, agent.name));
        }

    // Fallback: ask runtime for the endpoint (works in dev where CP runs on host)
    let container_id = nasiko_runtime::ContainerId::new(agent.name.clone());
    let endpoint = state
        .runtime
        .endpoint(&container_id)
        .await
        .map_err(|e| ProxyError::OrchestratorError(e.to_string()))?;

    // Parse host:port from the endpoint URL
    let stripped = endpoint.trim_start_matches("http://").trim_start_matches("https://");
    let host_port = stripped.split('/').next().unwrap_or(stripped);
    let (host, port) = if let Some((h, p)) = host_port.rsplit_once(':') {
        (h.to_string(), p.parse::<u16>().unwrap_or(8000))
    } else {
        (host_port.to_string(), 8000)
    };

    Ok((host, port, agent.name))
}


async fn log_proxy_call(db: PgPool, log: ProxyCallLog) {
    let log_json = serde_json::to_value(&log).unwrap_or_default();
    if let Err(e) = sqlx::query(
        r#"INSERT INTO proxy_logs (caller_id, target_agent_id, method, timestamp, latency_ms, status, error, metadata)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8)"#,
    )
    .bind(log.caller_id)
    .bind(log.target_agent_id)
    .bind(&log.method)
    .bind(log.timestamp)
    .bind(log.latency_ms as i64)
    .bind(log.status as i32)
    .bind(&log.error)
    .bind(log_json)
    .execute(&db)
    .await
    {
        warn!(caller_id = %log.caller_id, target = %log.target_agent_id, %e, "failed to write proxy audit log");
    }
}

#[derive(sqlx::FromRow)]
struct AgentRow {
    name: String,
    status: String,
    url: Option<String>,
}

#[derive(Debug)]
pub enum ProxyError {
    AgentNotFound,
    AgentNotRunning,
    AccessDenied,
    MissingFlowContext,
    CascadeRejected(String),
    UpstreamError(String),
    DatabaseError(String),
    OrchestratorError(String),
}

impl IntoResponse for ProxyError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            ProxyError::AgentNotFound => (StatusCode::NOT_FOUND, "Agent not found".to_string()),
            ProxyError::AgentNotRunning => (StatusCode::SERVICE_UNAVAILABLE, "Agent not running".to_string()),
            ProxyError::AccessDenied => (StatusCode::FORBIDDEN, "Access denied".to_string()),
            ProxyError::MissingFlowContext => (StatusCode::BAD_REQUEST, "Missing traceparent header — agents must have OTel instrumentation enabled".to_string()),
            ProxyError::CascadeRejected(reason) => (StatusCode::LOOP_DETECTED, format!("Cascade rejected: {}", reason)),
            ProxyError::UpstreamError(e) => (StatusCode::BAD_GATEWAY, format!("Upstream error: {}", e)),
            ProxyError::DatabaseError(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("Database error: {}", e)),
            ProxyError::OrchestratorError(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("Orchestrator error: {}", e)),
        };

        (status, message).into_response()
    }
}

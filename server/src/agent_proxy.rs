use std::collections::HashMap;

use axum::{
    body::Body,
    extract::{Path, Request, State},
    http::StatusCode,
    response::Response,
};
use nasiko_flow::{FlowContext, TRACEPARENT_HEADER};
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

    // Build the forwarded path: everything after /api/agents/{id}
    let full_path = req.uri().path();
    let id_str = agent_id.to_string();
    let forwarded_path = full_path
        .find(&id_str)
        .map(|pos| {
            let after_id = &full_path[pos + id_str.len()..];
            if after_id.is_empty() { "/".to_string() } else { after_id.to_string() }
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
    let flow_ctx = traceparent
        .as_deref()
        .and_then(FlowContext::from_traceparent)
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

    // Resolve agent container endpoint. `nasiko_agent_proxy::resolve` reads the
    // `agents.url` column, a snapshot taken at the last deploy/restart — stale
    // the moment the container is recreated outside that flow (Docker/Podman
    // assign a new random host port on every recreate). Prefer the live
    // runtime lookup instead (same fix already applied in
    // `resolve_endpoint` in `router/a2a_dispatch.rs`), falling back to the
    // stored value only if the runtime can't be reached (e.g. external agents
    // registered by URL rather than deployed through this platform).
    let stored = nasiko_agent_proxy::resolve(&state.db, agent_id)
        .await
        .map_err(|e| match e {
            nasiko_agent_proxy::ResolveError::NotFound => StatusCode::NOT_FOUND,
            nasiko_agent_proxy::ResolveError::NotRunning(_) => StatusCode::SERVICE_UNAVAILABLE,
            nasiko_agent_proxy::ResolveError::NoEndpoint => StatusCode::BAD_GATEWAY,
            nasiko_agent_proxy::ResolveError::Database(_) => StatusCode::INTERNAL_SERVER_ERROR,
        })?;

    let target_url = match state
        .runtime
        .endpoint(&nasiko_runtime::ContainerId::from_uuid(agent_id))
        .await
    {
        Ok(live) => format!("{}{}", live.trim_end_matches('/'), forwarded_path),
        Err(e) => {
            tracing::warn!(
                error = %e, %agent_id,
                "agent proxy: live endpoint lookup failed, falling back to stored agents.url"
            );
            format!("http://{}:{}{}", stored.host, stored.port, forwarded_path)
        }
    };

    // Forward the request
    let method = req.method().clone();
    let headers = req.headers().clone();
    let body_bytes = axum::body::to_bytes(req.into_body(), 10 * 1024 * 1024)
        .await
        .map_err(|_| StatusCode::BAD_REQUEST)?;

    // Explicit allowlist, not a denylist: the agent container is unvetted, so
    // anything not named here is dropped rather than forwarded by default.
    // In particular this drops `authorization`/`cookie` (the caller's platform
    // credentials — a hostile agent could otherwise replay them against
    // `/api/*`) and any inbound `x-user-id`/`x-username`/`x-is-superuser`
    // (spoofed identity — reqwest's `.header()` below is `HeaderMap::append`,
    // not replace, so a copied attacker value would sit ahead of the trusted
    // one most header readers return the first occurrence of).
    const FORWARDED_HEADERS: &[&str] = &["content-type", "accept", "accept-encoding", "accept-language", "a2a-version"];
    let mut forwarded = state.http_client.request(method, &target_url);
    for (name, value) in headers.iter() {
        if FORWARDED_HEADERS.contains(&name.as_str())
            && let Ok(val_str) = value.to_str()
        {
            forwarded = forwarded.header(name.as_str(), val_str);
        }
    }

    // Propagate trace context and caller identity to the agent
    forwarded = forwarded
        .header("traceparent", flow_ctx.to_traceparent())
        .header("x-user-id", &claims.sub)
        .header("x-username", &claims.username)
        .header(
            "x-is-superuser",
            if claims.is_superuser { "true" } else { "false" },
        );

    if !body_bytes.is_empty() {
        forwarded = forwarded.body(body_bytes);
    }

    let response = forwarded.send().await.map_err(|e| {
        tracing::error!(error = %e, %agent_id, %target_url, "agent proxy: request to agent failed");
        StatusCode::BAD_GATEWAY
    })?;

    state.flow_guard.record_return(&flow_ctx).await;

    to_axum_response(response, agent_id).await
}

async fn to_axum_response(response: reqwest::Response, agent_id: Uuid) -> Result<Response, StatusCode> {
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
        let stream = response.bytes_stream();
        builder
            .body(Body::from_stream(stream))
            .map_err(|e| {
                tracing::error!(error = %e, %agent_id, "agent proxy: failed to build streamed response");
                StatusCode::INTERNAL_SERVER_ERROR
            })
    } else {
        let bytes = response.bytes().await.map_err(|e| {
            tracing::error!(error = %e, %agent_id, "agent proxy: failed to read agent response body");
            StatusCode::BAD_GATEWAY
        })?;
        builder
            .body(Body::from(bytes))
            .map_err(|e| {
                tracing::error!(error = %e, %agent_id, "agent proxy: failed to build response");
                StatusCode::INTERNAL_SERVER_ERROR
            })
    }
}

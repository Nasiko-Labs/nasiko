use axum::{
    body::Body,
    extract::{Request, State},
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
pub async fn agent_proxy(
    State(state): State<AppState>,
    req: Request,
) -> Result<Response, StatusCode> {
    let claims = req
        .extensions()
        .get::<Claims>()
        .cloned()
        .ok_or(StatusCode::UNAUTHORIZED)?;

    let path = req.uri().path().to_string();
    let (agent_id, forwarded_path) = parse_agent_path(&path).ok_or(StatusCode::NOT_FOUND)?;

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

    // Resolve agent container endpoint from DB
    let endpoint = nasiko_agent_proxy::resolve(&state.db, agent_id)
        .await
        .map_err(|e| match e {
            nasiko_agent_proxy::ResolveError::NotFound => StatusCode::NOT_FOUND,
            nasiko_agent_proxy::ResolveError::NotRunning(_) => StatusCode::SERVICE_UNAVAILABLE,
            nasiko_agent_proxy::ResolveError::NoEndpoint => StatusCode::BAD_GATEWAY,
            nasiko_agent_proxy::ResolveError::Database(_) => StatusCode::INTERNAL_SERVER_ERROR,
        })?;

    let target_url = format!("http://{}:{}{}", endpoint.host, endpoint.port, forwarded_path);

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
    const FORWARDED_HEADERS: &[&str] = &["content-type", "accept", "accept-encoding", "accept-language"];
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

    // Mint a short-lived delegation token so the agent can call back into
    // `/api/mcp` proving "I am agent_id_str, acting for claims.sub". Minting
    // is best-effort: if JWT_SECRET is unset, MCP delegation is simply
    // unavailable to this agent rather than failing the whole proxy call.
    if let Ok(jwt_secret) = std::env::var("JWT_SECRET")
        && let Ok(delegation_token) =
            nasiko_auth::jwt::mint_delegation_token(&jwt_secret, &claims.sub, &agent_id_str)
    {
        forwarded = forwarded.header("x-nasiko-agent-token", delegation_token);
    }

    if !body_bytes.is_empty() {
        forwarded = forwarded.body(body_bytes);
    }

    let response = forwarded
        .send()
        .await
        .map_err(|_| StatusCode::BAD_GATEWAY)?;

    state.flow_guard.record_return(&flow_ctx).await;

    to_axum_response(response).await
}

async fn to_axum_response(response: reqwest::Response) -> Result<Response, StatusCode> {
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
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
    } else {
        let bytes = response.bytes().await.map_err(|_| StatusCode::BAD_GATEWAY)?;
        builder
            .body(Body::from(bytes))
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
    }
}

/// Parse `/agents/{uuid}/{*rest}` from the path seen inside the `/api` nest.
///
/// Axum strips the `/api` prefix when dispatching to nested routers, so the
/// handler sees `/agents/{uuid}/...` not `/api/agents/{uuid}/...`.
fn parse_agent_path(path: &str) -> Option<(Uuid, String)> {
    let rest = path.strip_prefix("/agents/")?;
    let (id_str, remainder) = rest.split_once('/').unwrap_or((rest, ""));
    let agent_id = Uuid::parse_str(id_str).ok()?;
    let forwarded = if remainder.is_empty() {
        "/".to_string()
    } else {
        format!("/{remainder}")
    };
    Some((agent_id, forwarded))
}
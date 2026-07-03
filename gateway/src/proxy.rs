use axum::{
    body::Body,
    extract::{Request, State},
    http::StatusCode,
    response::Response,
};
use nasiko_auth::{
    Identity, HEADER_IS_SUPERUSER, HEADER_USER_ID,
    HEADER_USER_ROLE, HEADER_USERNAME, TRUST_HEADERS,
};
use uuid::Uuid;

use nasiko_flow::{FlowContext, TRACEPARENT_HEADER};

use crate::state::GatewayState;

/// Proxy agent requests: auth → flow guard → resolve → forward to agent container.
pub async fn agent_proxy(
    State(state): State<GatewayState>,
    req: Request,
) -> Result<Response, StatusCode> {
    let identity = req
        .extensions()
        .get::<Identity>()
        .cloned()
        .ok_or(StatusCode::UNAUTHORIZED)?;

    let path = req.uri().path().to_string();
    let (agent_id, forwarded_path) = parse_agent_path(&path).ok_or(StatusCode::NOT_FOUND)?;

    // Flow guard
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

    // Resolve agent endpoint
    let endpoint =
        nasiko_agent_proxy::resolve(&state.db, agent_id)
            .await
            .map_err(|e| match e {
                nasiko_agent_proxy::ResolveError::NotFound => StatusCode::NOT_FOUND,
                nasiko_agent_proxy::ResolveError::NotRunning(_) => StatusCode::SERVICE_UNAVAILABLE,
                nasiko_agent_proxy::ResolveError::NoEndpoint => StatusCode::BAD_GATEWAY,
                nasiko_agent_proxy::ResolveError::Database(_) => StatusCode::INTERNAL_SERVER_ERROR,
            })?;

    let target_url = format!("http://{}:{}{}", endpoint.host, endpoint.port, forwarded_path);

    // Forward request
    let method = req.method().clone();
    let headers = req.headers().clone();
    let body_bytes = axum::body::to_bytes(req.into_body(), 10 * 1024 * 1024)
        .await
        .map_err(|_| StatusCode::BAD_REQUEST)?;

    let mut forwarded = state.http_client.request(method, &target_url);
    for (name, value) in headers.iter() {
        if name == "host" || name == "content-length" || name == "traceparent" {
            continue;
        }
        if let Ok(val_str) = value.to_str() {
            forwarded = forwarded.header(name.as_str(), val_str);
        }
    }
    forwarded = forwarded
        .header("traceparent", flow_ctx.to_traceparent())
        .header(HEADER_USER_ID, &identity.user_id)
        .header(HEADER_USERNAME, &identity.username)
        .header(
            HEADER_IS_SUPERUSER,
            if identity.is_superuser { "true" } else { "false" },
        );
    if let Some(ref role) = identity.role
        && let Ok(v) = serde_json::to_value(role)
        && let Some(s) = v.as_str()
    {
        forwarded = forwarded.header(HEADER_USER_ROLE, s);
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

/// Reverse-proxy catch-all to the control plane server.
pub async fn server_proxy(
    State(state): State<GatewayState>,
    req: Request,
) -> Result<Response, StatusCode> {
    let identity = req.extensions().get::<Identity>().cloned();
    let method = req.method().clone();
    let uri = req.uri().clone();
    let mut headers = req.headers().clone();

    // Strip any client-supplied trust headers before forwarding — only the
    // gateway is allowed to set these on requests reaching the server.
    for h in TRUST_HEADERS {
        headers.remove(*h);
    }

    let target_url = format!(
        "{}{}",
        state.server_upstream.trim_end_matches('/'),
        uri.path_and_query().map(|pq| pq.as_str()).unwrap_or("/")
    );

    let body_bytes = axum::body::to_bytes(req.into_body(), 50 * 1024 * 1024)
        .await
        .map_err(|_| StatusCode::BAD_REQUEST)?;

    let mut forwarded = state.http_client.request(method, &target_url);
    for (name, value) in headers.iter() {
        if name == "host" || name == "content-length" {
            continue;
        }
        if let Ok(val_str) = value.to_str() {
            forwarded = forwarded.header(name.as_str(), val_str);
        }
    }

    if let Some(ref id) = identity {
        forwarded = forwarded
            .header(HEADER_USER_ID, &id.user_id)
            .header(HEADER_USERNAME, &id.username)
            .header(
                HEADER_IS_SUPERUSER,
                if id.is_superuser { "true" } else { "false" },
            );
        if let Some(ref role) = id.role
            && let Ok(v) = serde_json::to_value(role)
            && let Some(s) = v.as_str()
        {
            forwarded = forwarded.header(HEADER_USER_ROLE, s);
        }
    }

    if !body_bytes.is_empty() {
        forwarded = forwarded.body(body_bytes);
    }

    let response = forwarded
        .send()
        .await
        .map_err(|_| StatusCode::BAD_GATEWAY)?;

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

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

    let agent_base = match state
        .runtime
        .endpoint(&nasiko_runtime::ContainerId::from_uuid(agent_id))
        .await
    {
        Ok(live) => live.trim_end_matches('/').to_string(),
        Err(e) => {
            tracing::warn!(
                error = %e, %agent_id,
                "agent proxy: live endpoint lookup failed, falling back to stored agents.url"
            );
            format!("http://{}:{}", stored.host, stored.port)
        }
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
    let body_bytes = ensure_chat_session(&state, &claims, agent_id, &agent_base, body_bytes).await?;

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
            let agent_name = stored.name.clone();
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

    to_axum_response(response, agent_id).await
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
async fn ensure_chat_session(
    state: &AppState,
    claims: &Claims,
    agent_id: Uuid,
    agent_base_url: &str,
    body_bytes: axum::body::Bytes,
) -> Result<axum::body::Bytes, StatusCode> {
    // Not JSON, or not a message send → pass through; the agent is the
    // authority on whether the payload is valid A2A.
    let Ok(mut rpc) = serde_json::from_slice::<serde_json::Value>(&body_bytes) else {
        return Ok(body_bytes);
    };
    let is_send = rpc
        .get("method")
        .and_then(|m| m.as_str())
        .is_some_and(|m| MESSAGE_SEND_METHODS.contains(&m));
    if !is_send {
        return Ok(body_bytes);
    }
    let Some(message) = rpc.pointer_mut("/params/message") else {
        return Ok(body_bytes);
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
    // web UI uses when it titles a fresh session).
    let title = message
        .get("parts")
        .and_then(|p| p.as_array())
        .and_then(|parts| {
            parts
                .iter()
                .find_map(|p| p.get("text").and_then(|t| t.as_str()))
        })
        .map(|t| {
            let t = t.trim();
            if t.len() > 60 {
                let mut n = 60;
                while !t.is_char_boundary(n) {
                    n -= 1;
                }
                format!("{}…", &t[..n])
            } else {
                t.to_string()
            }
        })
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| "New chat".into());

    let inserted = sqlx::query(
        "INSERT INTO chat_sessions (session_id, user_id, agent_id, agent_url, title)
         VALUES ($1, $2, $3, $4, $5)
         ON CONFLICT (session_id) DO NOTHING",
    )
    .bind(&session_id)
    .bind(user_id)
    .bind(agent_id)
    .bind(agent_base_url)
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

    // Session already existed — it must belong to the caller, otherwise any
    // authenticated user could graft messages onto someone else's session by
    // guessing/replaying its contextId.
    if inserted.rows_affected() == 0 && !claims.is_superuser {
        let owner: Option<Uuid> =
            sqlx::query_scalar("SELECT user_id FROM chat_sessions WHERE session_id = $1")
                .bind(&session_id)
                .fetch_optional(&state.db)
                .await
                .map_err(|e| {
                    tracing::error!(error = %e, %session_id, "agent proxy: session owner lookup failed");
                    StatusCode::INTERNAL_SERVER_ERROR
                })?;
        if owner != Some(user_id) {
            return Err(StatusCode::FORBIDDEN);
        }
    }

    if injected {
        message["contextId"] = serde_json::Value::String(session_id);
        let rewritten = serde_json::to_vec(&rpc).map_err(|e| {
            tracing::error!(error = %e, "agent proxy: failed to re-serialize body after contextId injection");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
        return Ok(rewritten.into());
    }
    Ok(body_bytes)
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

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde_json::json;
use uuid::Uuid;

use crate::auth::Claims;
use crate::state::AppState;

use super::models::{JsonRpcRequest, JsonRpcResponse};

/// Public A2A discovery routes (no auth required — agents discover each other)
pub fn discovery_router() -> Router<AppState> {
    Router::new()
        .route("/.well-known/agent-card.json", get(registry_agent_card))
        .route("/a2a/v1", post(registry_jsonrpc))
}

/// Protected routes (require auth)
pub fn router() -> Router<AppState> {
    Router::new()
        // Per-agent A2A proxy (forward JSONRPC to the agent container)
        .route("/agents/{agent_id}/a2a", post(agent_a2a_proxy))
        // Agent card proxying (for external clients to query individual agent cards)
        .route(
            "/agents/{agent_id}/.well-known/agent-card.json",
            get(agent_card),
        )
}

/// Serve registry's own agent card (registry is itself an A2A agent)
async fn registry_agent_card() -> Json<serde_json::Value> {
    Json(json!({
        "name": "Nasiko Agent Registry",
        "description": "Discovers and lists agents by capability, tags, or natural language query",
        "version": "1.0.0",
        "protocolVersion": "0.2.9",
        "supportedInterfaces": [{
            "url": "/a2a/v1",
            "protocolBinding": "JSONRPC",
            "protocolVersion": "1.0"
        }],
        "capabilities": {
            "streaming": false,
            "pushNotifications": false,
            "stateTransitionHistory": false
        },
        "defaultInputModes": ["application/json", "text/plain"],
        "defaultOutputModes": ["application/json"],
        "skills": [
            {
                "id": "discover-by-capability",
                "name": "Discover Agents by Capability",
                "description": "Find agents that match given tags, capabilities, or natural language description",
                "tags": ["discovery", "registry", "a2a", "search"],
                "examples": [
                    "Find agents that can translate text",
                    "Which agents support streaming?",
                    "List all agents with tag: summarization"
                ],
                "inputModes": ["application/json", "text/plain"],
                "outputModes": ["application/json"]
            },
            {
                "id": "get-agent-card",
                "name": "Get Agent Card",
                "description": "Retrieve the full Agent Card for a specific agent by ID or name",
                "tags": ["discovery", "registry", "lookup"],
                "inputModes": ["application/json", "text/plain"],
                "outputModes": ["application/json"]
            },
            {
                "id": "list-agents",
                "name": "List All Agents",
                "description": "List all active agents with their skills and endpoints",
                "tags": ["discovery", "registry", "list"],
                "inputModes": ["application/json"],
                "outputModes": ["application/json"]
            }
        ]
    }))
}

/// Handle registry JSONRPC queries (discovery, search, get-card)
// TODO: add optional auth / API key for production; open for now to allow agent-to-agent discovery
async fn registry_jsonrpc(
    State(state): State<AppState>,
    Json(req): Json<JsonRpcRequest>,
) -> Result<Json<JsonRpcResponse>, ProxyError> {
    match req.method.as_str() {
        "message/send" | "SendMessage" => handle_registry_message(state, req).await,
        _ => Ok(Json(JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            id: req.id,
            result: None,
            error: Some(super::models::JsonRpcError {
                code: -32601,
                message: "Method not found".to_string(),
                data: None,
            }),
        })),
    }
}

/// Forward A2A JSONRPC request to the target agent container.
async fn agent_a2a_proxy(
    State(state): State<AppState>,
    _claims: Claims,
    Path(agent_id): Path<Uuid>,
    Json(req): Json<JsonRpcRequest>,
) -> Result<Json<JsonRpcResponse>, ProxyError> {
    let agent = sqlx::query_as::<_, AgentEndpoint>(
        "SELECT name, status, url FROM agents WHERE id = $1",
    )
    .bind(agent_id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| ProxyError::DatabaseError(e.to_string()))?
    .ok_or(ProxyError::AgentNotFound)?;

    if agent.status != "running" {
        return Err(ProxyError::InvalidRequest(format!(
            "agent '{}' is not running (status: {})",
            agent.name, agent.status
        )));
    }

    let target_url = if let Some(ref url) = agent.url {
        if !url.is_empty() {
            let u = url.trim_end_matches('/');
            format!("{}/", u)
        } else {
            let container_id = nasiko_runtime::ContainerId::new(agent.name.clone());
            let endpoint = state.runtime.endpoint(&container_id).await
                .map_err(|e| ProxyError::DatabaseError(format!("runtime endpoint: {e}")))?;
            let e = endpoint.trim_end_matches('/');
            format!("{}/", e)
        }
    } else {
        let container_id = nasiko_runtime::ContainerId::new(agent.name.clone());
        let endpoint = state.runtime.endpoint(&container_id).await
            .map_err(|e| ProxyError::DatabaseError(format!("runtime endpoint: {e}")))?;
        let e = endpoint.trim_end_matches('/');
        format!("{}/", e)
    };

    let response = state
        .http_client
        .post(&target_url)
        .json(&req)
        .send()
        .await
        .map_err(|e| ProxyError::DatabaseError(format!("forward failed: {e}")))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(ProxyError::InvalidRequest(format!("agent HTTP {}: {}", status, body)));
    }

    let resp_body: JsonRpcResponse = response
        .json()
        .await
        .map_err(|e| ProxyError::InvalidRequest(format!("invalid agent response: {e}")))?;

    Ok(Json(resp_body))
}

#[derive(sqlx::FromRow)]
struct AgentEndpoint {
    name: String,
    status: String,
    url: Option<String>,
}

/// Process registry discovery queries
async fn handle_registry_message(
    state: AppState,
    req: JsonRpcRequest,
) -> Result<Json<JsonRpcResponse>, ProxyError> {
    // Parse message content
    let params = req.params.clone();
    let message = params
        .get("message")
        .ok_or(ProxyError::InvalidRequest("missing message".to_string()))?;

    let parts = message
        .get("parts")
        .and_then(|p| p.as_array())
        .ok_or(ProxyError::InvalidRequest("missing parts".to_string()))?;

    // Extract query from first part (could be text or structured data)
    let query_text = if let Some(text_part) = parts.iter().find(|p| p.get("kind") == Some(&json!("text"))) {
        text_part
            .get("text")
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .to_string()
    } else if let Some(data_part) = parts.iter().find(|p| p.get("kind") == Some(&json!("data"))) {
        // Structured query
        let empty_obj = json!({});
        let data = data_part.get("data").unwrap_or(&empty_obj);
        if let Some(filter) = data.get("filter") {
            // Extract tags for filtering
            filter.to_string()
        } else {
            String::new()
        }
    } else {
        String::new()
    };

    // Query agents (simple keyword match for now)
    let agents = sqlx::query_as::<_, AgentCardRow>(
        r#"
        SELECT id, name, display_name, description, version, protocol_version,
               preferred_transport, capabilities, security_schemes,
               default_input_modes, default_output_modes, skills, url, documentation_url
        FROM agents
        WHERE status = 'running'
          AND (
            $1 = ''
            OR name ILIKE '%' || $1 || '%'
            OR description ILIKE '%' || $1 || '%'
            OR EXISTS (
                SELECT 1 FROM unnest(tags) AS tag WHERE tag ILIKE '%' || $1 || '%'
            )
          )
        LIMIT 20
        "#,
    )
    .bind(query_text.as_str())
    .fetch_all(&state.db)
    .await
    .map_err(|e| ProxyError::DatabaseError(e.to_string()))?;

    // Build A2A response with discovered agents
    // Return agent URL as-is (it's set by the control plane when deploying)
    let agent_list: Vec<serde_json::Value> = agents
        .into_iter()
        .map(|a| {
            let url = a.url.clone().unwrap_or_else(|| format!("/agents/{}/a2a/v1", a.id));
            json!({
                "agent_id": a.id,
                "name": a.name,
                "display_name": a.display_name,
                "description": a.description,
                "version": a.version,
                "skills": a.skills,
                "capabilities": a.capabilities,
                "url": url,
            })
        })
        .collect();

    let response = JsonRpcResponse {
        jsonrpc: "2.0".to_string(),
        id: req.id,
        result: Some(json!({
            "taskId": Uuid::new_v4().to_string(),
            "contextId": Uuid::new_v4().to_string(),
            "status": { "state": "completed" },
            "artifacts": [{
                "parts": [{
                    "kind": "data",
                    "data": {
                        "agents": agent_list
                    }
                }]
            }]
        })),
        error: None,
    };

    Ok(Json(response))
}

/// Serve agent card for a specific agent (proxied well-known URL)
async fn agent_card(
    State(state): State<AppState>,
    Path(agent_id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ProxyError> {
    let agent = sqlx::query_as::<_, AgentCardRow>(
        r#"
        SELECT id, name, display_name, description, version, protocol_version,
               preferred_transport, capabilities, security_schemes,
               default_input_modes, default_output_modes, skills, url, documentation_url
        FROM agents
        WHERE id = $1 AND status = 'running'
        "#,
    )
    .bind(agent_id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| ProxyError::DatabaseError(e.to_string()))?
    .ok_or(ProxyError::AgentNotFound)?;

    // Build A2A-compliant agent card
    // URL points back to control plane for proxying
    let url = agent.url.clone().unwrap_or_else(|| format!("/agents/{}/a2a/v1", agent_id));
    let card = json!({
        "name": agent.name,
        "displayName": agent.display_name,
        "description": agent.description,
        "version": agent.version,
        "protocolVersion": agent.protocol_version,
        "supportedInterfaces": [{
            "url": url.clone(),
            "protocolBinding": agent.preferred_transport,
            "protocolVersion": agent.protocol_version,
        }],
        "capabilities": agent.capabilities,
        "securitySchemes": agent.security_schemes,
        "defaultInputModes": agent.default_input_modes,
        "defaultOutputModes": agent.default_output_modes,
        "skills": agent.skills,
        "url": agent.url,
        "documentationUrl": agent.documentation_url,
    });

    Ok(Json(card))
}

#[derive(sqlx::FromRow)]
struct AgentCardRow {
    id: Uuid,
    name: String,
    display_name: Option<String>,
    description: Option<String>,
    version: String,
    protocol_version: String,
    preferred_transport: String,
    capabilities: sqlx::types::Json<serde_json::Value>,
    security_schemes: sqlx::types::Json<serde_json::Value>,
    default_input_modes: sqlx::types::Json<Vec<String>>,
    default_output_modes: sqlx::types::Json<Vec<String>>,
    skills: sqlx::types::Json<Vec<serde_json::Value>>,
    url: Option<String>,
    documentation_url: Option<String>,
}

#[derive(Debug)]
enum ProxyError {
    AgentNotFound,
    DatabaseError(String),
    InvalidRequest(String),
}

impl IntoResponse for ProxyError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            ProxyError::AgentNotFound => (StatusCode::NOT_FOUND, "Agent not found or not running".to_string()),
            ProxyError::DatabaseError(e) => (StatusCode::INTERNAL_SERVER_ERROR, e),
            ProxyError::InvalidRequest(e) => (StatusCode::BAD_REQUEST, e),
        };

        let body = Json(json!({
            "error": message
        }));

        (status, body).into_response()
    }
}

//! A2A registry endpoint - the control plane acting as the "registry
//! agent" from `docs/A2A_REGISTRY_DESIGN.md`.
//!
//! Platform agents are told one URL (`A2A_DISCOVERY_URL`, injected at deploy
//! time) and discover their peers by POSTing a JSONRPC `message/send` to
//! `{base}/a2a/v1`, then reading `result.artifacts[0].parts[0].data.agents`.
//! Both in-tree consumers parse exactly that shape: the assistant-agent
//! (`oss/agents/assistant-agent/main.py::_discover_agents`) and
//! `nasiko_react_agent::AgentRegistry::discover_from_cp`.
//!
//! Authentication is optional but respected: an unauthenticated caller sees only
//! public agents; an authenticated caller (user session or an agent credential)
//! sees exactly the agents it may access under the existing ACL. Globally
//! rate-limited. Returns running agents' names, descriptions, skills, and their
//! *runtime-internal* endpoints (VPC-private node IPs / localhost), which are
//! unreachable from outside the deployment's network.

use axum::{
    Json,
    extract::State,
    http::HeaderMap,
    response::{IntoResponse, Response},
};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::state::AppState;

/// Identity resolved from optional authentication.
enum RegistryIdentity {
    /// Unauthenticated - see only public agents.
    Anonymous,
    /// Authenticated user session - see owned + public + granted agents.
    User(Uuid),
    /// Authenticated agent credential - see what its owner sees.
    Agent(Uuid), // owner_id
}

pub async fn registry_a2a_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<Value>,
) -> Response {
    let request_id = req.get("id").cloned().unwrap_or(Value::Null);
    let method = req
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or_default();
    // Accept both protocol dialects (0.3 message/send, 1.0 SendMessage) -
    // the registry answers the same directory either way.
    if !matches!(method, "message/send" | "message/stream" | "SendMessage") {
        return Json(jsonrpc_error(
            request_id,
            -32601,
            "unsupported method - the registry answers message/send",
        ))
        .into_response();
    }

    // Resolve optional authentication: user session (Claims in extensions)
    // or agent credential (Bearer token).
    let identity = resolve_identity(&state, &headers).await;

    match discoverable_agents(&state, identity).await {
        Ok(agents) => Json(agents_response(request_id, agents)).into_response(),
        Err(e) => {
            tracing::error!(error = %e, "registry a2a: agent listing failed");
            Json(jsonrpc_error(request_id, -32603, "internal error")).into_response()
        }
    }
}

/// Resolve the caller's identity from optional authentication. Validates JWT
/// session tokens (user credentials) or agent gateway tokens (agent credentials).
async fn resolve_identity(state: &AppState, headers: &HeaderMap) -> RegistryIdentity {
    if let Some(token) = crate::mcp::identity::bearer_token(headers) {
        // Try validating as a user session token (JWT).
        if let Ok(identity) = state.auth.validate_token(token).await
            && let Ok(user_id) = identity.user_id.parse::<Uuid>()
        {
            return RegistryIdentity::User(user_id);
        }

        // Try validating as an agent credential.
        match nasiko_mcp_gateway::agent_tokens::authenticate(&state.db, token).await {
            Ok(Some(agent_id)) => {
                // Resolve the agent's owner.
                if let Ok(Some(owner_id)) = sqlx::query_scalar::<_, Uuid>(
                    "SELECT owner_id FROM agents WHERE id = $1 AND deleted_at IS NULL",
                )
                .bind(agent_id)
                .fetch_optional(&state.db)
                .await
                {
                    return RegistryIdentity::Agent(owner_id);
                }
            }
            Ok(None) => {} // Invalid token - fall through to Anonymous
            Err(e) => {
                tracing::warn!(error = %e, "registry a2a: agent token lookup failed");
            }
        }
    }

    RegistryIdentity::Anonymous
}

#[derive(sqlx::FromRow)]
struct AgentRow {
    id: Uuid,
    name: String,
    description: Option<String>,
    url: Option<String>,
}

#[derive(sqlx::FromRow)]
struct SkillRow {
    agent_id: Uuid,
    skill_key: String,
    name: String,
    description: String,
    tags: Vec<String>,
    /// The AgentCard's own `examples` — the literal inputs this skill expects. A planner that
    /// only ever sees a paraphrasable name and description has to invent the wording it sends,
    /// which loses any skill that recognises an exact phrase.
    examples: Value,
}

/// Running agents the caller may access, with their callable endpoints and skills.
///
/// Filters by ACL: Anonymous sees only public agents; User and Agent identities
/// see owned + public + user grants + org grants (team/department in EE), reusing
/// the routing engine's ACL logic (`nasiko_orchestrator::agent_registry::get_agents_for_user`).
/// `agents.url` may be empty (a fresh k8s deploy persists before the pod is Ready) -
/// resolve those live from the runtime, same as `agent_proxy`, so a discovery answer
/// never hands out a knowingly-dead URL. An agent that still has no endpoint stays
/// listed (its name/description remain useful to a planner); callers already skip
/// delegation targets without a URL.
async fn discoverable_agents(
    state: &AppState,
    identity: RegistryIdentity,
) -> Result<Vec<Value>, sqlx::Error> {
    // Get the ACL-filtered agent IDs. For authenticated callers, this includes
    // owned + public + user grants + org grants (the full set). For anonymous,
    // we filter to public only via a simple query below.
    let allowed_agent_ids: Vec<Uuid> = match identity {
        RegistryIdentity::Anonymous => {
            // Unauthenticated: only public agents.
            sqlx::query_scalar(
                "SELECT id FROM agents \
                 WHERE status = 'running' AND deleted_at IS NULL \
                   AND NOT is_internal AND is_public = true",
            )
            .fetch_all(&state.db)
            .await?
        }
        RegistryIdentity::User(user_id) | RegistryIdentity::Agent(user_id) => {
            // Authenticated: use the routing engine's ACL, which includes org grants.
            // First resolve org-granted agent IDs (OSS returns empty, EE returns actual).
            let identity: nasiko_auth::Identity = nasiko_auth::Identity {
                user_id: user_id.to_string(),
                username: String::new(), // Not used by org_granted_agent_ids
                is_superuser: false,     // Not relevant for this check
            };
            let org_granted_ids = state.auth.org_granted_agent_ids(&identity).await;

            // Call the routing engine's ACL function, which returns the full filtered set.
            let cards = nasiko_orchestrator::agent_registry::get_agents_for_user(
                user_id,
                &org_granted_ids,
                &state.db,
            )
            .await
            .map_err(|e| sqlx::Error::Protocol(e.to_string()))?;

            // Extract just the IDs - we'll fetch our own fields below.
            cards.into_iter().map(|c| c.id).collect()
        }
    };

    // Fetch the registry fields (name, description, url) for the allowed agents.
    let rows: Vec<AgentRow> = sqlx::query_as(
        "SELECT id, name, description, url FROM agents \
         WHERE id = ANY($1) \
         ORDER BY name",
    )
    .bind(&allowed_agent_ids)
    .fetch_all(&state.db)
    .await?;

    let skills = sqlx::query_as::<_, SkillRow>(
        "SELECT agent_id, skill_key, name, description, tags, examples FROM agent_skills",
    )
    .fetch_all(&state.db)
    .await?;

    let mut agents = Vec::with_capacity(rows.len());
    for row in rows {
        let url = match row.url.as_deref() {
            Some(url) if !url.is_empty() => url.to_string(),
            _ => state
                .runtime
                .endpoint(&nasiko_runtime::ContainerId::from_uuid(row.id))
                .await
                .unwrap_or_default(),
        };
        let agent_skills: Vec<Value> = skills
            .iter()
            .filter(|s| s.agent_id == row.id)
            .map(|s| {
                json!({
                    "id": s.skill_key,
                    "name": s.name,
                    "description": s.description,
                    "tags": s.tags,
                    "examples": s.examples,
                })
            })
            .collect();
        agents.push(agent_entry(
            row.id,
            &row.name,
            row.description.as_deref().unwrap_or_default(),
            &url,
            agent_skills,
        ));
    }
    Ok(agents)
}

/// One discovery entry. Carries both naming conventions in use - `url`
/// (assistant-agent, and the design doc's docker-era `localhost` rewrite
/// depends on it) and `endpoint`/`agent_id` (`nasiko_react_agent::AgentInfo`,
/// the design doc's examples) - so either consumer deserializes it directly.
fn agent_entry(id: Uuid, name: &str, description: &str, url: &str, skills: Vec<Value>) -> Value {
    json!({
        "id": id.to_string(),
        "agent_id": id.to_string(),
        "name": name,
        "description": description,
        "url": url,
        "endpoint": url,
        "skills": skills,
    })
}

fn agents_response(request_id: Value, agents: Vec<Value>) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": request_id,
        "result": {
            "artifacts": [{
                "name": "agent-directory",
                "parts": [{ "kind": "data", "data": { "agents": agents } }],
            }],
        },
    })
}

fn jsonrpc_error(request_id: Value, code: i64, message: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": request_id,
        "error": { "code": code, "message": message },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_entry() -> Value {
        agent_entry(
            Uuid::nil(),
            "currency-agent",
            "Converts currencies",
            "http://172.31.0.68:31640",
            vec![json!({
                "id": "convert",
                "name": "Currency Conversion",
                "description": "Convert between currencies",
                "tags": ["finance"],
            })],
        )
    }

    /// The exact pointer `nasiko_react_agent::AgentRegistry::discover_from_cp`
    /// reads (`/artifacts/0/parts/0/data/agents`).
    #[test]
    fn response_exposes_agents_at_the_react_agent_pointer() {
        let resp = agents_response(json!("req-1"), vec![sample_entry()]);

        let agents = resp
            .pointer("/result/artifacts/0/parts/0/data/agents")
            .and_then(Value::as_array)
            .expect("agents array at the documented pointer");
        assert_eq!(agents.len(), 1);
        assert_eq!(resp["id"], json!("req-1"));
        assert_eq!(resp["jsonrpc"], "2.0");
    }

    /// The assistant-agent walks artifacts[].parts[] looking for kind=="data";
    /// react-agent's `AgentInfo` deserializes each entry and requires
    /// id/name/description/endpoint/skills.
    #[test]
    fn entries_satisfy_both_in_tree_consumers() {
        let resp = agents_response(Value::Null, vec![sample_entry()]);

        let part = &resp["result"]["artifacts"][0]["parts"][0];
        assert_eq!(part["kind"], "data");

        let entry = &part["data"]["agents"][0];
        for key in ["id", "agent_id", "name", "description", "url", "endpoint"] {
            assert!(
                entry[key].is_string(),
                "missing string field {key}: {entry}"
            );
        }
        assert_eq!(entry["url"], entry["endpoint"]);
        assert!(entry["skills"].is_array());
        assert_eq!(entry["skills"][0]["tags"][0], "finance");
    }

    #[test]
    fn non_message_methods_get_a_jsonrpc_method_error() {
        let err = jsonrpc_error(json!(7), -32601, "unsupported method");
        assert_eq!(err["id"], json!(7));
        assert_eq!(err["error"]["code"], -32601);
        assert!(err.get("result").is_none());
    }
}

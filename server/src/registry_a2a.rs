//! Public A2A registry endpoint — the control plane acting as the "registry
//! agent" from `docs/A2A_REGISTRY_DESIGN.md`.
//!
//! Platform agents are told one URL (`A2A_DISCOVERY_URL`, injected at deploy
//! time) and discover their peers by POSTing a JSONRPC `message/send` to
//! `{base}/a2a/v1`, then reading `result.artifacts[0].parts[0].data.agents`.
//! Both in-tree consumers parse exactly that shape: the assistant-agent
//! (`oss/agents/assistant-agent/main.py::_discover_agents`) and
//! `nasiko_react_agent::AgentRegistry::discover_from_cp`.
//!
//! Deliberately unauthenticated (agents carry no platform credentials — the
//! proxy strips `authorization` before forwarding, by design) but globally
//! rate-limited. What it exposes is the discovery surface the design doc
//! declares public anyway: running agents' names, descriptions, skills, and
//! their *runtime-internal* endpoints (VPC-private node IPs / localhost),
//! which are unreachable from outside the deployment's network.

use axum::{
    Json,
    extract::State,
    response::{IntoResponse, Response},
};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::state::AppState;

pub async fn registry_a2a_handler(
    State(state): State<AppState>,
    Json(req): Json<Value>,
) -> Response {
    let request_id = req.get("id").cloned().unwrap_or(Value::Null);
    let method = req
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or_default();
    // Accept both protocol dialects (0.3 message/send, 1.0 SendMessage) —
    // the registry answers the same directory either way.
    if !matches!(method, "message/send" | "message/stream" | "SendMessage") {
        return Json(jsonrpc_error(
            request_id,
            -32601,
            "unsupported method — the registry answers message/send",
        ))
        .into_response();
    }

    match discoverable_agents(&state).await {
        Ok(agents) => Json(agents_response(request_id, agents)).into_response(),
        Err(e) => {
            tracing::error!(error = %e, "registry a2a: agent listing failed");
            Json(jsonrpc_error(request_id, -32603, "internal error")).into_response()
        }
    }
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
}

/// Every running agent, with its callable endpoint and advertised skills.
///
/// `agents.url` may be empty (a fresh k8s deploy persists before the pod is
/// Ready) — resolve those live from the runtime, same as `agent_proxy`, so a
/// discovery answer never hands out a knowingly-dead URL. An agent that still
/// has no endpoint stays listed (its name/description remain useful to a
/// planner); callers already skip delegation targets without a URL.
async fn discoverable_agents(state: &AppState) -> Result<Vec<Value>, sqlx::Error> {
    let rows = sqlx::query_as::<_, AgentRow>(
        "SELECT id, name, description, url FROM agents \
         WHERE status = 'running' AND deleted_at IS NULL ORDER BY name",
    )
    .fetch_all(&state.db)
    .await?;

    let skills = sqlx::query_as::<_, SkillRow>(
        "SELECT agent_id, skill_key, name, description, tags FROM agent_skills",
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

/// One discovery entry. Carries both naming conventions in use — `url`
/// (assistant-agent, and the design doc's docker-era `localhost` rewrite
/// depends on it) and `endpoint`/`agent_id` (`nasiko_react_agent::AgentInfo`,
/// the design doc's examples) — so either consumer deserializes it directly.
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

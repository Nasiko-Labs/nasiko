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
//! What a caller sees depends on who it is:
//! - **No credentials:** public agents only. This is the catalog anyone who
//!   can reach the server may read, so it stays unauthenticated and globally
//!   rate-limited.
//! - **A session token** (`Authorization: Bearer`, or the session cookie): the
//!   agents that caller may call through the agent proxy, decided by the same
//!   edition-aware check the proxy uses (`AuthService::can_access_agent`). An
//!   invalid token is a 401, never a silent fall back to the public view.
//!
//! Every entry's address is the agent proxy path, `/api/agents/{id}`, relative
//! to this server — never a container address: a parked agent has none, a
//! harness agent never will, and the proxy is where authentication, `agent_acl`
//! and the wake-on-request happen. The runtime-internal address (a node IP, a
//! pod address, `localhost`) is never returned: it let any caller map the
//! private network, and calling it directly skipped the proxy's ACL, flow guard
//! and tracing (NAS-1129).

use axum::{
    Json,
    extract::State,
    http::{HeaderMap, header},
    response::{IntoResponse, Response},
};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::auth::Claims;
use crate::state::AppState;

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

    let viewer = match viewer(&state, &headers).await {
        Ok(viewer) => viewer,
        Err(refused) => return refused,
    };
    match discoverable_agents(&state, viewer.as_ref()).await {
        Ok(agents) => Json(agents_response(request_id, agents)).into_response(),
        Err(e) => {
            tracing::error!(error = %e, "registry a2a: agent listing failed");
            Json(jsonrpc_error(request_id, -32603, "internal error")).into_response()
        }
    }
}

/// Who is asking: `None` for a caller with no credentials, else the session
/// the caller presented. A credential that does not validate is refused with
/// 401, so an expired token never quietly narrows the answer to the public
/// view.
async fn viewer(state: &AppState, headers: &HeaderMap) -> Result<Option<Claims>, Response> {
    let presented = headers.contains_key(header::AUTHORIZATION)
        || crate::auth::middleware::extract_token(headers).is_some();
    if !presented {
        return Ok(None);
    }
    crate::auth::middleware::validate_bearer(state, headers)
        .await
        .map(Some)
        .map_err(|(status, message)| (status, message).into_response())
}

#[derive(sqlx::FromRow)]
struct AgentRow {
    id: Uuid,
    name: String,
    description: Option<String>,
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

/// The running or parked agents `viewer` may call, with their advertised skills.
///
/// No viewer: public agents only (`is_public`, or a public grant). A viewer:
/// every agent the edition's `can_access_agent` lets them call, which for a
/// superuser is all of them. Parked agents are included — the proxy wakes them
/// on the next call. A harness agent shows to someone other than its owner only
/// when it is public, a platform catalog row: no grant reaches it
/// (`nasiko_auth::sharing`), here or in `can_access_agent`.
async fn discoverable_agents(
    state: &AppState,
    viewer: Option<&Claims>,
) -> Result<Vec<Value>, sqlx::Error> {
    let rows = sqlx::query_as::<_, AgentRow>(&format!(
        "SELECT a.id, a.name, a.description FROM agents a \
         WHERE a.status IN ('running', 'parked') AND a.deleted_at IS NULL \
           AND NOT a.is_internal \
           AND ($1 OR a.is_public OR ({grants_reach} AND EXISTS ( \
                SELECT 1 FROM agent_grants g \
                 WHERE g.agent_id = a.id AND g.grant_type = 'public' AND g.grantee_id = '*'))) \
         ORDER BY a.name",
        grants_reach = nasiko_auth::sharing::grants_reach_sql("a"),
    ))
    .bind(viewer.is_some())
    .fetch_all(&state.db)
    .await?;

    let rows = match viewer {
        None => rows,
        Some(claims) => {
            let mut callable = Vec::with_capacity(rows.len());
            for row in rows {
                if crate::acl::can_access_agent(state, claims, row.id).await {
                    callable.push(row);
                }
            }
            callable
        }
    };

    let skills = sqlx::query_as::<_, SkillRow>(
        "SELECT agent_id, skill_key, name, description, tags, examples FROM agent_skills \
         WHERE agent_id = ANY($1)",
    )
    .bind(rows.iter().map(|r| r.id).collect::<Vec<_>>())
    .fetch_all(&state.db)
    .await?;

    Ok(rows
        .into_iter()
        .map(|row| {
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
            agent_entry(
                row.id,
                &row.name,
                row.description.as_deref().unwrap_or_default(),
                agent_skills,
            )
        })
        .collect())
}

/// One discovery entry. Carries both naming conventions in use — `url`
/// (assistant-agent) and `endpoint`/`agent_id` (`nasiko_react_agent::AgentInfo`,
/// the design doc's examples) — so either consumer deserializes it directly.
/// Both hold the agent proxy path, relative to this server.
fn agent_entry(id: Uuid, name: &str, description: &str, skills: Vec<Value>) -> Value {
    let proxy_path = format!("/api/agents/{id}");
    json!({
        "id": id.to_string(),
        "agent_id": id.to_string(),
        "name": name,
        "description": description,
        "url": proxy_path,
        "endpoint": proxy_path,
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
        assert_eq!(entry["url"], format!("/api/agents/{}", Uuid::nil()));
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

//! In-process stand-in for a running agent container.
//!
//! Real agent execution happens in a separate runtime and is deliberately out
//! of scope for control-plane benchmarking — `agent_proxy` only reverse-proxies
//! bytes (see `oss/server/src/agent_proxy.rs::to_axum_response`), it never
//! parses the agent's response body. So this just needs to answer any POST
//! with a valid-looking, fast A2A JSON-RPC response.

use axum::{Json, Router, routing::post};
use serde_json::{Value, json};
use tokio::task::JoinHandle;

pub struct SimAgentHandle {
    pub base_url: String,
    _task: JoinHandle<()>,
}

pub async fn spawn_sim_agent() -> SimAgentHandle {
    let app = Router::new().fallback(post(handle));

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind in-process sim agent");
    let addr = listener.local_addr().expect("sim agent local_addr");

    let task = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    SimAgentHandle { base_url: format!("http://{addr}"), _task: task }
}

async fn handle(Json(body): Json<Value>) -> Json<Value> {
    let id = body.get("id").cloned().unwrap_or(Value::Null);
    let context_id = body
        .pointer("/params/message/contextId")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

    Json(json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": {
            "task": {
                "id": uuid::Uuid::new_v4().to_string(),
                "contextId": context_id,
                "status": { "state": "TASK_STATE_COMPLETED" },
                "artifacts": [{
                    "artifactId": uuid::Uuid::new_v4().to_string(),
                    "parts": [{ "text": "bench-sim-agent: ok" }]
                }]
            }
        }
    }))
}

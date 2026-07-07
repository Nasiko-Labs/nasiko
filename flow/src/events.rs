use serde::Serialize;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{broadcast, RwLock};

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FlowEvent {
    AgentInvoke {
        caller_agent: String,
        target_agent: String,
        depth: u32,
    },
    AgentResult {
        caller_agent: String,
        target_agent: String,
        depth: u32,
        success: bool,
        latency_ms: u64,
    },
    /// An MCP tool call needs human approval (ask-stance permission, -32001)
    /// before it can proceed. Surfaced to the chat UI as an approval prompt.
    ToolApprovalRequired {
        agent_id: String,
        server: String,
        tool: String,
    },
}

/// In-memory broadcast bus for flow events, keyed by flow_id.
#[derive(Clone)]
pub struct FlowEventBus {
    channels: Arc<RwLock<HashMap<String, broadcast::Sender<FlowEvent>>>>,
}

impl Default for FlowEventBus {
    fn default() -> Self {
        Self::new()
    }
}

impl FlowEventBus {
    pub fn new() -> Self {
        Self {
            channels: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub async fn subscribe(&self, flow_id: &str) -> broadcast::Receiver<FlowEvent> {
        let mut channels = self.channels.write().await;
        let tx = channels
            .entry(flow_id.to_string())
            .or_insert_with(|| broadcast::channel(64).0);
        tx.subscribe()
    }

    pub async fn publish(&self, flow_id: &str, event: FlowEvent) {
        let channels = self.channels.read().await;
        if let Some(tx) = channels.get(flow_id) {
            let _ = tx.send(event);
        }
    }

    pub async fn remove(&self, flow_id: &str) {
        let mut channels = self.channels.write().await;
        channels.remove(flow_id);
    }
}

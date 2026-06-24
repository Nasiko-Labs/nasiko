use serde::Serialize;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{broadcast, RwLock};

/// An event emitted when the proxy observes an agent-to-agent call.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FlowEvent {
    /// An agent is invoking another agent (request forwarded).
    AgentInvoke {
        caller_agent: String,
        target_agent: String,
        depth: u32,
    },
    /// An agent-to-agent call completed.
    AgentResult {
        caller_agent: String,
        target_agent: String,
        depth: u32,
        success: bool,
        latency_ms: u64,
    },
}

/// In-memory broadcast bus for flow events. Keyed by flow_id.
/// The a2a_handler subscribes to a flow's channel and merges events into the SSE stream.
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

    /// Subscribe to events for a given flow_id. Creates the channel if it doesn't exist.
    pub async fn subscribe(&self, flow_id: &str) -> broadcast::Receiver<FlowEvent> {
        let mut channels = self.channels.write().await;
        let tx = channels
            .entry(flow_id.to_string())
            .or_insert_with(|| broadcast::channel(64).0);
        tx.subscribe()
    }

    /// Publish an event for a given flow_id. No-op if nobody is listening.
    pub async fn publish(&self, flow_id: &str, event: FlowEvent) {
        let channels = self.channels.read().await;
        if let Some(tx) = channels.get(flow_id) {
            let _ = tx.send(event);
        }
    }

    /// Remove a flow's channel (cleanup after stream ends).
    pub async fn remove(&self, flow_id: &str) {
        let mut channels = self.channels.write().await;
        channels.remove(flow_id);
    }
}

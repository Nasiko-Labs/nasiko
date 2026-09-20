use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const TRACEPARENT_HEADER: &str = "traceparent";

/// Parsed traceparent components relevant to flow tracking.
/// Format: {version}-{trace_id}-{parent_id}-{flags}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlowContext {
    pub flow_id: String,
    pub parent_span_id: String,
}

impl FlowContext {
    pub fn new_root() -> Self {
        let trace_id = Uuid::new_v4().simple().to_string();
        let span_id = Self::generate_span_id();
        Self {
            flow_id: trace_id,
            parent_span_id: span_id,
        }
    }

    /// Parse a traceparent header value into a FlowContext.
    pub fn from_traceparent(value: &str) -> Option<Self> {
        let parts: Vec<&str> = value.split('-').collect();
        if parts.len() < 4 {
            return None;
        }
        let trace_id = parts[1].to_string();
        let parent_id = parts[2].to_string();

        if trace_id.len() != 32 || parent_id.len() != 16 {
            return None;
        }

        Some(Self {
            flow_id: trace_id,
            parent_span_id: parent_id,
        })
    }

    /// Generate a traceparent header value for outbound calls.
    pub fn to_traceparent(&self) -> String {
        let child_span_id = Self::generate_span_id();
        format!("00-{}-{}-01", self.flow_id, child_span_id)
    }

    /// Redis key for this flow's state.
    pub fn redis_key(&self) -> String {
        format!("flow:{}", self.flow_id)
    }

    fn generate_span_id() -> String {
        let id = Uuid::new_v4();
        let bytes = id.as_bytes();
        bytes[..8].iter().map(|b| format!("{:02x}", b)).collect()
    }
}

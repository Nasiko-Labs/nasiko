use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// W3C Trace Context traceparent header name.
pub const TRACEPARENT_HEADER: &str = "traceparent";

/// Parsed traceparent components relevant to flow tracking.
/// Format: {version}-{trace_id}-{parent_id}-{flags}
/// Example: 00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlowContext {
    /// The trace_id from traceparent — used as flow_id. Shared across all hops.
    pub flow_id: String,
    /// The parent span_id — identifies which specific call this is.
    pub parent_span_id: String,
}

impl FlowContext {
    /// Create a new root flow context (generates a fresh trace_id).
    pub fn new_root() -> Self {
        let trace_id = Uuid::new_v4().simple().to_string();
        let span_id = Self::generate_span_id();
        Self {
            flow_id: trace_id,
            parent_span_id: span_id,
        }
    }

    /// Parse a traceparent header value into a FlowContext.
    /// Returns None if the header is missing or malformed.
    pub fn from_traceparent(header_value: Option<&str>) -> Option<Self> {
        let value = header_value?;
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

    /// Extract flow context from request headers.
    pub fn from_headers(headers: &axum::http::HeaderMap) -> Option<Self> {
        let value = headers.get(TRACEPARENT_HEADER)?.to_str().ok()?;
        Self::from_traceparent(Some(value))
    }

    /// Generate a traceparent header value for outbound calls.
    /// Creates a new span_id for the child span.
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

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_root_produces_valid_ids() {
        let ctx = FlowContext::new_root();
        assert_eq!(ctx.flow_id.len(), 32);
        assert_eq!(ctx.parent_span_id.len(), 16);
        assert!(ctx.flow_id.chars().all(|c| c.is_ascii_hexdigit()));
        assert!(ctx.parent_span_id.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn new_root_ids_are_unique() {
        let a = FlowContext::new_root();
        let b = FlowContext::new_root();
        assert_ne!(a.flow_id, b.flow_id);
    }

    #[test]
    fn from_traceparent_parses_valid_header() {
        let ctx = FlowContext::from_traceparent(
            "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01",
        )
        .unwrap();
        assert_eq!(ctx.flow_id, "4bf92f3577b34da6a3ce929d0e0e4736");
        assert_eq!(ctx.parent_span_id, "00f067aa0ba902b7");
    }

    #[test]
    fn from_traceparent_rejects_malformed() {
        assert!(FlowContext::from_traceparent("not-a-traceparent").is_none());
        assert!(FlowContext::from_traceparent("00-short-short-01").is_none());
    }

    #[test]
    fn to_traceparent_format() {
        let ctx = FlowContext::from_traceparent(
            "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01",
        )
        .unwrap();
        let tp = ctx.to_traceparent();
        assert!(tp.starts_with("00-4bf92f3577b34da6a3ce929d0e0e4736-"));
        assert!(tp.ends_with("-01"));
        let parts: Vec<&str> = tp.split('-').collect();
        assert_eq!(parts.len(), 4);
        assert_eq!(parts[2].len(), 16);
    }
}

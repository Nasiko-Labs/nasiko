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

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderMap;

    #[test]
    fn new_root_produces_valid_trace_and_span_ids() {
        let ctx = FlowContext::new_root();
        assert_eq!(ctx.flow_id.len(), 32, "trace_id must be 32 hex chars");
        assert_eq!(ctx.parent_span_id.len(), 16, "span_id must be 16 hex chars");
        assert!(ctx.flow_id.chars().all(|c| c.is_ascii_hexdigit()));
        assert!(ctx.parent_span_id.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn new_root_ids_are_unique_across_calls() {
        let a = FlowContext::new_root();
        let b = FlowContext::new_root();
        assert_ne!(a.flow_id, b.flow_id);
    }

    #[test]
    fn from_traceparent_parses_valid_header() {
        let header = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";
        let ctx = FlowContext::from_traceparent(Some(header)).unwrap();
        assert_eq!(ctx.flow_id, "4bf92f3577b34da6a3ce929d0e0e4736");
        assert_eq!(ctx.parent_span_id, "00f067aa0ba902b7");
    }

    #[test]
    fn from_traceparent_returns_none_for_missing_header() {
        assert!(FlowContext::from_traceparent(None).is_none());
    }

    #[test]
    fn from_traceparent_returns_none_for_malformed_header() {
        assert!(FlowContext::from_traceparent(Some("not-a-traceparent")).is_none());
        assert!(FlowContext::from_traceparent(Some("00-short-short-01")).is_none());
    }

    #[test]
    fn from_headers_returns_none_when_traceparent_absent() {
        let headers = HeaderMap::new();
        assert!(FlowContext::from_headers(&headers).is_none());
    }

    #[test]
    fn from_headers_parses_traceparent_when_present() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "traceparent",
            "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01"
                .parse()
                .unwrap(),
        );
        let ctx = FlowContext::from_headers(&headers).unwrap();
        assert_eq!(ctx.flow_id, "4bf92f3577b34da6a3ce929d0e0e4736");
    }

    #[test]
    fn middleware_fallback_pattern_produces_root_context_when_header_missing() {
        // Regression: proxy middleware used to error on missing traceparent.
        // It now falls back to new_root() so direct calls without OTel work.
        let headers = HeaderMap::new();
        let ctx = FlowContext::from_headers(&headers).unwrap_or_else(FlowContext::new_root);
        assert_eq!(ctx.flow_id.len(), 32, "fallback root trace must have valid trace_id");
    }
}

//! Bridges `tracing` spans to flow tracking.
//!
//! The dispatch/proxy handlers used to mint flow ids with
//! `FlowContext::new_root()` and forward `FlowContext::to_traceparent()`, which
//! generates a random child span id that no exporter ever backs — every agent
//! span parented to a phantom node. These helpers derive the FlowContext (and
//! the forwarded traceparent) from a real server span instead, so the trace
//! tree in Tempo is connected end to end.

use nasiko_flow::FlowContext;

pub use nasiko_observability::remote_context_from_traceparent;

/// Build a [`FlowContext`] from the span's real OTel trace/span ids.
///
/// Returns `None` when no OTel layer is active (no exporter configured) —
/// callers fall back to their previous id-minting behavior.
pub fn flow_context_from_span(span: &tracing::Span) -> Option<FlowContext> {
    nasiko_observability::span_ids(span).map(|(flow_id, parent_span_id)| FlowContext {
        flow_id,
        parent_span_id,
    })
}

/// W3C traceparent naming this context's span as the parent — unlike
/// `FlowContext::to_traceparent()`, which mints a fresh random child span id.
pub fn traceparent_for(ctx: &FlowContext) -> String {
    format!("00-{}-{}-01", ctx.flow_id, ctx.parent_span_id)
}

/// One text message in the GenAI semconv `gen_ai.{input,output}.messages`
/// shape: `[{role, parts: [{type: "text", content}]}]`.
pub fn genai_text_message(role: &str, text: &str) -> String {
    serde_json::json!([{"role": role, "parts": [{"type": "text", "content": text}]}]).to_string()
}

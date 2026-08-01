//! Bridges `tracing` spans to flow tracking.
//!
//! The dispatch/proxy handlers used to mint flow ids with
//! `FlowContext::new_root()` and forward `FlowContext::to_traceparent()`, which
//! generates a random child span id that no exporter ever backs — every agent
//! span parented to a phantom node. These helpers derive the FlowContext (and
//! the forwarded traceparent) from a real server span instead, so the trace
//! tree in Tempo is connected end to end.

use nasiko_flow::FlowContext;

/// Build a [`FlowContext`] from the span's real OTel trace/span ids.
///
/// Returns `None` when no OTel layer is active (no exporter configured) —
/// callers fall back to their previous id-minting behavior.
pub fn flow_context_from_span(span: &tracing::Span) -> Option<FlowContext> {
    use opentelemetry::trace::TraceContextExt as _;
    use tracing_opentelemetry::OpenTelemetrySpanExt as _;
    let cx = span.context();
    let span_ref = cx.span();
    let sc = span_ref.span_context();
    sc.is_valid().then(|| FlowContext {
        flow_id: sc.trace_id().to_string(),
        parent_span_id: sc.span_id().to_string(),
    })
}

/// W3C traceparent naming this context's span as the parent — unlike
/// `FlowContext::to_traceparent()`, which mints a fresh random child span id.
pub fn traceparent_for(ctx: &FlowContext) -> String {
    format!("00-{}-{}-01", ctx.flow_id, ctx.parent_span_id)
}

/// Parse an inbound W3C traceparent into a remote OTel parent context, so a
/// proxy span joins the calling agent's trace. `None` for anything malformed.
pub fn remote_context_from_traceparent(tp: &str) -> Option<opentelemetry::Context> {
    use opentelemetry::propagation::TextMapPropagator as _;
    use opentelemetry::trace::TraceContextExt as _;
    let carrier: std::collections::HashMap<String, String> =
        [("traceparent".to_string(), tp.to_string())].into();
    let cx = opentelemetry_sdk::propagation::TraceContextPropagator::new().extract(&carrier);
    cx.span().span_context().is_valid().then_some(cx)
}

/// One text message in the GenAI semconv `gen_ai.{input,output}.messages`
/// shape: `[{role, parts: [{type: "text", content}]}]`.
pub fn genai_text_message(role: &str, text: &str) -> String {
    serde_json::json!([{"role": role, "parts": [{"type": "text", "content": text}]}]).to_string()
}

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

/// Stamp a request span with the sandbox that served it: `sandbox.provider` always, and
/// `sandbox.wake_ms` only when this request woke the agent. The span must declare both fields
/// (as `tracing::field::Empty`) or the records are silently dropped.
pub fn record_sandbox_on_span(
    span: &tracing::Span,
    provider: nasiko_runtime::SandboxProviderKind,
    wake_ms: Option<u64>,
) {
    span.record("sandbox.provider", provider.to_string().as_str());
    if let Some(wake_ms) = wake_ms {
        span.record("sandbox.wake_ms", wake_ms);
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use nasiko_runtime::SandboxProviderKind;
    use std::sync::{Arc, Mutex};
    use tracing::field::{Field, Visit};
    use tracing_subscriber::layer::{Context, SubscriberExt as _};
    use tracing_subscriber::registry::LookupSpan;

    /// Collects every `name=value` recorded on a span, however it was recorded.
    #[derive(Clone, Default)]
    struct Captured(Arc<Mutex<Vec<(String, String)>>>);

    impl Visit for Captured {
        fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
            self.0
                .lock()
                .unwrap()
                .push((field.name().to_owned(), format!("{value:?}")));
        }
    }

    impl<S: tracing::Subscriber + for<'a> LookupSpan<'a>> tracing_subscriber::Layer<S> for Captured {
        fn on_record(
            &self,
            _id: &tracing::span::Id,
            values: &tracing::span::Record<'_>,
            _ctx: Context<'_, S>,
        ) {
            values.record(&mut self.clone());
        }
    }

    fn proxy_span_with_sandbox_fields() -> tracing::Span {
        tracing::info_span!(
            "a2a.proxy",
            sandbox.provider = tracing::field::Empty,
            sandbox.wake_ms = tracing::field::Empty,
        )
    }

    fn recorded(wake_ms: Option<u64>) -> Vec<(String, String)> {
        let captured = Captured::default();
        let subscriber = tracing_subscriber::registry().with(captured.clone());
        tracing::subscriber::with_default(subscriber, || {
            let span = proxy_span_with_sandbox_fields();
            record_sandbox_on_span(&span, SandboxProviderKind::Fake, wake_ms);
        });
        captured.0.lock().unwrap().clone()
    }

    #[test]
    fn a_woken_agent_span_carries_provider_and_wake_ms() {
        let seen = recorded(Some(1234));
        assert!(seen.contains(&("sandbox.provider".into(), "\"fake\"".into())));
        assert!(seen.contains(&("sandbox.wake_ms".into(), "1234".into())));
    }

    #[test]
    fn a_warm_agent_span_carries_no_wake_ms() {
        let seen = recorded(None);
        assert!(seen.iter().any(|(k, _)| k == "sandbox.provider"));
        assert!(!seen.iter().any(|(k, _)| k == "sandbox.wake_ms"));
    }
}

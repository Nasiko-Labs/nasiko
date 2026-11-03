//! W3C trace-context helpers shared by the server and the MCP gateway, so a
//! span can join a caller's trace and name itself as the parent of an
//! outbound call without either crate depending on the `opentelemetry` crates.

/// Parse an inbound W3C traceparent into a remote OTel parent context.
/// `None` for anything malformed.
pub fn remote_context_from_traceparent(tp: &str) -> Option<opentelemetry::Context> {
    use opentelemetry::propagation::TextMapPropagator as _;
    use opentelemetry::trace::TraceContextExt as _;
    let carrier: std::collections::HashMap<String, String> =
        [("traceparent".to_string(), tp.to_string())].into();
    let cx = opentelemetry_sdk::propagation::TraceContextPropagator::new().extract(&carrier);
    cx.span().span_context().is_valid().then_some(cx)
}

/// Re-parent `span` onto the trace named by `traceparent`. Returns whether it
/// joined; a malformed header leaves the span as it was.
pub fn join_remote_trace(span: &tracing::Span, traceparent: &str) -> bool {
    use tracing_opentelemetry::OpenTelemetrySpanExt as _;
    remote_context_from_traceparent(traceparent)
        .map(|cx| span.set_parent(cx))
        .is_some()
}

/// The span's real OTel `(trace_id, span_id)` as hex. `None` when no OTel
/// layer is active.
pub fn span_ids(span: &tracing::Span) -> Option<(String, String)> {
    use opentelemetry::trace::TraceContextExt as _;
    use tracing_opentelemetry::OpenTelemetrySpanExt as _;
    let cx = span.context();
    let span_ref = cx.span();
    let sc = span_ref.span_context();
    sc.is_valid()
        .then(|| (sc.trace_id().to_string(), sc.span_id().to_string()))
}

/// W3C traceparent naming `span` as the parent, for forwarding downstream.
pub fn traceparent_of(span: &tracing::Span) -> Option<String> {
    span_ids(span).map(|(trace_id, span_id)| format!("00-{trace_id}-{span_id}-01"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const TP: &str = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";

    #[test]
    fn parses_a_valid_traceparent() {
        assert!(remote_context_from_traceparent(TP).is_some());
    }

    #[test]
    fn rejects_a_malformed_traceparent() {
        assert!(remote_context_from_traceparent("garbage").is_none());
    }

    #[test]
    fn without_an_otel_layer_there_are_no_ids() {
        let span = tracing::Span::none();
        assert!(span_ids(&span).is_none());
        assert!(traceparent_of(&span).is_none());
    }

    #[test]
    fn a_span_joins_the_remote_trace_and_names_itself_as_parent() {
        use opentelemetry::trace::TracerProvider as _;
        use tracing_subscriber::layer::SubscriberExt as _;
        let provider = opentelemetry_sdk::trace::SdkTracerProvider::builder().build();
        let subscriber = tracing_subscriber::registry()
            .with(tracing_opentelemetry::layer().with_tracer(provider.tracer("test")));
        tracing::subscriber::with_default(subscriber, || {
            let span = tracing::info_span!("probe");
            assert!(join_remote_trace(&span, TP));
            let (trace_id, span_id) = span_ids(&span).expect("ids");
            assert_eq!(trace_id, "4bf92f3577b34da6a3ce929d0e0e4736");
            assert_ne!(span_id, "00f067aa0ba902b7");
            assert_eq!(
                traceparent_of(&span).unwrap(),
                format!("00-{trace_id}-{span_id}-01")
            );
            assert!(!join_remote_trace(&span, "garbage"));
        });
    }
}

mod flow_span;
pub mod genai;
mod setup;

pub use flow_span::{
    flow_context_from_span, genai_text_message, remote_context_from_traceparent, traceparent_for,
};
pub use genai::{GenAiMetrics, GenAiSpan};
pub use setup::{TelemetryConfig, init_telemetry};

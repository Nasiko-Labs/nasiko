mod context_report;
mod flow_span;
pub mod genai;
mod setup;
mod tool_result_savings;

pub use context_report::{ContextRecord, record as record_context_report};
pub use flow_span::{
    flow_context_from_span, genai_text_message, remote_context_from_traceparent, traceparent_for,
};
pub use genai::{GenAiMetrics, GenAiSpan};
pub use setup::{TelemetryConfig, init_telemetry};
pub use tool_result_savings::{ToolResultRecord, record as record_tool_result_savings};

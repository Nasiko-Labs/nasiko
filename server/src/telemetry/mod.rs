pub mod genai;
mod setup;

pub use genai::{GenAiSpan, GenAiMetrics};
pub use setup::{init_telemetry, TelemetryConfig};

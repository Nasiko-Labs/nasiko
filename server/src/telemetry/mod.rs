pub mod genai;
mod setup;

pub use genai::{GenAiMetrics, GenAiSpan};
pub use setup::{TelemetryConfig, init_telemetry};

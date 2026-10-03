use serde::{Deserialize, Serialize};

/// Resource and depth limits for compact tool decoding and streaming.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Limits {
    /// Maximum number of tools per request (default: 128).
    pub max_tools: usize,
    /// Total input schema bytes allowed (default: 1 MiB).
    pub max_schema_bytes: usize,
    /// Maximum nesting depth for schemas (default: 32).
    pub max_schema_depth: usize,
    /// Maximum total response bytes processed (default: 1 MiB).
    pub max_response_bytes: usize,
    /// Maximum argument-object bytes per call (default: 256 KiB).
    pub max_argument_bytes: usize,
    /// Maximum JSON container nesting depth (default: 64).
    pub max_json_depth: usize,
    /// Maximum calls per response (default: 32).
    pub max_calls: usize,
    /// Maximum tool-name length in bytes (default: 64).
    pub max_tool_name_bytes: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_tools: 128,
            max_schema_bytes: 1024 * 1024,      // 1 MiB
            max_schema_depth: 32,
            max_response_bytes: 1024 * 1024,    // 1 MiB
            max_argument_bytes: 256 * 1024,     // 256 KiB
            max_json_depth: 64,
            max_calls: 32,
            max_tool_name_bytes: 64,
        }
    }
}

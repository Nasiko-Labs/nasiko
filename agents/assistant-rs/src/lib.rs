//! Reusable building blocks for Nasiko Rust agents.
//!
//! Copy this crate's `src/{mcp,pause,session}.rs` into another agent, or depend on it by path:
//!
//! - [`mcp`] — MCP gateway client. Tool calls return [`mcp::ToolOutcome`], so an
//!   `AUTH_REQUIRED` refusal (code `-32002`) arrives as structured data, not prose.
//! - [`pause`] — A2A events for `TASK_STATE_AUTH_REQUIRED` and `TASK_STATE_INPUT_REQUIRED`
//!   with the metadata the platform hoists onto the HITL card (`auth_url`, `provider`,
//!   `hitl_request_id`, `options`).
//! - [`llm_stream`] — parser for streamed chat completions: text deltas + tool calls.
//! - [`session`] — per-context conversation store, so a paused task resumes where it stopped.

pub mod llm_stream;
pub mod mcp;
pub mod pause;
pub mod session;

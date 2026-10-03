//! The text that frames the compact definitions for the model.
//!
//! Kept in its own module so wording can be tuned from measurement without touching logic. The
//! block is deliberately short: on a two-tool catalog the framing alone decides whether the
//! compact request is smaller than the native one, so every sentence here must pay for itself.
//! The grammar legend names only what a model has to read (`?`); `|null`, `datetime` and
//! `@{...}` are self-describing, and the rarer reversibility markers (`!`, `+`, `=`, `(...)`)
//! are inert to the model. One abstract example anchors the call syntax without teaching a tool
//! name that does not exist in the catalog.

/// Line placed before the definitions.
pub const HEADER: &str = "Tools (name(arg:type, optional?:type) - purpose):";

/// Call protocol placed after the definitions. [`crate::CompactTools::prompt`] assembles
/// `HEADER`, the definitions and this text.
pub const INSTRUCTIONS: &str = "Call a tool by replying <<call tool_name {\"arg\": \"value\"}>>, \
one call per line, listed tools only; otherwise answer normally.";

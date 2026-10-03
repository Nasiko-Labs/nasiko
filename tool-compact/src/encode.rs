//! Tool definitions → compact text, and calls → compact call text.

use crate::decode::{CALL_CLOSE, CALL_OPEN, parameters};
use crate::error::{CompactError, Result};
use crate::render::{DescriptionPolicy, Renderer, push_escaped, words};
use crate::types::{ToolCall, ToolDef};

/// Instructions appended after the definitions. They are paid on every request (23 o200k_base
/// tokens), so each phrase earned its place in live runs: the `{"arg":value}` shape stops models
/// writing `NAME(key:value)` or unquoted keys, and "only add optional args the user gave" stops
/// them inventing defaults (both raised format adherence in live runs).
pub const CALL_INSTRUCTIONS: &str =
    "Call tools as <<call NAME {\"arg\":value}>>; only add optional (?) args the user gave.";

/// Longest tool name OpenAI accepts.
const MAX_NAME_LEN: usize = 64;

/// Encoding knobs. The default is what the eval and the router use.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EncodeOptions {
    /// What happens to property descriptions.
    pub descriptions: DescriptionPolicy,
}

/// Compact definitions for one request's tools.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactTools {
    definitions: String,
}

impl CompactTools {
    /// One line per tool, in request order (grammar in the crate docs).
    pub fn definitions(&self) -> &str {
        &self.definitions
    }

    /// Definitions plus call-format instructions: the text to inject as a system message.
    pub fn system_prompt(&self) -> String {
        format!("{}\n{CALL_INSTRUCTIONS}", self.definitions)
    }
}

/// Encodes `tools` with [`EncodeOptions::default`].
///
/// Fails with [`CompactError::Unsupported`] when any tool uses a schema feature the grammar
/// cannot express; callers then send the request with native tools (bypass), never a lossy
/// rendering.
pub fn encode_tools(tools: &[ToolDef]) -> Result<CompactTools> {
    encode_tools_with(tools, EncodeOptions::default())
}

/// Encodes `tools` with explicit options.
pub fn encode_tools_with(tools: &[ToolDef], options: EncodeOptions) -> Result<CompactTools> {
    let mut definitions = String::new();
    for (i, tool) in tools.iter().enumerate() {
        check_name(tool, &tools[..i])?;
        if i > 0 {
            definitions.push('\n');
        }
        render_tool(tool, options, &mut definitions)?;
    }
    Ok(CompactTools { definitions })
}

fn check_name(tool: &ToolDef, earlier: &[ToolDef]) -> Result<()> {
    let invalid = |reason: &str| {
        Err(CompactError::InvalidTool {
            tool: tool.name.clone(),
            reason: reason.into(),
        })
    };
    let n = &tool.name;
    if n.is_empty() || n.len() > MAX_NAME_LEN {
        return invalid("name must be 1-64 characters");
    }
    if !n
        .bytes()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-'))
    {
        return invalid("name may only contain letters, digits, `_` and `-`");
    }
    if earlier.iter().any(|t| &t.name == n) {
        return invalid("duplicate tool name");
    }
    Ok(())
}

fn render_tool(tool: &ToolDef, options: EncodeOptions, out: &mut String) -> Result<()> {
    let params = parameters(tool)?;
    let tool_words = words(&tool.name);
    let renderer = Renderer {
        policy: options.descriptions,
        tool_words: &tool_words,
    };
    out.push_str(&tool.name);
    out.push('(');
    renderer.fields(&params, out);
    out.push(')');
    if params.closed && tool.parameters.is_some() {
        out.push('!');
    }
    if let Some(desc) = tool.description.as_deref().filter(|d| !d.is_empty()) {
        out.push_str(" - ");
        push_escaped(desc, &[], out);
    }
    Ok(())
}

/// Writes calls in the compact call format, one per line. Argument keys come out sorted, so the
/// text is deterministic.
pub fn render_calls(calls: &[ToolCall]) -> String {
    calls
        .iter()
        .map(|c| format!("{CALL_OPEN} {} {}{CALL_CLOSE}", c.name, c.arguments_json()))
        .collect::<Vec<_>>()
        .join("\n")
}

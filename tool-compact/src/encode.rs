//! Encode a slice of [`ToolDef`]s into a [`CompactTools`] handle.

use crate::error::EncodeError;
use crate::schema::render_compact_params;
use crate::types::{CompactTools, ToolDef};

/// The opening call marker prefix (7 bytes: `<<call `).
#[allow(dead_code)]
pub const CALL_OPEN: &str = "<<call ";
#[allow(dead_code)]
pub const CALL_CLOSE: &str = ">>";

/// Grammar hint injected into every system prompt block.
pub const GRAMMAR_HINT: &str = "\
To invoke a tool, respond with exactly:\n\
  <<call TOOL_NAME {\"arg\":value, ...}>>\n\
You may include multiple calls in one response. \
Plain text before, after, or between calls is preserved as-is.";

/// Encode a slice of [`ToolDef`]s into a [`CompactTools`] handle.
///
/// `Ok(compact)` contains:
/// - `schema_text`: a compact block for the system prompt listing each tool
///   with its parameters, types, required/optional markers, and enum values.
/// - `grammar_hint`: grammar instructions for the model.
/// - `defs`: cloned originals retained for validation during decoding.
///
/// Returns `Err(EncodeError::NoTools)` if `tools` is empty.
pub fn encode_tools(tools: &[ToolDef]) -> Result<CompactTools, EncodeError> {
    if tools.is_empty() {
        return Err(EncodeError::NoTools);
    }

    let mut lines = vec!["## Available tools\n".to_string()];

    for tool in tools {
        let f = &tool.function;
        let desc = f
            .description
            .as_deref()
            .map(|d| format!(" — {}", d))
            .unwrap_or_default();

        lines.push(format!("**{}**{}", f.name, desc));

        if let Some(params) = &f.parameters {
            let compact_params = render_compact_params(params);
            if !compact_params.is_empty() {
                lines.push(format!("  Parameters: {}", compact_params));
            }
        }

        lines.push(String::new());
    }

    let schema_text = lines.join("\n").trim_end().to_string();

    Ok(CompactTools {
        schema_text,
        grammar_hint: GRAMMAR_HINT.to_string(),
        defs: tools.to_vec(),
    })
}

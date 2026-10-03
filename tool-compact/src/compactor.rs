//! ToolDef JSON → compact signature string.
//!
//! # Output format
//! ```text
//! tool_name(req1:type, req2:type, opt1?:type, opt2?:type) - Description (truncated at 80 chars)
//! ```
//!
//! Rules:
//! - Required params come first, no `?`
//! - Optional params follow, with `?` suffix before the colon
//! - Within each group, params are sorted alphabetically for determinism
//! - Description is trimmed to 80 chars with `…` if longer
//!
//! # Injection
//! [`compact_tools`] also returns a call-format instruction string that must be
//! appended to the system prompt so the model knows the <<call>> format.

use serde_json::Value;

use crate::schema::render_type;

/// Result of compacting a full tools array.
#[derive(Debug, Clone)]
pub struct CompactTools {
    /// One compact signature line per tool.
    pub signatures: Vec<String>,
    /// The call-format instruction to inject into the system prompt.
    pub call_instruction: String,
    /// Original token count estimate (chars / 4 approximation).
    pub original_chars: usize,
    /// Compact token count estimate.
    pub compact_chars: usize,
}

impl CompactTools {
    /// Fraction of chars saved. 0.0 = no saving, 1.0 = everything removed.
    pub fn reduction_ratio(&self) -> f64 {
        if self.original_chars == 0 {
            return 0.0;
        }
        1.0 - (self.compact_chars as f64 / self.original_chars as f64)
    }

    /// Full system-prompt block: signatures + call instruction.
    pub fn prompt_block(&self) -> String {
        let mut block = String::from("Available tools:\n");
        for sig in &self.signatures {
            block.push_str(sig);
            block.push('\n');
        }
        block.push('\n');
        block.push_str(&self.call_instruction);
        block
    }
}

/// The call-format instruction injected into every request that has tools.
/// This is a constant so callers can measure its token cost separately.
pub const CALL_INSTRUCTION: &str = "\
To call a tool, emit EXACTLY this format (no markdown, no prose before/after the marker):\n\
<<call tool_name {\"arg\":\"value\"}>>
Required args must be present. Use JSON for all values. Emit ONE call per turn.";

/// Compact a slice of raw tool definition `Value`s into [`CompactTools`].
///
/// Silently skips any tool that does not have a valid `function.name`.
/// Never fails — bad fields are ignored and rendered as `any`.
pub fn compact_tools(tools: &[Value]) -> CompactTools {
    let original_chars: usize = tools.iter().map(|t| t.to_string().len()).sum();

    let signatures: Vec<String> = tools
        .iter()
        .filter_map(|tool| compact_one(tool))
        .collect();

    let compact_chars: usize = signatures.iter().map(|s| s.len()).sum::<usize>()
        + CALL_INSTRUCTION.len();

    CompactTools {
        signatures,
        call_instruction: CALL_INSTRUCTION.to_string(),
        original_chars,
        compact_chars,
    }
}

/// Compact a single tool definition Value → one signature line.
fn compact_one(tool: &Value) -> Option<String> {
    let func = tool.get("function")?;
    let name = func.get("name")?.as_str()?;
    let description = func
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or("");

    let params = func
        .get("parameters")
        .and_then(|p| p.get("properties"))
        .and_then(Value::as_object);

    let required: Vec<&str> = func
        .get("parameters")
        .and_then(|p| p.get("required"))
        .and_then(Value::as_array)
        .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect())
        .unwrap_or_default();

    let mut req_params: Vec<String> = Vec::new();
    let mut opt_params: Vec<String> = Vec::new();

    if let Some(props) = params {
        // Sort keys alphabetically within each group for determinism
        let mut keys: Vec<&str> = props.keys().map(String::as_str).collect();
        keys.sort_unstable();

        for key in &keys {
            let schema = &props[*key];
            let type_str = render_type(schema);
            if required.contains(key) {
                req_params.push(format!("{key}:{type_str}"));
            } else {
                opt_params.push(format!("{key}?:{type_str}"));
            }
        }

        // Required params first, then optional — both already sorted
        // Re-sort required by their position in the `required` array to match spec order
        req_params.sort_by_key(|p| {
            let param_name = p.split(':').next().unwrap_or("");
            required.iter().position(|r| *r == param_name).unwrap_or(usize::MAX)
        });
    }

    let mut all_params = req_params;
    all_params.extend(opt_params);
    let params_str = all_params.join(", ");

    // Truncate description at 80 chars
    let desc = if description.len() > 80 {
        format!("{}…", &description[..79])
    } else {
        description.to_string()
    };

    let sig = if desc.is_empty() {
        format!("{name}({params_str})")
    } else {
        format!("{name}({params_str}) - {desc}")
    };

    Some(sig)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn calendar_tool() -> Value {
        json!({
            "type": "function",
            "function": {
                "name": "create_calendar_event",
                "description": "Create an event in the user's calendar.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "title":       {"type": "string", "description": "Event title"},
                        "start":       {"type": "string", "format": "date-time"},
                        "attendees":   {"type": "array", "items": {"type": "string"}},
                        "visibility":  {"type": "string", "enum": ["public", "private"]}
                    },
                    "required": ["title", "start"]
                }
            }
        })
    }

    #[test]
    fn compact_calendar_tool() {
        let result = compact_tools(&[calendar_tool()]);
        assert_eq!(result.signatures.len(), 1);
        let sig = &result.signatures[0];
        // Required params first
        assert!(sig.starts_with("create_calendar_event(title:str, start:datetime"));
        // Optional params with ?
        assert!(sig.contains("attendees?:[str]"));
        assert!(sig.contains("visibility?:public|private"));
        // Description
        assert!(sig.contains("Create an event"));
        println!("Signature: {sig}");
    }

    #[test]
    fn reduction_ratio_positive() {
        let result = compact_tools(&[calendar_tool()]);
        let ratio = result.reduction_ratio();
        println!("Token reduction: {:.1}%", ratio * 100.0);
        assert!(ratio > 0.0, "compact should always be smaller than original");
    }

    #[test]
    fn no_tools_no_panic() {
        let result = compact_tools(&[]);
        assert!(result.signatures.is_empty());
        assert_eq!(result.reduction_ratio(), 0.0);
    }

    #[test]
    fn prompt_block_has_instruction() {
        let result = compact_tools(&[calendar_tool()]);
        let block = result.prompt_block();
        assert!(block.contains("<<call"));
        assert!(block.contains("create_calendar_event"));
    }
}

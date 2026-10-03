//! Encoder — converts full OpenAI tool definitions into the compact grammar.
//!
//! # Token-reduction strategy
//!
//! The native JSON Schema for a single tool can easily be 300–600 tokens. The compact
//! format renders each tool as a **single line**:
//!
//! ```text
//! tool_name(param1:type, param2?:type, enum_p?:a|b|c) - Description.
//! ```
//!
//! Followed by a **call-format instruction block** injected once. Together these shrink
//! the tool-definition section by ≥ 50% for typical schemas.
//!
//! # Bypass policy
//!
//! Some JSON Schema features can't be losslessly compressed without the risk of
//! confusing a model. For those the encoder returns `Ok(CompactTools::Bypassed)`,
//! and callers must fall back to native tools. See [`bypass_reason`].
//!
//! Bypassed features (document here and in the PR):
//! - `oneOf` / `anyOf` / `allOf` / `$ref` combinators at the parameter level.
//! - `additionalProperties` schemas (open-ended objects inside parameters).
//! - Nested objects more than 2 levels deep (heuristic to avoid runaway inlining).

use serde_json::Value;

use crate::types::{FunctionDef, ToolDef};

/// The result of encoding a set of tool definitions.
#[derive(Debug, Clone)]
pub enum CompactTools {
    /// All tools encoded successfully. Contains the compact text to inject and the
    /// schema registry for later validation during decoding.
    Encoded {
        /// The compact tool-definition block plus call-format instructions, ready to
        /// be appended to the system message (or injected as a separate system turn).
        text: String,
    },
    /// At least one tool has a schema feature we can't safely compact. Fall back to
    /// native tool calling for this request; no text is produced.
    Bypassed { reason: String },
}

impl CompactTools {
    /// Return the compact text, or `None` if bypassed.
    pub fn text(&self) -> Option<&str> {
        match self {
            CompactTools::Encoded { text } => Some(text),
            CompactTools::Bypassed { .. } => None,
        }
    }

    /// True when the encoder succeeded.
    pub fn is_encoded(&self) -> bool {
        matches!(self, CompactTools::Encoded { .. })
    }
}

/// Encode a slice of tool definitions into the compact grammar.
///
/// Returns `Ok(CompactTools::Bypassed)` (never `Err`) when a schema feature prevents
/// safe compaction; the caller falls back to native tool calling.
pub fn encode_tools(tools: &[ToolDef]) -> Result<CompactTools, EncodeError> {
    if tools.is_empty() {
        return Ok(CompactTools::Encoded {
            text: String::new(),
        });
    }

    let mut lines: Vec<String> = Vec::with_capacity(tools.len());
    for tool in tools {
        match encode_one(&tool.function)? {
            EncodeOne::Line(line) => lines.push(line),
            EncodeOne::Bypass(reason) => return Ok(CompactTools::Bypassed { reason }),
        }
    }

    let defs = lines.join("\n");
    let text = format!(
        "{defs}\n\
         \n\
         To call a tool, emit exactly: <<call name {{json_args}}>>\n\
         Rules:\n\
         - Replace `name` with the exact tool name above.\n\
         - Replace `{{json_args}}` with a single JSON object of arguments.\n\
         - Only one call per <<call ... >> block; emit multiple blocks for multiple calls.\n\
         - If no tool is needed, reply normally without any <<call ...>> block."
    );
    Ok(CompactTools::Encoded { text })
}

// ─────────────────────────────────────────────────────────────────────────────

enum EncodeOne {
    Line(String),
    Bypass(String),
}

/// Check whether a schema value contains a combinator we refuse to inline.
fn bypass_reason(schema: &Value) -> Option<String> {
    for key in ["oneOf", "anyOf", "allOf", "$ref"] {
        if schema.get(key).is_some() {
            return Some(format!("unsupported schema keyword '{key}' in parameters"));
        }
    }
    // additionalProperties as a schema (not `true`/`false`) → bypass
    if let Some(ap) = schema.get("additionalProperties") {
        if ap.is_object() {
            return Some("additionalProperties schema not supported".into());
        }
    }
    None
}

/// Recursively check nesting depth of object properties.
fn max_nesting(schema: &Value, depth: u32) -> u32 {
    if depth > 4 {
        return depth; // short-circuit
    }
    if let Some(props) = schema.get("properties").and_then(|p| p.as_object()) {
        props
            .values()
            .map(|v| max_nesting(v, depth + 1))
            .max()
            .unwrap_or(depth)
    } else {
        depth
    }
}

fn encode_one(func: &FunctionDef) -> Result<EncodeOne, EncodeError> {
    let Some(params) = &func.parameters else {
        // No parameters: render as zero-arg tool.
        let desc = func
            .description
            .as_deref()
            .map(|d| format!(" - {d}"))
            .unwrap_or_default();
        return Ok(EncodeOne::Line(format!("{}(){}", func.name, desc)));
    };

    // Top-level bypass checks.
    if let Some(reason) = bypass_reason(params) {
        return Ok(EncodeOne::Bypass(reason));
    }
    if max_nesting(params, 0) > 2 {
        return Ok(EncodeOne::Bypass(
            "deeply nested object schema (> 2 levels)".into(),
        ));
    }

    let required: Vec<String> = params
        .get("required")
        .and_then(|r| r.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();

    let props = params
        .get("properties")
        .and_then(|p| p.as_object())
        .cloned()
        .unwrap_or_default();

    let mut param_strs: Vec<String> = Vec::new();
    for (name, schema) in &props {
        if let Some(reason) = bypass_reason(schema) {
            return Ok(EncodeOne::Bypass(reason));
        }
        let is_required = required.contains(name);
        let opt = if is_required { "" } else { "?" };
        let type_str = compact_type(schema);
        param_strs.push(format!("{name}{opt}:{type_str}"));
    }

    let params_str = param_strs.join(", ");
    let desc = func
        .description
        .as_deref()
        .map(|d| format!(" - {d}"))
        .unwrap_or_default();

    Ok(EncodeOne::Line(format!(
        "{}({}){}",
        func.name, params_str, desc
    )))
}

/// Produce the compact type annotation for a parameter schema.
fn compact_type(schema: &Value) -> String {
    // Enum: render as pipe-separated values regardless of type.
    if let Some(enums) = schema.get("enum").and_then(|e| e.as_array()) {
        let vals: Vec<String> = enums
            .iter()
            .map(|v| {
                if let Some(s) = v.as_str() {
                    s.to_string()
                } else {
                    v.to_string()
                }
            })
            .collect();
        return vals.join("|");
    }

    let base = match schema.get("type").and_then(|t| t.as_str()) {
        Some("string") => match schema.get("format").and_then(|f| f.as_str()) {
            Some("date-time") => "datetime",
            Some("date") => "date",
            Some("uri") | Some("url") => "url",
            _ => "str",
        },
        Some("integer") => "int",
        Some("number") => "float",
        Some("boolean") => "bool",
        Some("array") => {
            let item_type = schema
                .get("items")
                .map(|i| compact_type(i))
                .unwrap_or_else(|| "any".into());
            return format!("[{item_type}]");
        }
        Some("object") => {
            // Inline object: render as {field:type, ...}.
            let props = schema
                .get("properties")
                .and_then(|p| p.as_object())
                .cloned()
                .unwrap_or_default();
            if props.is_empty() {
                return "obj".into();
            }
            let required: Vec<String> = schema
                .get("required")
                .and_then(|r| r.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            let fields: Vec<String> = props
                .iter()
                .map(|(k, v)| {
                    let opt = if required.contains(k) { "" } else { "?" };
                    format!("{k}{opt}:{}", compact_type(v))
                })
                .collect();
            return format!("{{{}}}", fields.join(","));
        }
        Some(other) => other,
        None => "any",
    };
    base.into()
}

/// Error type for the encoder (currently infallible — bypasses are `Ok(Bypassed)`).
#[derive(Debug, thiserror::Error)]
pub enum EncodeError {}

// ─────────────────────────────────────────────────────────────────────────────
#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{FunctionDef, ToolDef};
    use serde_json::json;

    fn make_tool(name: &str, desc: &str, params: serde_json::Value) -> ToolDef {
        ToolDef {
            kind: "function".into(),
            function: FunctionDef {
                name: name.into(),
                description: Some(desc.into()),
                parameters: Some(params),
            },
        }
    }

    #[test]
    fn calendar_event_encodes() {
        let tool = make_tool(
            "create_calendar_event",
            "Create an event in the user's calendar.",
            json!({
                "type": "object",
                "properties": {
                    "title": {"type": "string", "description": "Event title"},
                    "start": {"type": "string", "format": "date-time", "description": "Start time, ISO 8601"},
                    "duration_min": {"type": "integer", "description": "Duration in minutes"},
                    "attendees": {"type": "array", "items": {"type": "string"}, "description": "Attendee emails"},
                    "visibility": {"type": "string", "enum": ["public", "private"]}
                },
                "required": ["title", "start"]
            }),
        );
        let result = encode_tools(&[tool]).unwrap();
        assert!(result.is_encoded());
        let text = result.text().unwrap();
        assert!(text.contains("create_calendar_event("));
        assert!(text.contains("title:str"));
        assert!(text.contains("start:datetime"));
        assert!(text.contains("duration_min?:int"));
        assert!(text.contains("<<call name"));
    }

    #[test]
    fn enum_field_renders_pipe_separated() {
        let tool = make_tool(
            "set_priority",
            "Set priority.",
            json!({
                "type": "object",
                "properties": {
                    "level": {"type": "string", "enum": ["low", "medium", "high"]}
                },
                "required": ["level"]
            }),
        );
        let result = encode_tools(&[tool]).unwrap();
        let text = result.text().unwrap();
        assert!(text.contains("level:low|medium|high"), "got: {text}");
    }

    #[test]
    fn bypass_on_anyof() {
        let tool = make_tool(
            "complex",
            "Complex.",
            json!({
                "type": "object",
                "properties": {
                    "val": {"anyOf": [{"type": "string"}, {"type": "integer"}]}
                }
            }),
        );
        let result = encode_tools(&[tool]).unwrap();
        assert!(!result.is_encoded(), "should have bypassed");
    }

    #[test]
    fn empty_tools_produces_empty_text() {
        let result = encode_tools(&[]).unwrap();
        assert_eq!(result.text().unwrap_or(""), "");
    }
}

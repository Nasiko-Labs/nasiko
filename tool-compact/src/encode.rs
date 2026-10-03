//! ToolDef → compact prompt text.

use crate::error::{CompactError, Result};
use crate::schema::{params_to_schema, parse_parameters};
use crate::types::{CompactToolEntry, CompactTools, ParamSpec, ToolDef, TypeExpr};

/// Max characters of a tool-level description kept in the signature line.
const DESC_MAX: usize = 120;
/// Max characters of a parameter description kept as a hint.
const PARAM_DESC_MAX: usize = 40;

/// Call-format instructions appended after tool signatures.
///
/// Kept short for token savings, but explicit enough for live model adherence.
pub const CALL_INSTRUCTIONS: &str = "\
COMPACT TOOL MODE: if a listed tool applies, emit ONLY <<call TOOL_NAME {\"key\":\"value\"}>> (exact name; valid JSON; required+enums exact; never invent values; copy user wording for titles/subjects/bodies/emails; omit optional fields the user did not mention; datetimes ISO-8601 from reference). One call per action. No fences/native tool_calls. No tool → plain text.
Example: <<call example_tool {\"query\":\"status\"}>>";

/// Build a short reference-time preamble for relative date/time resolution.
///
/// Pure helper — callers supply date/timezone (eval harness or router config).
/// Does not hard-code evaluation case answers.
pub fn reference_time_preamble(date: &str, timezone: &str) -> String {
    format!(
        "Reference date: {date}\n\
Timezone: {timezone}\n\
Resolve relative dates/times vs this reference as ISO-8601 with the correct offset. \
'today'/'tomorrow' are calendar days; a weekday name means the next occurrence of that weekday on or after the reference date."
    )
}

/// Encode tool definitions into a compact prompt block.
///
/// Fails closed on unsupported JSON Schema features — the router should bypass compaction.
pub fn encode_tools(tools: &[ToolDef]) -> Result<CompactTools> {
    encode_tools_inner(tools)
}

/// Encode tool signatures + call instructions into a compact prompt block.
pub fn encode_tools_inner(tools: &[ToolDef]) -> Result<CompactTools> {
    if tools.is_empty() {
        return Err(CompactError::InvalidSchema("no tools to encode".into()));
    }

    let mut entries = Vec::with_capacity(tools.len());
    let mut lines = Vec::with_capacity(tools.len());

    for tool in tools {
        if tool.name.is_empty() {
            return Err(CompactError::InvalidSchema("tool name is empty".into()));
        }
        if !is_valid_tool_name(&tool.name) {
            return Err(CompactError::InvalidSchema(format!(
                "tool name {:?} is not a valid identifier",
                tool.name
            )));
        }
        let params = parse_parameters(tool.parameters.as_ref())?;
        let signature = format_signature(&tool.name, &params, tool.description.as_deref());
        lines.push(signature.clone());
        entries.push(CompactToolEntry {
            name: tool.name.clone(),
            description: tool.description.clone(),
            signature,
            params,
        });
    }

    let mut prompt = lines.join("\n");
    prompt.push('\n');
    prompt.push('\n');
    prompt.push_str(CALL_INSTRUCTIONS);

    Ok(CompactTools { prompt, entries })
}

/// Reconstruct [`ToolDef`]s from a [`CompactTools`] (semantic round-trip).
pub fn decode_tools(compact: &CompactTools) -> Result<Vec<ToolDef>> {
    Ok(compact
        .entries
        .iter()
        .map(|e| ToolDef {
            name: e.name.clone(),
            description: e.description.clone(),
            parameters: Some(params_to_schema(&e.params)),
        })
        .collect())
}

fn is_valid_tool_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

fn format_signature(name: &str, params: &[ParamSpec], description: Option<&str>) -> String {
    let mut s = String::new();
    s.push_str(name);
    s.push('(');
    for (i, p) in params.iter().enumerate() {
        if i > 0 {
            s.push_str(", ");
        }
        s.push_str(&p.name);
        if !p.required {
            s.push('?');
        }
        s.push(':');
        s.push_str(&format_type_expr(&p.type_expr));
        if let Some(hint) = p
            .description
            .as_deref()
            .map(str::trim)
            .filter(|d| !d.is_empty())
        {
            let clipped = clip_chars(hint, PARAM_DESC_MAX);
            s.push('(');
            s.push_str(&clipped);
            s.push(')');
        }
    }
    s.push(')');
    if let Some(desc) = description.map(str::trim).filter(|d| !d.is_empty()) {
        s.push_str(" - ");
        s.push_str(&clip_chars(desc, DESC_MAX));
    }
    s
}

fn clip_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

fn format_type_expr(expr: &TypeExpr) -> String {
    match expr {
        TypeExpr::Str => "str".into(),
        TypeExpr::Datetime => "datetime".into(),
        TypeExpr::Date => "date".into(),
        TypeExpr::Time => "time".into(),
        TypeExpr::Uri => "uri".into(),
        TypeExpr::Int => "int".into(),
        TypeExpr::Num => "num".into(),
        TypeExpr::Bool => "bool".into(),
        TypeExpr::Null => "null".into(),
        TypeExpr::Enum(variants) => variants.join("|"),
        TypeExpr::Array(inner) => format!("[{}]", format_type_expr(inner)),
        TypeExpr::OpaqueObject => "obj".into(),
        TypeExpr::Object(fields) => {
            let mut s = String::from("{");
            for (i, f) in fields.iter().enumerate() {
                if i > 0 {
                    s.push_str(", ");
                }
                s.push_str(&f.name);
                if !f.required {
                    s.push('?');
                }
                s.push(':');
                s.push_str(&format_type_expr(&f.type_expr));
            }
            s.push('}');
            s
        }
    }
}

/// Render a single call in the compact wire grammar (for eval / tests).
pub fn render_call(call: &crate::types::ToolCall) -> Result<String> {
    if !is_valid_tool_name(&call.name) {
        return Err(CompactError::MalformedCall(format!(
            "invalid tool name {}",
            call.name
        )));
    }
    let args = serde_json::to_string(&call.arguments)
        .map_err(|e| CompactError::InvalidJson(format!("serialize arguments: {e}")))?;
    Ok(format!("<<call {} {}>>", call.name, args))
}

/// Render multiple calls, one per line.
pub fn render_calls(calls: &[crate::types::ToolCall]) -> Result<String> {
    let mut lines = Vec::with_capacity(calls.len());
    for c in calls {
        lines.push(render_call(c)?);
    }
    Ok(lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::types::ToolDef;

    fn calendar_tool() -> ToolDef {
        ToolDef {
            name: "create_calendar_event".into(),
            description: Some("Create an event in the user's calendar.".into()),
            parameters: Some(json!({
                "type": "object",
                "properties": {
                    "title": { "type": "string", "description": "Event title" },
                    "start": { "type": "string", "format": "date-time" },
                    "duration_min": { "type": "integer" },
                    "attendees": { "type": "array", "items": { "type": "string" } },
                    "visibility": { "type": "string", "enum": ["public", "private"] }
                },
                "required": ["title", "start"]
            })),
        }
    }

    #[test]
    fn encodes_calendar_signature() {
        let compact = encode_tools(&[calendar_tool()]).unwrap();
        assert!(compact.prompt.contains(
            "create_calendar_event(attendees?:[str], duration_min?:int, start:datetime, title:str(Event title), visibility?:public|private)"
        ));
        assert!(compact.prompt.contains("COMPACT TOOL MODE"));
        assert!(compact.prompt.contains("example_tool"));
    }

    #[test]
    fn reference_preamble_is_generic() {
        let p = reference_time_preamble("2026-10-02", "Asia/Kolkata");
        assert!(p.contains("Reference date: 2026-10-02"));
        assert!(p.contains("Timezone: Asia/Kolkata"));
        assert!(!p.contains("ct-001"));
        assert!(!p.contains("riya@"));
    }

    #[test]
    fn rejects_any_of() {
        let tool = ToolDef {
            name: "weird".into(),
            description: None,
            parameters: Some(json!({
                "type": "object",
                "properties": {
                    "x": { "anyOf": [{ "type": "string" }, { "type": "number" }] }
                }
            })),
        };
        let err = encode_tools(&[tool]).unwrap_err();
        assert!(matches!(err, CompactError::UnsupportedSchema(_)));
    }

    #[test]
    fn roundtrip_decode_tools() {
        let tools = vec![calendar_tool()];
        let compact = encode_tools(&tools).unwrap();
        let back = decode_tools(&compact).unwrap();
        assert_eq!(back[0].name, "create_calendar_event");
        let props = back[0].parameters.as_ref().unwrap()["properties"]
            .as_object()
            .unwrap();
        assert!(props.contains_key("title"));
        assert!(props.contains_key("start"));
        assert_eq!(props["visibility"]["enum"], json!(["public", "private"]));
    }
}

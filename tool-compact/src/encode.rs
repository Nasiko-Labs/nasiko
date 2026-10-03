//! Compact tool encoding and schema round-trip.

use serde_json::{Map, Value};

use crate::schema::check_supported;
use crate::types::{CompactError, CompactTools, ToolCall, ToolDef};

/// Call-format instructions appended after every compacted tool list.
///
/// Kept short — this text is paid on every compacted request. Grammar details that
/// matter for decoding (`>>` inside JSON strings) are stated once.
pub const CALL_FORMAT: &str =
    "Call tools as <<call name {json}>> (one per line). No marker = no call. \
     >> inside JSON strings is literal; close the object before >>.";

/// Encode tools into a compact prompt fragment.
///
/// Returns [`CompactError::UnsupportedSchema`] when any tool uses a feature we cannot
/// preserve — callers must bypass compaction for that request (fail closed at the seam).
pub fn encode_tools(tools: &[ToolDef]) -> Result<CompactTools, CompactError> {
    if tools.is_empty() {
        return Ok(CompactTools {
            prompt: String::new(),
            tools: Vec::new(),
        });
    }

    let mut lines = Vec::with_capacity(tools.len() + 2);
    let mut kept = Vec::with_capacity(tools.len());

    for tool in tools {
        validate_name(&tool.name)?;
        check_supported(tool.parameters.as_ref())?;
        lines.push(render_tool_line(tool)?);
        kept.push(tool.clone());
    }

    lines.push(String::new());
    lines.push(CALL_FORMAT.to_string());

    Ok(CompactTools {
        prompt: lines.join("\n"),
        tools: kept,
    })
}

/// Reconstruct [`ToolDef`]s from a [`CompactTools`] (schema meaning preserved).
pub fn decode_tools(compact: &CompactTools) -> Result<Vec<ToolDef>, CompactError> {
    Ok(compact.tools.clone())
}

/// Render expected / known calls in the compact call grammar (for eval round-trips).
pub fn render_calls(calls: &[ToolCall]) -> Result<String, CompactError> {
    let mut out = Vec::with_capacity(calls.len());
    for call in calls {
        // Compact JSON (no spaces) keeps marker splits stable relative to the public set.
        let args: Value = serde_json::from_str(&call.arguments)
            .map_err(|_| CompactError::InvalidArguments)?;
        let compact_args =
            serde_json::to_string(&args).map_err(|_| CompactError::InvalidArguments)?;
        out.push(format!("<<call {} {}>>", call.name, compact_args));
    }
    Ok(out.join("\n"))
}

/// Build a [`ToolCall`] from name + JSON value (arguments become a JSON string).
pub fn tool_call(name: impl Into<String>, arguments: &Value) -> Result<ToolCall, CompactError> {
    let arguments =
        serde_json::to_string(arguments).map_err(|_| CompactError::InvalidArguments)?;
    Ok(ToolCall {
        name: name.into(),
        arguments,
    })
}

fn validate_name(name: &str) -> Result<(), CompactError> {
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_')
        || name.chars().next().is_some_and(|c| c.is_ascii_digit())
    {
        return Err(CompactError::InvalidToolName(name.into()));
    }
    Ok(())
}

fn render_tool_line(tool: &ToolDef) -> Result<String, CompactError> {
    let sig = render_signature(tool.parameters.as_ref())?;
    match tool.description.as_deref().map(shorten_description) {
        Some(desc) if !desc.is_empty() => Ok(format!("{}({}) - {}", tool.name, sig, desc)),
        _ => Ok(format!("{}({})", tool.name, sig)),
    }
}

fn shorten_description(s: &str) -> String {
    let trimmed = s.split_whitespace().collect::<Vec<_>>().join(" ");
    // Keep enough to disambiguate; cut filler. Never drop entirely when present.
    const MAX: usize = 80;
    if trimmed.chars().count() <= MAX {
        return trimmed;
    }
    let mut out = String::new();
    for (i, ch) in trimmed.chars().enumerate() {
        if i + 1 >= MAX {
            out.push('…');
            break;
        }
        out.push(ch);
    }
    out
}

fn render_signature(parameters: Option<&Value>) -> Result<String, CompactError> {
    let Some(schema) = parameters else {
        return Ok(String::new());
    };
    let Value::Object(map) = schema else {
        return Err(CompactError::UnsupportedSchema(
            "parameters must be object schema".into(),
        ));
    };
    let props = map
        .get("properties")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let required: Vec<&str> = map
        .get("required")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();

    // Required fields in the schema's `required` array order, then optional in
    // properties insertion order — matches OpenAI examples and stays deterministic.
    let mut ordered = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for key in &required {
        if let Some(prop) = props.get(*key) {
            ordered.push(render_field(key, prop, true)?);
            seen.insert(*key);
        }
    }
    for (name, prop) in &props {
        if seen.contains(name.as_str()) {
            continue;
        }
        ordered.push(render_field(name, prop, false)?);
    }
    Ok(ordered.join(", "))
}

fn render_field(name: &str, schema: &Value, required: bool) -> Result<String, CompactError> {
    let ty = render_type(schema)?;
    if required {
        Ok(format!("{name}:{ty}"))
    } else {
        Ok(format!("{name}?:{ty}"))
    }
}

fn render_type(schema: &Value) -> Result<String, CompactError> {
    let Value::Object(map) = schema else {
        return Err(CompactError::UnsupportedSchema(
            "property schema must be object".into(),
        ));
    };

    if let Some(enum_vals) = map.get("enum").and_then(Value::as_array) {
        let parts: Result<Vec<_>, _> = enum_vals.iter().map(render_enum_literal).collect();
        return Ok(parts?.join("|"));
    }

    let ty = map
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("object");

    match ty {
        "string" => {
            if map.get("format").and_then(Value::as_str) == Some("date-time") {
                Ok("datetime".into())
            } else {
                Ok("str".into())
            }
        }
        "integer" => Ok("int".into()),
        "number" => Ok("float".into()),
        "boolean" => Ok("bool".into()),
        "null" => Ok("null".into()),
        "array" => {
            let empty = Value::Object(Map::new());
            let items = map.get("items").unwrap_or(&empty);
            let inner = render_type(items)?;
            Ok(format!("[{inner}]"))
        }
        "object" => {
            if let Some(props) = map.get("properties").and_then(Value::as_object) {
                let required: Vec<&str> = map
                    .get("required")
                    .and_then(Value::as_array)
                    .map(|a| a.iter().filter_map(Value::as_str).collect())
                    .unwrap_or_default();
                let mut fields = Vec::new();
                for (n, p) in props {
                    fields.push(render_field(n, p, required.contains(&n.as_str()))?);
                }
                Ok(format!("{{{}}}", fields.join(", ")))
            } else {
                Ok("object".into())
            }
        }
        other => Err(CompactError::UnsupportedSchema(format!(
            "type {other}"
        ))),
    }
}

fn render_enum_literal(v: &Value) -> Result<String, CompactError> {
    match v {
        Value::String(s) => {
            if s.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
            {
                Ok(s.clone())
            } else {
                Ok(serde_json::to_string(v).map_err(|_| {
                    CompactError::UnsupportedSchema("enum literal".into())
                })?)
            }
        }
        Value::Number(n) => Ok(n.to_string()),
        Value::Bool(b) => Ok(b.to_string()),
        Value::Null => Ok("null".into()),
        _ => Err(CompactError::UnsupportedSchema(
            "complex enum literal".into(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn calendar() -> ToolDef {
        ToolDef {
            name: "create_calendar_event".into(),
            description: Some("Create an event in the user's calendar.".into()),
            parameters: Some(json!({
                "type": "object",
                "properties": {
                    "title": {"type": "string", "description": "Event title"},
                    "start": {"type": "string", "format": "date-time"},
                    "duration_min": {"type": "integer"},
                    "attendees": {"type": "array", "items": {"type": "string"}},
                    "visibility": {"type": "string", "enum": ["public", "private"]}
                },
                "required": ["title", "start"]
            })),
        }
    }

    #[test]
    fn encode_contains_signature_and_call_format() {
        let c = encode_tools(&[calendar()]).unwrap();
        assert!(
            c.prompt.contains("create_calendar_event(") && c.prompt.contains("title:str"),
            "prompt was: {}",
            c.prompt
        );
        assert!(
            c.prompt.contains("start:datetime"),
            "prompt was: {}",
            c.prompt
        );
        assert!(c.prompt.contains("visibility?:public|private"));
        assert!(c.prompt.contains("<<call name {json}>>"));
        assert!(c.prompt.contains("Create an event"));
    }

    #[test]
    fn decode_tools_roundtrip() {
        let tools = vec![calendar()];
        let c = encode_tools(&tools).unwrap();
        assert_eq!(decode_tools(&c).unwrap(), tools);
    }
}

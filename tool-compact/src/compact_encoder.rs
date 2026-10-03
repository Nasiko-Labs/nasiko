use crate::error::{Result, ToolCompactError};
use crate::schema::{ParameterSchema, ToolSchema, ValueType};
use crate::types::ToolDef;
use std::collections::HashSet;

/// Call-format instructions added once per request (see GRAMMAR.md).
pub const CALL_INSTRUCTIONS: &str = "To call a tool, emit <<call name {json args}>> where args is one JSON object using only the listed parameters (name? = optional). Several calls are allowed; if no tool is needed, answer normally.";

/// Compact tool definitions plus call-format instructions.
#[derive(Debug, Clone, PartialEq)]
pub struct CompactTools {
    /// One line per tool, in the order the tools were given.
    pub definitions: Vec<String>,
    pub instructions: String,
}

impl CompactTools {
    /// The text to inject into the request (for example in the system message).
    pub fn prompt(&self) -> String {
        format!("Tools:\n{}\n{}", self.definitions.join("\n"), self.instructions)
    }
}

/// Encodes OpenAI-shaped tool definitions into the compact format.
///
/// Fails closed: any schema feature we cannot express losslessly returns
/// `ToolCompactError::SchemaError("unsupported ...")`; callers must then send the request
/// with the native tool definitions (bypass).
pub fn encode_tools(tools: &[ToolDef]) -> Result<CompactTools> {
    let mut seen = HashSet::new();
    let mut definitions = Vec::with_capacity(tools.len());
    for def in tools {
        let schema = schema_from_def(def)?;
        if !seen.insert(schema.name.clone()) {
            return Err(unsupported(&schema.name, "duplicate tool name"));
        }
        definitions.push(encode_tool_line(&schema)?);
    }
    Ok(CompactTools {
        definitions,
        instructions: CALL_INSTRUCTIONS.to_string(),
    })
}

/// Converts an OpenAI-shaped `ToolDef` into the internal `ToolSchema`.
pub fn schema_from_def(def: &ToolDef) -> Result<ToolSchema> {
    if def.kind != "function" {
        return Err(unsupported(&def.function.name, "tool type is not 'function'"));
    }
    let value = serde_json::to_value(def)
        .map_err(|e| ToolCompactError::SerializationError(e.to_string()))?;
    ToolSchema::from_json_value(&value)
}

fn unsupported(path: &str, msg: &str) -> ToolCompactError {
    ToolCompactError::SchemaError(format!("unsupported schema at '{path}': {msg}"))
}

fn is_ident(s: &str) -> bool {
    !s.is_empty()
        && s
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.')
}

/// Collapses whitespace and replaces double quotes so a description stays on one line.
fn clean(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ").replace('"', "'")
}

fn encode_tool_line(t: &ToolSchema) -> Result<String> {
    if !is_ident(&t.name) {
        return Err(unsupported(&t.name, "tool name has unsupported characters"));
    }
    let mut out = format!("{}({})", t.name, encode_params(&t.parameters, &t.name)?);
    let desc = clean(&t.description);
    if !desc.is_empty() {
        out.push_str(" - ");
        out.push_str(&desc);
    }
    Ok(out)
}

/// Required parameters first, then optional; alphabetical inside each group, so the
/// output is deterministic whatever order the schema used.
fn encode_params(params: &[ParameterSchema], path: &str) -> Result<String> {
    let mut ordered: Vec<&ParameterSchema> = params.iter().collect();
    ordered.sort_by(|a, b| (!a.required, &a.name).cmp(&(!b.required, &b.name)));
    let parts = ordered
        .iter()
        .map(|p| encode_param(p, path))
        .collect::<Result<Vec<_>>>()?;
    Ok(parts.join(", "))
}

fn encode_param(p: &ParameterSchema, parent: &str) -> Result<String> {
    if !is_ident(&p.name) {
        return Err(unsupported(parent, "parameter name has unsupported characters"));
    }
    let here = format!("{parent}.{}", p.name);
    let mut s = String::new();
    s.push_str(&p.name);
    if !p.required {
        s.push('?');
    }
    s.push(':');
    s.push_str(&encode_type(p, &here)?);
    if let Some(d) = p.description.as_deref() {
        let d = clean(d);
        if !d.is_empty() {
            s.push_str(" \"");
            s.push_str(&d);
            s.push('"');
        }
    }
    Ok(s)
}

fn encode_type(p: &ParameterSchema, path: &str) -> Result<String> {
    Ok(match p.val_type {
        ValueType::String => {
            if let Some(vals) = &p.enum_values {
                if vals.is_empty() {
                    return Err(unsupported(path, "empty enum"));
                }
                if vals.iter().any(|v| !is_ident(v)) {
                    return Err(unsupported(path, "enum value has unsupported characters"));
                }
                vals.join("|")
            } else if p.format.as_deref() == Some("date-time") {
                "datetime".to_string()
            } else {
                "str".to_string()
            }
        }
        ValueType::Integer => "int".to_string(),
        ValueType::Number => "num".to_string(),
        ValueType::Boolean => "bool".to_string(),
        ValueType::Array => {
            let item = p
                .item_type
                .as_deref()
                .ok_or_else(|| unsupported(path, "array without item type"))?;
            format!("[{}]", primitive_name(item, path)?)
        }
        ValueType::Object => match &p.properties {
            Some(props) => format!("{{{}}}", encode_params(props, path)?),
            None => "object".to_string(),
        },
    })
}

fn primitive_name(t: &ValueType, path: &str) -> Result<&'static str> {
    match t {
        ValueType::String => Ok("str"),
        ValueType::Integer => Ok("int"),
        ValueType::Number => Ok("num"),
        ValueType::Boolean => Ok("bool"),
        _ => Err(unsupported(path, "array items must be primitive")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tool(v: serde_json::Value) -> ToolDef {
        serde_json::from_value(v).unwrap()
    }

    fn calendar() -> ToolDef {
        tool(json!({
            "type": "function",
            "function": {
                "name": "create_calendar_event",
                "description": "Create an event in the user's calendar.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "title": {"type": "string", "description": "Event title"},
                        "start": {"type": "string", "format": "date-time", "description": "Start time, ISO 8601"},
                        "duration_min": {"type": "integer", "description": "Duration in minutes"},
                        "attendees": {"type": "array", "items": {"type": "string"}, "description": "Attendee emails"},
                        "visibility": {"type": "string", "enum": ["public", "private"]}
                    },
                    "required": ["title", "start"]
                }
            }
        }))
    }

    #[test]
    fn encodes_the_brief_example() {
        let c = encode_tools(&[calendar()]).unwrap();
        assert_eq!(
            c.definitions[0],
            "create_calendar_event(start:datetime \"Start time, ISO 8601\", title:str \"Event title\", attendees?:[str] \"Attendee emails\", duration_min?:int \"Duration in minutes\", visibility?:public|private) - Create an event in the user's calendar."
        );
    }

    #[test]
    fn prompt_contains_definitions_and_call_instructions() {
        let p = encode_tools(&[calendar()]).unwrap().prompt();
        assert!(p.starts_with("Tools:\ncreate_calendar_event("));
        assert!(p.contains("<<call name {json args}>>"));
    }

    #[test]
    fn nested_objects_and_no_description() {
        let t = tool(json!({
            "type": "function",
            "function": {
                "name": "search",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "filter": {
                            "type": "object",
                            "properties": {
                                "city": {"type": "string"},
                                "limit": {"type": "integer"}
                            },
                            "required": ["city"]
                        }
                    },
                    "required": ["filter"]
                }
            }
        }));
        let c = encode_tools(&[t]).unwrap();
        assert_eq!(c.definitions[0], "search(filter:{city:str, limit?:int})");
    }

    #[test]
    fn tool_without_parameters() {
        let t = tool(json!({"type":"function","function":{"name":"ping"}}));
        assert_eq!(encode_tools(&[t]).unwrap().definitions[0], "ping()");
    }

    #[test]
    fn unsupported_schema_is_rejected_for_bypass() {
        let t = tool(json!({
            "type": "function",
            "function": {
                "name": "f",
                "parameters": {
                    "type": "object",
                    "properties": {"x": {"anyOf": [{"type": "string"}, {"type": "integer"}]}}
                }
            }
        }));
        assert!(matches!(encode_tools(&[t]), Err(ToolCompactError::SchemaError(_))));
    }

    #[test]
    fn enum_value_with_separator_is_rejected() {
        let t = tool(json!({
            "type": "function",
            "function": {
                "name": "f",
                "parameters": {
                    "type": "object",
                    "properties": {"x": {"type": "string", "enum": ["a|b", "c"]}}
                }
            }
        }));
        assert!(matches!(encode_tools(&[t]), Err(ToolCompactError::SchemaError(_))));
    }

    #[test]
    fn duplicate_tool_names_are_rejected() {
        let t = tool(json!({"type":"function","function":{"name":"ping"}}));
        assert!(matches!(
            encode_tools(&[t.clone(), t]),
            Err(ToolCompactError::SchemaError(_))
        ));
    }
}
//! Encode full JSON Schema tool definitions into compact text.

use crate::error::CompactError;
use crate::types::{CompactTools, Param, SchemaType, ToolDef};
use serde_json::Value;

/// Call-format instruction — short but explicit enough for all models.
const CALL_INSTRUCTION: &str =
    r#"Reply <<call NAME {"k":"v"}>> to use a tool. No tool needed? Reply normally."#;

/// Encode a set of tool definitions into compact text with call-format instructions.
pub fn encode_tools(tools: &[ToolDef]) -> Result<CompactTools, CompactError> {
    let mut lines = Vec::new();
    let mut bypassed = Vec::new();
    let mut count = 0;

    for tool in tools {
        match encode_one(tool) {
            Ok(line) => {
                lines.push(line);
                count += 1;
            }
            Err(_) => {
                bypassed.push(tool.name.clone());
            }
        }
    }

    let mut text = String::new();
    if !lines.is_empty() {
        for line in &lines {
            text.push_str(line);
            text.push('\n');
        }
        text.push_str(CALL_INSTRUCTION);
    }

    Ok(CompactTools {
        text,
        tool_count: count,
        bypassed,
    })
}

fn encode_one(tool: &ToolDef) -> Result<String, CompactError> {
    let params = extract_params(tool)?;
    let mut sig = tool.name.clone();
    sig.push('(');

    let param_strs: Vec<String> = params.iter().map(render_param).collect();
    sig.push_str(&param_strs.join(", "));
    sig.push(')');

    if let Some(desc) = &tool.description
        && !is_description_redundant(&tool.name, desc)
    {
        sig.push_str(" - ");
        if desc.len() > 80 {
            sig.push_str(&desc[..77]);
            sig.push_str("...");
        } else {
            sig.push_str(desc);
        }
    }

    Ok(sig)
}

fn extract_params(tool: &ToolDef) -> Result<Vec<Param>, CompactError> {
    let schema = match &tool.parameters {
        Some(v) => v,
        None => return Ok(Vec::new()),
    };

    let props = match schema.get("properties") {
        Some(Value::Object(m)) => m,
        _ => return Ok(Vec::new()),
    };

    let required: Vec<String> = schema
        .get("required")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .map(String::from)
                .collect()
        })
        .unwrap_or_default();

    let mut params = Vec::new();
    // Preserve insertion order from the JSON object
    for (name, prop) in props {
        let typ = json_schema_to_type(prop);
        let desc = prop
            .get("description")
            .and_then(Value::as_str)
            .map(String::from);
        params.push(Param {
            name: name.clone(),
            typ,
            required: required.contains(name),
            description: desc,
        });
    }

    // Sort: required params first, then optional
    params.sort_by_key(|p| !p.required);
    Ok(params)
}

fn json_schema_to_type(schema: &Value) -> SchemaType {
    // Check for enum first
    if let Some(values) = schema.get("enum").and_then(Value::as_array) {
        let variants: Vec<String> = values
            .iter()
            .map(|v| match v {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            })
            .collect();
        return SchemaType::Enum(variants);
    }

    let type_str = schema.get("type").and_then(Value::as_str).unwrap_or("any");

    match type_str {
        "string" => {
            if schema.get("format").and_then(Value::as_str) == Some("date-time") {
                SchemaType::DateTime
            } else {
                SchemaType::Str
            }
        }
        "integer" => SchemaType::Int,
        "number" => SchemaType::Float,
        "boolean" => SchemaType::Bool,
        "array" => {
            let item_type = schema
                .get("items")
                .map(json_schema_to_type)
                .unwrap_or(SchemaType::Any);
            SchemaType::Array(Box::new(item_type))
        }
        "object" => {
            if let Some(Value::Object(props)) = schema.get("properties") {
                let required: Vec<String> = schema
                    .get("required")
                    .and_then(Value::as_array)
                    .map(|arr| {
                        arr.iter()
                            .filter_map(Value::as_str)
                            .map(String::from)
                            .collect()
                    })
                    .unwrap_or_default();

                let params: Vec<Param> = props
                    .iter()
                    .map(|(name, prop)| Param {
                        name: name.clone(),
                        typ: json_schema_to_type(prop),
                        required: required.contains(name),
                        description: prop
                            .get("description")
                            .and_then(Value::as_str)
                            .map(String::from),
                    })
                    .collect();
                SchemaType::Object(params)
            } else {
                SchemaType::Any
            }
        }
        _ => SchemaType::Any,
    }
}

/// Check if a description is mostly redundant with the tool name.
/// e.g. "create_calendar_event" + "Create an event in the user's calendar."
fn is_description_redundant(name: &str, desc: &str) -> bool {
    let name_lower = name.to_lowercase().replace('_', " ");
    let desc_lower = desc.to_lowercase();
    // If >50% of the name words appear in the description, it's redundant
    let name_words: Vec<&str> = name_lower.split_whitespace().collect();
    if name_words.is_empty() {
        return false;
    }
    let matches = name_words
        .iter()
        .filter(|w| desc_lower.contains(**w))
        .count();
    matches * 2 >= name_words.len()
}

fn render_param(param: &Param) -> String {
    let opt_marker = if param.required { "" } else { "?" };
    let type_str = render_type(&param.typ);
    format!("{}{opt_marker}:{type_str}", param.name)
}

fn render_type(typ: &SchemaType) -> String {
    match typ {
        SchemaType::Str => "str".to_string(),
        SchemaType::Int => "int".to_string(),
        SchemaType::Float => "float".to_string(),
        SchemaType::Bool => "bool".to_string(),
        SchemaType::DateTime => "dt".to_string(),
        SchemaType::Enum(variants) => variants.join("|"),
        SchemaType::Array(inner) => format!("[{}]", render_type(inner)),
        SchemaType::Object(params) => {
            let fields: Vec<String> = params
                .iter()
                .map(|p| {
                    let opt = if p.required { "" } else { "?" };
                    format!("{}{opt}:{}", p.name, render_type(&p.typ))
                })
                .collect();
            format!("{{{}}}", fields.join(", "))
        }
        SchemaType::Any => "any".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn calendar_tool() -> ToolDef {
        ToolDef {
            name: "create_calendar_event".to_string(),
            description: Some("Create an event in the user's calendar.".to_string()),
            parameters: Some(json!({
                "type": "object",
                "properties": {
                    "title": {"type": "string", "description": "Event title"},
                    "start": {"type": "string", "format": "date-time", "description": "Start time, ISO 8601"},
                    "duration_min": {"type": "integer", "description": "Duration in minutes"},
                    "attendees": {"type": "array", "items": {"type": "string"}, "description": "Attendee emails"},
                    "visibility": {"type": "string", "enum": ["public", "private"]}
                },
                "required": ["title", "start"]
            })),
        }
    }

    fn email_tool() -> ToolDef {
        ToolDef {
            name: "send_email".to_string(),
            description: Some("Send an email to one or more recipients.".to_string()),
            parameters: Some(json!({
                "type": "object",
                "properties": {
                    "to": {"type": "array", "items": {"type": "string"}, "description": "Recipient emails"},
                    "subject": {"type": "string", "description": "Subject line"},
                    "body": {"type": "string", "description": "Email body"},
                    "cc": {"type": "array", "items": {"type": "string"}, "description": "CC recipients"}
                },
                "required": ["to", "subject", "body"]
            })),
        }
    }

    #[test]
    fn encode_single_tool() {
        let result = encode_tools(&[calendar_tool()]).unwrap();
        assert_eq!(result.tool_count, 1);
        assert!(result.bypassed.is_empty());
        assert!(result.text.contains("create_calendar_event("));
        assert!(result.text.contains("title:str"));
        assert!(result.text.contains("start:dt"));
        assert!(result.text.contains("duration_min?:int"));
        assert!(result.text.contains("attendees?:[str]"));
        assert!(result.text.contains("visibility?:public|private"));
        assert!(result.text.contains("<<call"));
    }

    #[test]
    fn encode_multiple_tools() {
        let result = encode_tools(&[calendar_tool(), email_tool()]).unwrap();
        assert_eq!(result.tool_count, 2);
        assert!(result.text.contains("create_calendar_event("));
        assert!(result.text.contains("send_email("));
    }

    #[test]
    fn encode_no_params() {
        let tool = ToolDef {
            name: "get_time".to_string(),
            description: Some("Get the current time.".to_string()),
            parameters: None,
        };
        let result = encode_tools(&[tool]).unwrap();
        assert!(result.text.contains("get_time()"));
    }

    #[test]
    fn encode_enum_type() {
        let result = encode_tools(&[calendar_tool()]).unwrap();
        assert!(result.text.contains("public|private"));
    }

    #[test]
    fn encode_nested_object() {
        let tool = ToolDef {
            name: "create_task".to_string(),
            description: Some("Create a task.".to_string()),
            parameters: Some(json!({
                "type": "object",
                "properties": {
                    "title": {"type": "string"},
                    "assignee": {
                        "type": "object",
                        "properties": {
                            "name": {"type": "string"},
                            "email": {"type": "string"}
                        },
                        "required": ["name"]
                    }
                },
                "required": ["title"]
            })),
        };
        let result = encode_tools(&[tool]).unwrap();
        // JSON object key order is not guaranteed, so check parts individually
        assert!(result.text.contains("assignee?:{"));
        assert!(result.text.contains("name:str"));
        assert!(result.text.contains("email?:str"));
    }
}

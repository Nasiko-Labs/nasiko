use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDef {
    #[serde(rename = "type", default = "default_tool_kind")]
    pub kind: String,

    pub function: FunctionDef,

    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionDef {
    pub name: String,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parameters: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,

    #[serde(rename = "type", default = "default_tool_kind")]
    pub kind: String,

    pub function: FunctionCall,

    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionCall {
    pub name: String,
    pub arguments: String,
}

fn default_tool_kind() -> String {
    "function".to_string()
}
#[derive(Debug, Clone)]
pub struct CompactTools {
    pub text: String,
}

#[derive(Debug, Clone)]
pub struct CompactError {
    pub message: String,
}

impl std::fmt::Display for CompactError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for CompactError {}

pub type Result<T> = std::result::Result<T, CompactError>;
pub fn encode_tools(tools: &[ToolDef]) -> Result<CompactTools> {
    let mut output = String::new();

    for (index, tool) in tools.iter().enumerate() {
        if index > 0 {
            output.push('\n');
        }

        let function = &tool.function;

        output.push_str(&function.name);
        output.push('(');

        if let Some(parameters) = &function.parameters {
            if let Some(properties) = parameters
                .get("properties")
                .and_then(Value::as_object)
            {
                let required = parameters
                    .get("required")
                    .and_then(Value::as_array);

                let required_names: std::collections::HashSet<&str> = required
                    .map(|items| {
                        items
                            .iter()
                            .filter_map(Value::as_str)
                            .collect()
                    })
                    .unwrap_or_default();

                let mut fields: Vec<_> = properties.iter().collect();
                fields.sort_by(|(name_a, _), (name_b, _)| {
                    let required_a = required_names.contains(name_a.as_str());
                    let required_b = required_names.contains(name_b.as_str());
                    required_b.cmp(&required_a).then_with(|| name_a.cmp(name_b))
                });
                for (field_index, (name, schema)) in fields.iter().enumerate() {
                    if field_index > 0 {
                        output.push_str(", ");
                    }

                    output.push_str(name);

                    if !required_names.contains(name.as_str()) {
                        output.push('?');
                    }

                    output.push(':');
                    output.push_str(&schema_type(schema));
                }
            }
        }

        output.push(')');

        if let Some(description) = &function.description {
            output.push_str(" - ");
            output.push_str(description);
        }
    }

    Ok(CompactTools { text: output })
}
fn schema_type(schema: &Value) -> String {
    if let Some(enum_values) = schema.get("enum").and_then(Value::as_array) {
        let values: Vec<String> = enum_values
            .iter()
            .filter_map(|value| value.as_str().map(str::to_string))
            .collect();

        if !values.is_empty() {
            return values.join("|");
        }
    }

    match schema.get("type").and_then(Value::as_str) {
        Some("string") => {
            if schema
                .get("format")
                .and_then(Value::as_str)
                == Some("date-time")
            {
                "datetime".to_string()
            } else {
                "str".to_string()
            }
        }

        Some("integer") => "int".to_string(),

        Some("number") => "num".to_string(),

        Some("boolean") => "bool".to_string(),

        Some("array") => {
            if let Some(items) = schema.get("items") {
                format!("[{}]", schema_type(items))
            } else {
                "[unknown]".to_string()
            }
        }

        Some("object") => {
            if let Some(properties) = schema
                .get("properties")
                .and_then(Value::as_object)
            {
                let fields: Vec<String> = properties
                    .iter()
                    .map(|(name, value)| {
                        format!("{}:{}", name, schema_type(value))
                    })
                    .collect();

                format!("{{{}}}", fields.join(", "))
            } else {
                "object".to_string()
            }
        }

        _ => "unknown".to_string(),
    }

}
fn validate_arguments(schema: &Value, value: &Value, path: &str) -> Result<()> {
    let expected_type = schema.get("type").and_then(Value::as_str);

    if let Some(enum_values) = schema.get("enum").and_then(Value::as_array) {
        if !enum_values.iter().any(|allowed| allowed == value) {
            return Err(CompactError {
                message: format!(
                    "invalid enum value at {path}: {value}"
                ),
            });
        }
    }

    match expected_type {
        Some("object") => {
            let object = value.as_object().ok_or_else(|| CompactError {
                message: format!("expected object at {path}"),
            })?;

            let properties = schema
                .get("properties")
                .and_then(Value::as_object);

            let required = schema
                .get("required")
                .and_then(Value::as_array);

            if let Some(required) = required {
                for required_name in required.iter().filter_map(Value::as_str) {
                    if !object.contains_key(required_name) {
                        return Err(CompactError {
                            message: format!(
                                "missing required field: {path}.{required_name}"
                            ),
                        });
                    }
                }
            }

            if let Some(properties) = properties {
                for (name, actual_value) in object {
                    let property_schema = properties.get(name).ok_or_else(|| {
                        CompactError {
                            message: format!(
                                "unknown field: {path}.{name}"
                            ),
                        }
                    })?;

                    validate_arguments(
                        property_schema,
                        actual_value,
                        &format!("{path}.{name}"),
                    )?;
                }
            }
        }

        Some("string") => {
            if !value.is_string() {
                return Err(CompactError {
                    message: format!("expected string at {path}"),
                });
            }
        }

        Some("integer") => {
            if !value.as_i64().is_some() && !value.as_u64().is_some() {
                return Err(CompactError {
                    message: format!("expected integer at {path}"),
                });
            }
        }

        Some("number") => {
            if !value.is_number() {
                return Err(CompactError {
                    message: format!("expected number at {path}"),
                });
            }
        }

        Some("boolean") => {
            if !value.is_boolean() {
                return Err(CompactError {
                    message: format!("expected boolean at {path}"),
                });
            }
        }

        Some("array") => {
            let array = value.as_array().ok_or_else(|| CompactError {
                message: format!("expected array at {path}"),
            })?;

            if let Some(item_schema) = schema.get("items") {
                for (index, item) in array.iter().enumerate() {
                    validate_arguments(
                        item_schema,
                        item,
                        &format!("{path}[{index}]"),
                    )?;
                }
            }
        }

        _ => {}
    }

    Ok(())
}
pub fn decode_calls(text: &str, tools: &[ToolDef]) -> Result<Vec<ToolCall>> {
    let mut calls = Vec::new();
    let mut remaining = text;

    while let Some(start) = remaining.find("<<call ") {
        let after_start = &remaining[start + 7..];

        let name_end = after_start
            .find(' ')
            .ok_or_else(|| CompactError {
                message: "missing tool name".to_string(),
            })?;

        let name = &after_start[..name_end];

        if !tools.iter().any(|tool| tool.function.name == name) {
            return Err(CompactError {
                message: format!("unknown tool: {name}"),
            });
        }

        let json_start = &after_start[name_end + 1..];
        let end = find_call_end(json_start)?;

        
        let arguments = json_start[..end].trim();

        let parsed: Value = serde_json::from_str(arguments)
            .map_err(|error| CompactError {
                message: format!("invalid JSON arguments: {error}"),
            })?;

        if !parsed.is_object() {
            return Err(CompactError {
                message: "tool arguments must be a JSON object".to_string(),
            });
        }
        let tool = tools.iter().find(|tool| tool.function.name == name)
        .ok_or_else(|| CompactError {
            message: format!("unknown tool: {name}"),
        })?;

if let Some(schema) = &tool.function.parameters {
    validate_arguments(schema, &parsed, "arguments")?;
}

        calls.push(ToolCall {
            id: format!("compact-call-{}", calls.len() + 1),
            kind: "function".to_string(),
            function: FunctionCall {
                name: name.to_string(),
                arguments: arguments.to_string(),
            },
            extra: Map::new(),
        });

        remaining = &json_start[end + 2..];
    }

    Ok(calls)
}
fn find_call_end(text: &str) -> Result<usize> {
    let bytes = text.as_bytes();

    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;

    for i in 0..bytes.len() {
        let byte = bytes[i];

        if in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }

            continue;
        }

        match byte {
            b'"' => in_string = true,

            b'{' | b'[' => {
                depth += 1;
            }

            b'}' | b']' => {
                if depth == 0 {
                    return Err(CompactError {
                        message: "unexpected closing bracket".to_string(),
                    });
                }

                depth -= 1;
            }

            b'>' if i + 1 < bytes.len()
                && bytes[i + 1] == b'>'
                && depth == 0 =>
            {
                return Ok(i);
            }

            _ => {}
        }
    }

    Err(CompactError {
        message: "missing call terminator".to_string(),
    })
}
#[derive(Debug, Default)]
pub struct StreamDecoder {
    buffer: String,
}

impl StreamDecoder {
    pub fn new() -> Self {
        Self {
            buffer: String::new(),
        }
    }

    pub fn push(
        &mut self,
        chunk: &str,
        tools: &[ToolDef],
    ) -> Result<Vec<ToolCall>> {
        self.buffer.push_str(chunk);

        let mut calls = Vec::new();

        loop {
            let Some(start) = self.buffer.find("<<call ") else {
                break;
            };

            let after_start = &self.buffer[start + 7..];

            let Some(name_end) = after_start.find(' ') else {
                break;
            };

            let json_start = &after_start[name_end + 1..];

            let end = match find_call_end(json_start) {
                Ok(end) => end,
                Err(_) => break,
            };

            let complete = format!(
                "{}",
                &self.buffer[start..start + 7 + name_end + 1 + end + 2]
            );

            let mut decoded = decode_calls(&complete, tools)?;

            calls.append(&mut decoded);

            let consumed = start + 7 + name_end + 1 + end + 2;
            self.buffer = self.buffer[consumed..].to_string();
        }

        Ok(calls)
    }

    pub fn finish(&mut self, tools: &[ToolDef]) -> Result<Vec<ToolCall>> {
        let calls = self.push("", tools)?;

        if self.buffer.contains("<<call ") {
            return Err(CompactError {
                message: "incomplete tool call at end of stream".to_string(),
            });
        }

        self.buffer.clear();

        Ok(calls)
    }
}
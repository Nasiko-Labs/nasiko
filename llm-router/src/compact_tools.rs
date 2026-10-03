use std::collections::{HashMap, HashSet};

use serde_json::{Map, Value};
use thiserror::Error;

use crate::ir::{ChatRequest, FunctionCall, Message, ToolCall, ToolDef};

/// Explicit grammar emitted to the model.
///
/// compact-call := "<<call " tool-name " " json-object ">>"
/// tool-name   := ASCII letter (ASCII letter | digit | "_" | "-")*
/// json-object := valid JSON object
pub const CALL_PREFIX: &str = "<<call ";
pub const CALL_SUFFIX: &str = ">>";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactTools(pub String);

#[derive(Debug, Clone)]
pub struct CompactToolSet {
    pub tools: Vec<ToolDef>,
    pub compact: CompactTools,
}

#[derive(Debug, Clone)]
pub struct CompactToolContext {
    pub tools: Vec<ToolDef>,
    pub tool_choice: Option<Value>,
    pub compact_schema: CompactTools,
}

#[derive(Debug, Error)]
pub enum CompactToolsError {
    #[error("tool name is invalid: {0}")]
    InvalidToolName(String),

    #[error("duplicate tool: {0}")]
    DuplicateTool(String),

    #[error("unsupported schema for tool `{tool}`: {reason}")]
    UnsupportedSchema { tool: String, reason: String },

    #[error("invalid compact call syntax")]
    InvalidCallSyntax,

    #[error("unknown tool: {0}")]
    UnknownTool(String),

    #[error("tool `{tool}` arguments must be a JSON object")]
    ArgumentsNotObject { tool: String },

    #[error("invalid JSON arguments: {0}")]
    InvalidJson(String),

    #[error("tool `{tool}` is missing required argument `{field}`")]
    MissingRequired { tool: String, field: String },

    #[error("tool `{tool}` argument `{field}` has invalid type")]
    InvalidType { tool: String, field: String },

    #[error("tool `{tool}` argument `{field}` has invalid enum value")]
    InvalidEnum { tool: String, field: String },

    #[error("tool `{tool}` contains unknown argument `{field}`")]
    UnknownArgument { tool: String, field: String },

    #[error("malformed compact call")]
    MalformedCall,

    #[error("incomplete compact call stream")]
    IncompleteStream,
}

impl CompactTools {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Prepare a request for compact tool calling.
///
/// Native tool definitions are removed from the request and replaced with a
/// provider-independent textual protocol in a system message. The original
/// definitions are retained in CompactToolContext so model output can be
/// decoded back into canonical tool calls.
pub fn prepare_request(
    req: &mut ChatRequest,
) -> Result<Option<CompactToolContext>, CompactToolsError> {
    let Some(tools) = req.tools.as_ref() else {
        return Ok(None);
    };

    if tools.is_empty() {
        return Ok(None);
    }

    // Validate and render everything before mutating the request.
    // This guarantees fail-closed behavior without destroying the native
    // request if schema validation fails.
    let tool_set = encode_tools(tools)?;

    let tool_choice = req.tool_choice.clone();

    validate_tool_choice(tool_choice.as_ref(), tools)?;

    let mut protocol = String::from(
        "You have access to the following tools.\n\
         Use them only when appropriate.\n\
         When calling a tool, emit exactly this grammar:\n\
         <<call TOOL_NAME JSON_OBJECT>>\n\
         Do not use Markdown code fences around tool calls.\n\
         The JSON object must contain only valid arguments for the selected tool.\n",
    );

    protocol.push_str("\nTool choice policy: ");
    protocol.push_str(render_tool_choice(tool_choice.as_ref()));
    protocol.push_str(
        "\nFollow this policy exactly. Do not call a tool that is not permitted by it.\n",
    );

    protocol.push_str("\nAvailable tools:\n");
    protocol.push_str(tool_set.compact.as_str());

    // Only mutate the request after the entire compact representation has
    // been successfully validated and constructed.
    req.tools = None;
    req.tool_choice = None;

    req.messages.insert(
        0,
        Message {
            role: "system".to_string(),
            content: Some(Value::String(protocol)),
            name: None,
            tool_calls: None,
            tool_call_id: None,
            extra: Map::new(),
        },
    );

    Ok(Some(CompactToolContext {
        tools: tool_set.tools,
        tool_choice,
        compact_schema: tool_set.compact,
    }))
}

fn validate_tool_choice(
    choice: Option<&Value>,
    tools: &[ToolDef],
) -> Result<(), CompactToolsError> {
    let Some(choice) = choice else {
        return Ok(());
    };

    match choice {
        Value::String(value) if matches!(value.as_str(), "auto" | "none" | "required") => Ok(()),

        Value::Object(object) => {
            let kind = object
                .get("type")
                .and_then(Value::as_str)
                .ok_or_else(|| CompactToolsError::InvalidCallSyntax)?;

            if kind != "function" {
                return Err(CompactToolsError::InvalidCallSyntax);
            }

            let function = object
                .get("function")
                .and_then(Value::as_object)
                .ok_or_else(|| CompactToolsError::InvalidCallSyntax)?;

            let name = function
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| CompactToolsError::InvalidCallSyntax)?;

            if !tools.iter().any(|tool| tool.function.name == name) {
                return Err(CompactToolsError::UnknownTool(name.to_string()));
            }

            Ok(())
        }

        _ => Err(CompactToolsError::InvalidCallSyntax),
    }
}

fn render_tool_choice(choice: Option<&Value>) -> &'static str {
    match choice {
        None => "auto",
        Some(Value::String(value)) => match value.as_str() {
            "auto" => "auto",
            "none" => "none",
            "required" => "required",
            _ => "invalid",
        },
        Some(Value::Object(_)) => "specific",
        _ => "invalid",
    }
}

pub fn encode_tools(tools: &[ToolDef]) -> Result<CompactToolSet, CompactToolsError> {
    let mut names = HashSet::new();
    let mut lines = Vec::with_capacity(tools.len());

    for tool in tools {
        let function = &tool.function;

        validate_tool_name(&function.name)?;

        if !names.insert(function.name.clone()) {
            return Err(CompactToolsError::DuplicateTool(function.name.clone()));
        }

        let parameters = function.parameters.as_ref().cloned().unwrap_or_else(|| {
            serde_json::json!({
                "type": "object",
                "properties": {}
            })
        });

        validate_schema(&function.name, &parameters)?;

        let signature = render_object_schema(&function.name, &parameters)?;

        let mut line = signature;

        if let Some(description) = &function.description {
            if !description.trim().is_empty() {
                line.push_str(" - ");
                line.push_str(description.trim());
            }
        }

        lines.push(line);
    }

    Ok(CompactToolSet {
        tools: tools.to_vec(),
        compact: CompactTools(lines.join("\n")),
    })
}

fn validate_tool_name(name: &str) -> Result<(), CompactToolsError> {
    let mut chars = name.chars();

    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() => {}
        _ => {
            return Err(CompactToolsError::InvalidToolName(name.to_string()));
        }
    }

    if !chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
        return Err(CompactToolsError::InvalidToolName(name.to_string()));
    }

    Ok(())
}

fn validate_schema(tool: &str, schema: &Value) -> Result<(), CompactToolsError> {
    let Some(obj) = schema.as_object() else {
        return Err(CompactToolsError::UnsupportedSchema {
            tool: tool.to_string(),
            reason: "schema must be an object".to_string(),
        });
    };

    if obj.contains_key("oneOf") {
        return Err(CompactToolsError::UnsupportedSchema {
            tool: tool.to_string(),
            reason: "oneOf is not supported".to_string(),
        });
    }

    if obj.contains_key("anyOf") {
        return Err(CompactToolsError::UnsupportedSchema {
            tool: tool.to_string(),
            reason: "anyOf is not supported".to_string(),
        });
    }

    if obj.contains_key("allOf") {
        return Err(CompactToolsError::UnsupportedSchema {
            tool: tool.to_string(),
            reason: "allOf is not supported".to_string(),
        });
    }

    if obj.contains_key("$ref") {
        return Err(CompactToolsError::UnsupportedSchema {
            tool: tool.to_string(),
            reason: "$ref is not supported".to_string(),
        });
    }

    if let Some(additional) = obj.get("additionalProperties")
        && additional != &Value::Bool(false)
    {
        return Err(CompactToolsError::UnsupportedSchema {
            tool: tool.to_string(),
            reason: "additionalProperties must be false when specified".to_string(),
        });
    }

    if let Some(properties) = obj.get("properties") {
        let Some(properties) = properties.as_object() else {
            return Err(CompactToolsError::UnsupportedSchema {
                tool: tool.to_string(),
                reason: "properties must be an object".to_string(),
            });
        };

        let property_names: HashSet<&str> = properties.keys().map(String::as_str).collect();

        if let Some(required) = obj.get("required") {
            let Some(required) = required.as_array() else {
                return Err(CompactToolsError::UnsupportedSchema {
                    tool: tool.to_string(),
                    reason: "required must be an array".to_string(),
                });
            };

            for field in required {
                let Some(field) = field.as_str() else {
                    return Err(CompactToolsError::UnsupportedSchema {
                        tool: tool.to_string(),
                        reason: "required entries must be strings".to_string(),
                    });
                };

                if !property_names.contains(field) {
                    return Err(CompactToolsError::UnsupportedSchema {
                        tool: tool.to_string(),
                        reason: format!("required field `{field}` is not present in properties"),
                    });
                }
            }
        }

        for (name, child) in properties {
            validate_schema(tool, child).map_err(|err| match err {
                CompactToolsError::UnsupportedSchema { reason, .. } => {
                    CompactToolsError::UnsupportedSchema {
                        tool: tool.to_string(),
                        reason: format!("property `{name}`: {reason}"),
                    }
                }
                other => other,
            })?;
        }
    }

    if let Some(items) = obj.get("items") {
        validate_schema(tool, items).map_err(|err| match err {
            CompactToolsError::UnsupportedSchema { reason, .. } => {
                CompactToolsError::UnsupportedSchema {
                    tool: tool.to_string(),
                    reason: format!("items: {reason}"),
                }
            }
            other => other,
        })?;
    }

    if let Some(enum_values) = obj.get("enum")
        && !enum_values.is_array()
    {
        return Err(CompactToolsError::UnsupportedSchema {
            tool: tool.to_string(),
            reason: "enum must be an array".to_string(),
        });
    }

    Ok(())
}

fn render_object_schema(tool: &str, schema: &Value) -> Result<String, CompactToolsError> {
    let Some(obj) = schema.as_object() else {
        return Err(CompactToolsError::UnsupportedSchema {
            tool: tool.to_string(),
            reason: "schema must be an object".to_string(),
        });
    };

    let properties = obj
        .get("properties")
        .and_then(Value::as_object)
        .ok_or_else(|| CompactToolsError::UnsupportedSchema {
            tool: tool.to_string(),
            reason: "tool parameters must define object properties".to_string(),
        })?;

    let required: HashSet<&str> = obj
        .get("required")
        .and_then(Value::as_array)
        .map(|items| items.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();

    let mut fields = Vec::with_capacity(properties.len());

    for (name, schema) in properties {
        let rendered = render_schema_type(tool, schema)?;

        let marker = if required.contains(name.as_str()) {
            "!"
        } else {
            "?"
        };

        fields.push(format!("{name}{marker}:{rendered}"));
    }

    Ok(format!("{tool}({})", fields.join(", ")))
}

fn render_schema_type(tool: &str, schema: &Value) -> Result<String, CompactToolsError> {
    let Some(obj) = schema.as_object() else {
        return Err(CompactToolsError::UnsupportedSchema {
            tool: tool.to_string(),
            reason: "property schema must be an object".to_string(),
        });
    };

    if let Some(constant) = obj.get("const") {
        return render_scalar_value(constant).ok_or_else(|| CompactToolsError::UnsupportedSchema {
            tool: tool.to_string(),
            reason: "unsupported const value".to_string(),
        });
    }

    if let Some(values) = obj.get("enum") {
        let values = values
            .as_array()
            .ok_or_else(|| CompactToolsError::UnsupportedSchema {
                tool: tool.to_string(),
                reason: "enum must be an array".to_string(),
            })?;

        let rendered: Vec<String> = values.iter().filter_map(render_scalar_value).collect();

        if rendered.len() != values.len() || rendered.is_empty() {
            return Err(CompactToolsError::UnsupportedSchema {
                tool: tool.to_string(),
                reason: "only scalar enum values are supported".to_string(),
            });
        }

        return Ok(rendered.join("|"));
    }

    match obj.get("type").and_then(Value::as_str) {
        Some("string") => Ok("str".to_string()),
        Some("integer") => Ok("int".to_string()),
        Some("number") => Ok("num".to_string()),
        Some("boolean") => Ok("bool".to_string()),
        Some("null") => Ok("null".to_string()),

        Some("array") => {
            let items = obj
                .get("items")
                .ok_or_else(|| CompactToolsError::UnsupportedSchema {
                    tool: tool.to_string(),
                    reason: "array requires items".to_string(),
                })?;

            Ok(format!("[{}]", render_schema_type(tool, items)?))
        }

        Some("object") => {
            let properties = obj
                .get("properties")
                .and_then(Value::as_object)
                .ok_or_else(|| CompactToolsError::UnsupportedSchema {
                    tool: tool.to_string(),
                    reason: "nested object requires properties".to_string(),
                })?;

            let required: HashSet<&str> = obj
                .get("required")
                .and_then(Value::as_array)
                .map(|items| items.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();

            let mut fields = Vec::with_capacity(properties.len());

            for (name, child) in properties {
                let marker = if required.contains(name.as_str()) {
                    "!"
                } else {
                    "?"
                };

                fields.push(format!(
                    "{name}{marker}:{}",
                    render_schema_type(tool, child)?
                ));
            }

            Ok(format!("{{{}}}", fields.join(", ")))
        }

        Some(other) => Err(CompactToolsError::UnsupportedSchema {
            tool: tool.to_string(),
            reason: format!("unsupported JSON Schema type `{other}`"),
        }),

        None => Err(CompactToolsError::UnsupportedSchema {
            tool: tool.to_string(),
            reason: "schema type is required".to_string(),
        }),
    }
}

fn render_scalar_value(value: &Value) -> Option<String> {
    match value {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        Value::Null => Some("null".to_string()),
        Value::Array(_) | Value::Object(_) => None,
    }
}

/// Decode every compact call in a completed model response.
///
/// Text surrounding calls is returned separately so the caller can preserve it
/// as ordinary assistant content.
pub fn decode_calls(
    text: &str,
    tools: &[ToolDef],
) -> Result<(String, Vec<ToolCall>), CompactToolsError> {
    let mut tool_map = HashMap::new();

    for tool in tools {
        tool_map.insert(tool.function.name.as_str(), tool);
    }

    let mut remaining = text.to_string();
    let mut calls = Vec::new();
    let mut search_from = 0usize;
    let mut call_index = 0usize;

    while let Some(relative_start) = remaining[search_from..].find(CALL_PREFIX) {
        let start = search_from + relative_start;

        let args_start = start + CALL_PREFIX.len();

        let Some(relative_end) = find_json_call_end(&remaining[args_start..], CALL_SUFFIX) else {
            return Err(CompactToolsError::MalformedCall);
        };

        let end = args_start + relative_end;

        let body = &remaining[args_start..end];

        let (name, json_text) = split_call_body(body)?;

        let tool = tool_map
            .get(name)
            .ok_or_else(|| CompactToolsError::UnknownTool(name.to_string()))?;

        let arguments: Value = serde_json::from_str(json_text)
            .map_err(|e| CompactToolsError::InvalidJson(e.to_string()))?;

        validate_arguments(name, tool.function.parameters.as_ref(), &arguments)?;

        calls.push(ToolCall {
            id: format!("call_compact_{call_index}"),
            kind: "function".to_string(),
            function: FunctionCall {
                name: name.to_string(),
                arguments: serde_json::to_string(&arguments)
                    .map_err(|e| CompactToolsError::InvalidJson(e.to_string()))?,
            },
            extra: Map::new(),
        });

        remaining.replace_range(start..end + CALL_SUFFIX.len(), "");
        search_from = start;
        call_index += 1;
    }

    Ok((remaining, calls))
}

fn split_call_body(body: &str) -> Result<(&str, &str), CompactToolsError> {
    let body = body.trim();

    let split = body
        .find(char::is_whitespace)
        .ok_or(CompactToolsError::InvalidCallSyntax)?;

    let name = &body[..split];
    let arguments = body[split..].trim();

    if arguments.is_empty() {
        return Err(CompactToolsError::InvalidCallSyntax);
    }

    validate_tool_name(name)?;

    Ok((name, arguments))
}

fn validate_arguments(
    tool_name: &str,
    schema: Option<&Value>,
    arguments: &Value,
) -> Result<(), CompactToolsError> {
    let Some(schema) = schema else {
        if !arguments.is_object() {
            return Err(CompactToolsError::ArgumentsNotObject {
                tool: tool_name.to_string(),
            });
        }
        return Ok(());
    };

    validate_value(tool_name, "", schema, arguments)
}

fn validate_value(
    tool_name: &str,
    field: &str,
    schema: &Value,
    value: &Value,
) -> Result<(), CompactToolsError> {
    let obj = schema
        .as_object()
        .ok_or_else(|| CompactToolsError::UnsupportedSchema {
            tool: tool_name.to_string(),
            reason: "schema must be an object".to_string(),
        })?;

    if let Some(constant) = obj.get("const")
        && value != constant
    {
        return Err(CompactToolsError::InvalidEnum {
            tool: tool_name.to_string(),
            field: field.to_string(),
        });
    }

    if let Some(values) = obj.get("enum").and_then(Value::as_array)
        && !values.iter().any(|candidate| candidate == value)
    {
        return Err(CompactToolsError::InvalidEnum {
            tool: tool_name.to_string(),
            field: field.to_string(),
        });
    }

    if let Some(expected) = obj.get("type").and_then(Value::as_str) {
        let valid = match expected {
            "string" => value.is_string(),
            "integer" => value.as_i64().is_some(),
            "number" => value.is_number(),
            "boolean" => value.is_boolean(),
            "null" => value.is_null(),
            "array" => value.is_array(),
            "object" => value.is_object(),
            _ => false,
        };

        if !valid {
            return Err(CompactToolsError::InvalidType {
                tool: tool_name.to_string(),
                field: field.to_string(),
            });
        }
    }

    if let (Some(properties), Some(object)) = (
        obj.get("properties").and_then(Value::as_object),
        value.as_object(),
    ) {
        let required: HashSet<&str> = obj
            .get("required")
            .and_then(Value::as_array)
            .map(|items| items.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();

        for required_field in required {
            if !object.contains_key(required_field) {
                return Err(CompactToolsError::MissingRequired {
                    tool: tool_name.to_string(),
                    field: required_field.to_string(),
                });
            }
        }

        let additional_properties_allowed = obj
            .get("additionalProperties")
            .and_then(Value::as_bool)
            .unwrap_or(true);

        for key in object.keys() {
            let Some(property_schema) = properties.get(key) else {
                if !additional_properties_allowed {
                    return Err(CompactToolsError::UnknownArgument {
                        tool: tool_name.to_string(),
                        field: key.clone(),
                    });
                }
                continue;
            };

            let nested_field = if field.is_empty() {
                key.clone()
            } else {
                format!("{field}.{key}")
            };

            validate_value(tool_name, &nested_field, property_schema, &object[key])?;
        }
    }

    if let (Some(items_schema), Some(array)) = (obj.get("items"), value.as_array()) {
        for (index, item) in array.iter().enumerate() {
            let item_field = format!("{field}[{index}]");
            validate_value(tool_name, &item_field, items_schema, item)?;
        }
    }

    Ok(())
}

/// Incrementally extracts compact calls from arbitrary stream chunks.
///
/// The decoder intentionally emits only complete calls. Ordinary text is returned
/// as text fragments and can be forwarded as normal assistant content.
#[derive(Debug, Default)]
pub struct StreamDecoder {
    buffer: String,
    emitted_text: usize,
    next_call_index: usize,
    finished: bool,
}

#[derive(Debug, Clone)]
pub enum StreamEvent {
    Text(String),
    ToolCall(ToolCall),
}

impl StreamDecoder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(
        &mut self,
        chunk: &str,
        tools: &[ToolDef],
    ) -> Result<Vec<StreamEvent>, CompactToolsError> {
        if self.finished {
            return Err(CompactToolsError::IncompleteStream);
        }

        self.buffer.push_str(chunk);

        let mut events = Vec::new();

        loop {
            let Some(start) = self.buffer.find(CALL_PREFIX) else {
                // Keep enough suffix to detect a marker split across chunks.
                let keep = longest_marker_suffix(&self.buffer, CALL_PREFIX);

                let emit_len = self.buffer.len().saturating_sub(keep);

                if emit_len > self.emitted_text {
                    let text = self.buffer[self.emitted_text..emit_len].to_string();
                    if !text.is_empty() {
                        events.push(StreamEvent::Text(text));
                    }
                }

                self.emitted_text = emit_len;
                break;
            };

            if start > self.emitted_text {
                let text = self.buffer[self.emitted_text..start].to_string();
                if !text.is_empty() {
                    events.push(StreamEvent::Text(text));
                }

                // The text before an incomplete call has already been emitted.
                // Remember that position so it is never emitted twice when
                // the remaining JSON arrives in a later stream chunk.
                self.emitted_text = start;
            }

            let args_start = start + CALL_PREFIX.len();

            let Some(relative_end) = find_json_call_end(&self.buffer[args_start..], CALL_SUFFIX)
            else {
                break;
            };

            let end = args_start + relative_end;
            let body = &self.buffer[args_start..end];

            let (name, json_text) = split_call_body(body)?;

            let tool = tools
                .iter()
                .find(|tool| tool.function.name == name)
                .ok_or_else(|| CompactToolsError::UnknownTool(name.to_string()))?;

            let arguments: Value = serde_json::from_str(json_text)
                .map_err(|e| CompactToolsError::InvalidJson(e.to_string()))?;

            validate_arguments(name, tool.function.parameters.as_ref(), &arguments)?;

            events.push(StreamEvent::ToolCall(ToolCall {
                id: format!("call_compact_{}", self.next_call_index),
                kind: "function".to_string(),
                function: FunctionCall {
                    name: name.to_string(),
                    arguments: serde_json::to_string(&arguments)
                        .map_err(|e| CompactToolsError::InvalidJson(e.to_string()))?,
                },
                extra: Map::new(),
            }));

            self.next_call_index += 1;

            self.buffer.drain(..end + CALL_SUFFIX.len());
            self.emitted_text = 0;
        }

        Ok(events)
    }

    pub fn finish(&mut self) -> Result<Vec<StreamEvent>, CompactToolsError> {
        self.finished = true;

        if contains_partial_marker(&self.buffer) {
            return Err(CompactToolsError::IncompleteStream);
        }

        let mut events = Vec::new();

        if self.emitted_text < self.buffer.len() {
            let text = self.buffer[self.emitted_text..].to_string();
            if !text.is_empty() {
                events.push(StreamEvent::Text(text));
            }
        }

        self.buffer.clear();
        self.emitted_text = 0;

        Ok(events)
    }
}

fn find_json_call_end(input: &str, suffix: &str) -> Option<usize> {
    let bytes = input.as_bytes();

    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;

    for i in 0..bytes.len() {
        let c = bytes[i];

        if in_string {
            if escaped {
                escaped = false;
            } else if c == b'\\' {
                escaped = true;
            } else if c == b'"' {
                in_string = false;
            }
            continue;
        }

        match c {
            b'"' => in_string = true,
            b'{' => depth += 1,
            b'}' => {
                if depth == 0 {
                    return None;
                }

                depth -= 1;

                if depth == 0 {
                    let rest = &input[i + 1..];
                    if rest.starts_with(suffix) {
                        return Some(i + 1);
                    }
                }
            }
            _ => {}
        }
    }

    None
}

fn longest_marker_suffix(input: &str, marker: &str) -> usize {
    let max = input.len().min(marker.len().saturating_sub(1));

    for len in (1..=max).rev() {
        if input.ends_with(&marker[..len]) {
            return len;
        }
    }

    0
}

fn contains_partial_marker(input: &str) -> bool {
    longest_marker_suffix(input, CALL_PREFIX) > 0 || input.contains(CALL_PREFIX)
}

#[cfg(test)]
mod tests {
    use crate::ir::FunctionDef;
    #[test]
    fn prepare_request_removes_native_tools_and_injects_protocol() {
        let tool = ToolDef {
            kind: "function".to_string(),
            function: FunctionDef {
                name: "get_weather".to_string(),
                description: Some("Get weather for a city".to_string()),
                parameters: Some(json!({
                    "type": "object",
                    "properties": {
                        "city": {
                            "type": "string"
                        }
                    },
                    "required": ["city"]
                })),
            },
            extra: Map::new(),
        };

        let mut req = ChatRequest {
            model: Some("test-model".to_string()),
            messages: vec![Message {
                role: "user".to_string(),
                content: Some(Value::String("What is the weather?".to_string())),
                name: None,
                tool_calls: None,
                tool_call_id: None,
                extra: Map::new(),
            }],
            tools: Some(vec![tool]),
            tool_choice: None,
            temperature: None,
            max_tokens: None,
            stream: None,
            extra: Map::new(),
        };

        let context = prepare_request(&mut req)
            .expect("prepare_request should succeed")
            .expect("tool context should exist");

        assert!(req.tools.is_none());
        assert!(req.tool_choice.is_none());

        assert_eq!(context.tools.len(), 1);
        assert_eq!(context.tools[0].function.name, "get_weather");

        let system = req
            .messages
            .first()
            .and_then(|m| m.content.as_ref())
            .and_then(Value::as_str)
            .expect("compact protocol system message");

        assert!(system.contains("<<call TOOL_NAME JSON_OBJECT>>"));
        assert!(system.contains("get_weather"));
        assert!(system.contains("city"));
    }

    #[test]
    fn prepare_request_preserves_tool_choice_in_context() {
        let tool = ToolDef {
            kind: "function".to_string(),
            function: FunctionDef {
                name: "get_weather".to_string(),
                description: None,
                parameters: Some(json!({
                    "type": "object",
                    "properties": {}
                })),
            },
            extra: Map::new(),
        };

        let tool_choice = json!({
            "type": "function",
            "function": {
                "name": "get_weather"
            }
        });

        let mut req = ChatRequest {
            model: None,
            messages: vec![],
            tools: Some(vec![tool]),
            tool_choice: Some(tool_choice.clone()),
            temperature: None,
            max_tokens: None,
            stream: None,
            extra: Map::new(),
        };

        let context = prepare_request(&mut req)
            .expect("prepare_request should succeed")
            .expect("tool context should exist");

        assert!(req.tools.is_none());
        assert!(req.tool_choice.is_none());
        assert_eq!(context.tool_choice, Some(tool_choice));
    }

    #[test]
    fn prepare_request_invalid_schema_leaves_request_unchanged() {
        let tool = ToolDef {
            kind: "function".to_string(),
            function: FunctionDef {
                name: "broken_tool".to_string(),
                description: None,
                parameters: Some(json!({
                    "type": "object",
                    "properties": {
                        "value": {
                            "oneOf": [
                                {"type": "string"},
                                {"type": "number"}
                            ]
                        }
                    }
                })),
            },
            extra: Map::new(),
        };

        let mut req = ChatRequest {
            model: None,
            messages: vec![],
            tools: Some(vec![tool]),
            tool_choice: None,
            temperature: None,
            max_tokens: None,
            stream: None,
            extra: Map::new(),
        };

        let result = prepare_request(&mut req);

        assert!(matches!(
            result,
            Err(CompactToolsError::UnsupportedSchema { .. })
        ));

        assert!(req.tools.is_some());
        assert!(req.messages.is_empty());
    }

    #[test]
    fn prepare_request_without_tools_is_noop() {
        let mut req = ChatRequest {
            model: None,
            messages: vec![],
            tools: None,
            tool_choice: None,
            temperature: None,
            max_tokens: None,
            stream: None,
            extra: Map::new(),
        };

        let result = prepare_request(&mut req).expect("request without tools should succeed");

        assert!(result.is_none());
        assert!(req.tools.is_none());
        assert!(req.messages.is_empty());
    }

    use super::*;
    use serde_json::json;

    fn tools() -> Vec<ToolDef> {
        vec![ToolDef {
            kind: "function".into(),
            function: FunctionDef {
                name: "create_event".into(),
                description: Some("Create an event".into()),
                parameters: Some(json!({
                    "type": "object",
                    "properties": {
                        "title": { "type": "string" },
                        "duration_min": {
                            "type": "integer",
                            "enum": [15, 30, 60]
                        },
                        "visibility": {
                            "type": "string",
                            "enum": ["public", "private"]
                        }
                    },
                    "required": ["title"]
                })),
            },
            extra: Map::new(),
        }]
    }

    #[test]
    fn compact_schema_preserves_required_optional_and_enum() {
        let result = encode_tools(&tools()).unwrap();

        let schema = result.compact.as_str();

        assert!(schema.starts_with("create_event("));
        assert!(schema.contains("title!:str"));
        assert!(schema.contains("duration_min?:15|30|60"));
        assert!(schema.contains("visibility?:public|private"));
        assert!(schema.ends_with(") - Create an event"));
    }

    #[test]
    fn multiple_calls_and_surrounding_text() {
        let input = r#"Before <<call create_event {"title":"A","duration_min":30}>> after <<call create_event {"title":"B"}>> done"#;

        let (text, calls) = decode_calls(input, &tools()).unwrap();

        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].function.name, "create_event");
        assert_eq!(calls[1].function.name, "create_event");
        assert_eq!(text, "Before  after  done");
    }

    #[test]
    fn unknown_tool_fails_closed() {
        let err = decode_calls(r#"<<call delete_everything {}>>"#, &tools()).unwrap_err();

        assert!(matches!(err, CompactToolsError::UnknownTool(_)));
    }

    #[test]
    fn missing_required_fails() {
        let err =
            decode_calls(r#"<<call create_event {"duration_min":30}>>"#, &tools()).unwrap_err();

        assert!(matches!(err, CompactToolsError::MissingRequired { .. }));
    }

    #[test]
    fn enum_violation_fails() {
        let err = decode_calls(
            r#"<<call create_event {"title":"A","duration_min":45}>>"#,
            &tools(),
        )
        .unwrap_err();

        assert!(matches!(err, CompactToolsError::InvalidEnum { .. }));
    }

    #[test]
    fn nested_schema_is_supported() {
        let nested = ToolDef {
            kind: "function".into(),
            function: FunctionDef {
                name: "nested".into(),
                description: None,
                parameters: Some(json!({
                    "type": "object",
                    "properties": {
                        "config": {
                            "type": "object",
                            "properties": {
                                "enabled": { "type": "boolean" },
                                "tags": {
                                    "type": "array",
                                    "items": { "type": "string" }
                                }
                            },
                            "required": ["enabled"]
                        }
                    },
                    "required": ["config"]
                })),
            },
            extra: Map::new(),
        };

        let result = encode_tools(&[nested]).unwrap();

        assert_eq!(
            result.compact.as_str(),
            "nested(config!:{enabled!:bool, tags?:[str]})"
        );
    }

    #[test]
    fn delimiter_inside_json_string_is_safe() {
        let input = r#"<<call create_event {"title":"contains >> inside"}>>"#;

        let (_, calls) = decode_calls(input, &tools()).unwrap();

        assert_eq!(calls.len(), 1);
    }

    #[test]
    fn unsupported_one_of_fails_closed() {
        let tool = ToolDef {
            kind: "function".into(),
            function: FunctionDef {
                name: "test".into(),
                description: None,
                parameters: Some(json!({
                    "type": "object",
                    "properties": {
                        "value": {
                            "oneOf": [
                                {"type": "string"},
                                {"type": "integer"}
                            ]
                        }
                    }
                })),
            },
            extra: Map::new(),
        };

        let err = encode_tools(&[tool]).unwrap_err();

        assert!(matches!(err, CompactToolsError::UnsupportedSchema { .. }));
    }

    #[test]
    fn stream_handles_split_marker_and_json() {
        let mut decoder = StreamDecoder::new();
        let schema = tools();

        let mut events = Vec::new();

        for chunk in [
            "hello <<c",
            "all create_event {\"title\":\"A\",",
            "\"duration_min\":30}>> world",
        ] {
            events.extend(decoder.push(chunk, &schema).unwrap());
        }

        events.extend(decoder.finish().unwrap());

        assert!(matches!(&events[0], StreamEvent::Text(t) if t == "hello "));
        assert!(matches!(&events[1], StreamEvent::ToolCall(call)
            if call.function.name == "create_event"));
        assert!(matches!(&events[2], StreamEvent::Text(t) if t == " world"));
    }

    #[test]
    fn malformed_json_fails_closed() {
        let err = decode_calls(r#"<<call create_event {"title":"A",}>>"#, &tools()).unwrap_err();

        assert!(matches!(err, CompactToolsError::InvalidJson(_)));
    }

    #[test]
    fn wrong_argument_type_fails_closed() {
        let err = decode_calls(r#"<<call create_event {"title":123}>>"#, &tools()).unwrap_err();

        assert!(matches!(err, CompactToolsError::InvalidType { .. }));
    }

    #[test]
    fn incomplete_stream_fails_closed() {
        let mut decoder = StreamDecoder::new();

        let events = decoder
            .push("before <<call create_event {\"title\":\"A\"", &tools())
            .unwrap();

        assert!(matches!(&events[0], StreamEvent::Text(t) if t == "before "));

        let err = decoder.finish().unwrap_err();

        assert!(matches!(err, CompactToolsError::IncompleteStream));
    }

    #[test]
    fn text_only_stream_is_preserved() {
        let mut decoder = StreamDecoder::new();

        let mut events = Vec::new();

        for chunk in ["hello ", "world", "!"] {
            events.extend(decoder.push(chunk, &tools()).unwrap());
        }

        events.extend(decoder.finish().unwrap());

        assert_eq!(events.len(), 3);
        assert!(matches!(&events[0], StreamEvent::Text(t) if t == "hello "));
        assert!(matches!(&events[1], StreamEvent::Text(t) if t == "world"));
        assert!(matches!(&events[2], StreamEvent::Text(t) if t == "!"));
    }

    #[test]
    fn multiple_streamed_calls_get_sequential_ids() {
        let mut decoder = StreamDecoder::new();

        let mut events = Vec::new();

        for chunk in [
            "<<call create_event {\"title\":\"A\"}>> and ",
            "<<call create_event {\"title\":\"B\",\"duration_min\":60}>>",
        ] {
            events.extend(decoder.push(chunk, &tools()).unwrap());
        }

        events.extend(decoder.finish().unwrap());

        let calls: Vec<_> = events
            .iter()
            .filter_map(|event| match event {
                StreamEvent::ToolCall(call) => Some(call),
                _ => None,
            })
            .collect();

        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].id, "call_compact_0");
        assert_eq!(calls[1].id, "call_compact_1");
    }

    #[test]
    fn streaming_delimiter_inside_json_string_is_safe() {
        let mut decoder = StreamDecoder::new();

        let mut events = Vec::new();

        for chunk in [
            "<<call create_event {\"title\":\"contains ",
            ">> inside\"}>>",
        ] {
            events.extend(decoder.push(chunk, &tools()).unwrap());
        }

        events.extend(decoder.finish().unwrap());

        assert_eq!(events.len(), 1);

        match &events[0] {
            StreamEvent::ToolCall(call) => {
                assert_eq!(call.function.name, "create_event");
                assert_eq!(call.function.arguments, r#"{"title":"contains >> inside"}"#);
            }
            other => panic!("expected tool call, got {other:?}"),
        }
    }

    #[test]
    fn marker_can_be_split_at_every_boundary() {
        let input = r#"hello <<call create_event {"title":"A","duration_min":30}>> world"#;

        for boundary in 1..input.len() {
            let mut decoder = StreamDecoder::new();

            let first = &input[..boundary];
            let second = &input[boundary..];

            let mut events = Vec::new();
            events.extend(decoder.push(first, &tools()).unwrap());
            events.extend(decoder.push(second, &tools()).unwrap());
            events.extend(decoder.finish().unwrap());

            assert!(
                events.iter().any(|event| matches!(
                    event,
                    StreamEvent::ToolCall(call)
                        if call.function.name == "create_event"
                )),
                "tool call was lost at boundary {boundary}"
            );

            let text: String = events
                .iter()
                .filter_map(|event| match event {
                    StreamEvent::Text(value) => Some(value.as_str()),
                    StreamEvent::ToolCall(_) => None,
                })
                .collect();

            assert_eq!(text, "hello  world", "text changed at boundary {boundary}");
        }
    }

    #[test]
    fn empty_stream_chunks_do_not_panic() {
        let mut decoder = StreamDecoder::new();

        assert!(decoder.push("", &tools()).unwrap().is_empty());
        assert!(decoder.push("", &tools()).unwrap().is_empty());

        let events = decoder.finish().unwrap();

        assert!(events.is_empty());
    }
}

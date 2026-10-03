use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use thiserror::Error;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolDef {
    pub name: String,
    pub description: Option<String>,
    pub parameters: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolCall {
    pub name: String,
    pub arguments: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CompactTools {
    pub prompt_instructions: String,
    pub compacted_definitions: String,
}

#[derive(Debug, Error, PartialEq)]
pub enum CompactError {
    #[error("Unknown tool: {0}")]
    UnknownTool(String),
    #[error("Invalid arguments: {0}")]
    InvalidArguments(String),
    #[error("Parse error: {0}")]
    ParseError(String),
}

fn schema_type(prop: &Value) -> String {
    let ty = prop
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("any")
        .to_string();

    match ty.as_str() {
        "string" => {
            if let Some(format) = prop.get("format").and_then(Value::as_str) {
                if format == "date-time" {
                    return "datetime".to_string();
                }
            }
            if let Some(enum_vals) = prop.get("enum").and_then(Value::as_array) {
                let vals: Vec<String> = enum_vals
                    .iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect();
                if !vals.is_empty() {
                    return vals.join("|");
                }
            }
            "str".to_string()
        }
        "integer" | "number" => "int".to_string(),
        "boolean" => "bool".to_string(),
        "array" => {
            let item_type = prop
                .get("items")
                .map(schema_type)
                .unwrap_or_else(|| "str".to_string());
            format!("[{}]", item_type)
        }
        _ => "any".to_string(),
    }
}

pub fn encode_tools(tools: &[ToolDef]) -> Result<CompactTools, CompactError> {
    let mut definitions = Vec::new();

    for tool in tools {
        let mut params: Vec<String> = Vec::new();
        let empty_val = Value::Object(Map::new());

        let parameters = tool.parameters.as_ref().unwrap_or(&empty_val);
        let properties = parameters
            .get("properties")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        let required: Vec<String> = parameters
            .get("required")
            .and_then(Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();

        for (field_name, schema) in properties.iter() {
            let ty = schema_type(schema);
            if required.contains(field_name) {
                params.push(format!("{}:{}", field_name, ty));
            } else {
                params.push(format!("{}?:{}", field_name, ty));
            }
        }

        let description = tool.description.clone().unwrap_or_default();
        definitions.push(format!(
            "{}({}) - {}",
            tool.name,
            params.join(", "),
            description
        ));
    }

    Ok(CompactTools {
        prompt_instructions: "To call a tool, emit: <<call name {json args}>>".to_string(),
        compacted_definitions: definitions.join("\n"),
    })
}

pub fn decode_calls(text: &str, tools: &[ToolDef]) -> Result<Vec<ToolCall>, CompactError> {
    let re = Regex::new(r"(?s)<<call\s+([a-zA-Z0-9_\-]+)\s+(\{.*?\})>>")
        .map_err(|e| CompactError::ParseError(e.to_string()))?;

    let mut calls = Vec::new();

    for cap in re.captures_iter(text) {
        let name = cap
            .get(1)
            .map(|m| m.as_str())
            .ok_or_else(|| CompactError::ParseError("Missing tool name".to_string()))?;
        let json_str = cap
            .get(2)
            .map(|m| m.as_str())
            .ok_or_else(|| CompactError::ParseError("Missing arguments".to_string()))?;

        let tool = tools
            .iter()
            .find(|t| t.name == name)
            .ok_or_else(|| CompactError::UnknownTool(name.to_string()))?;

        let args: Value =
            serde_json::from_str(json_str).map_err(|e| CompactError::ParseError(e.to_string()))?;

        let empty_val = Value::Object(Map::new());
        let parameters = tool.parameters.as_ref().unwrap_or(&empty_val);

        let required: Vec<String> = parameters
            .get("required")
            .and_then(Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();

        for field in required.iter() {
            if args.get(field).is_none() {
                return Err(CompactError::InvalidArguments(format!(
                    "Missing required field: {}",
                    field
                )));
            }
        }

        if let Some(properties) = parameters.get("properties").and_then(Value::as_object) {
            for (arg_name, arg_value) in args.as_object().unwrap_or(&Map::new()) {
                if let Some(prop) = properties.get(arg_name) {
                    if let Some(enum_vals) = prop.get("enum").and_then(Value::as_array) {
                        let allowed: Vec<&str> =
                            enum_vals.iter().filter_map(Value::as_str).collect();
                        let val_str = match arg_value {
                            Value::String(s) => s.clone(),
                            other => other.to_string(),
                        };
                        if !allowed.contains(&val_str.as_str()) {
                            return Err(CompactError::InvalidArguments(format!(
                                "Invalid enum value: {}",
                                val_str
                            )));
                        }
                    }
                }
            }
        }

        calls.push(ToolCall {
            name: name.to_string(),
            arguments: args,
        });
    }

    Ok(calls)
}

pub fn decode_tools(compact: &CompactTools) -> Result<Vec<ToolDef>, CompactError> {
    let mut tools = Vec::new();

    for line in compact.compacted_definitions.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let open = line.find('(').ok_or_else(|| {
            CompactError::ParseError(format!("Invalid definition line: {}", line))
        })?;
        let close = line.find(')').ok_or_else(|| {
            CompactError::ParseError(format!("Invalid definition line: {}", line))
        })?;

        let name = line[..open].trim().to_string();
        let params_str = &line[(open + 1)..close];
        let description_raw = line[(close + 1)..].trim();
        let description = description_raw
            .strip_prefix("- ")
            .map(str::to_string)
            .unwrap_or_else(|| description_raw.to_string());

        let mut properties = Map::new();
        let mut required = Vec::new();

        for param in params_str.split(',') {
            let param = param.trim();
            if param.is_empty() {
                continue;
            }

            let (field, required_flag) = if let Some(f) = param.strip_suffix('?') {
                (f, false)
            } else {
                (param, true)
            };
            let (field_name, ty) = match field.split_once(':') {
                Some((n, t)) => (n.trim(), t.trim()),
                None => (field, "any"),
            };

            let mut schema = Map::new();
            match ty {
                "str" => {
                    schema.insert("type".to_string(), Value::String("string".to_string()));
                }
                "datetime" => {
                    schema.insert("type".to_string(), Value::String("string".to_string()));
                    schema.insert("format".to_string(), Value::String("date-time".to_string()));
                }
                "int" => {
                    schema.insert("type".to_string(), Value::String("integer".to_string()));
                }
                "bool" => {
                    schema.insert("type".to_string(), Value::String("boolean".to_string()));
                }
                "any" => {}
                t if t.starts_with('[') && t.ends_with(']') => {
                    schema.insert("type".to_string(), Value::String("array".to_string()));
                    let item = &t[1..t.len() - 1];
                    let mut item_schema = Map::new();
                    match item {
                        "str" => {
                            item_schema
                                .insert("type".to_string(), Value::String("string".to_string()));
                        }
                        "int" => {
                            item_schema
                                .insert("type".to_string(), Value::String("integer".to_string()));
                        }
                        "bool" => {
                            item_schema
                                .insert("type".to_string(), Value::String("boolean".to_string()));
                        }
                        "datetime" => {
                            item_schema
                                .insert("type".to_string(), Value::String("string".to_string()));
                            item_schema.insert(
                                "format".to_string(),
                                Value::String("date-time".to_string()),
                            );
                        }
                        _ => {}
                    }
                    schema.insert("items".to_string(), Value::Object(item_schema));
                }
                t if t.contains('|') => {
                    schema.insert("type".to_string(), Value::String("string".to_string()));
                    let enum_vals: Vec<Value> = t
                        .split('|')
                        .map(|v| Value::String(v.trim().to_string()))
                        .collect();
                    schema.insert("enum".to_string(), Value::Array(enum_vals));
                }
                _ => {}
            }

            properties.insert(field_name.to_string(), Value::Object(schema));
            if required_flag {
                required.push(Value::String(field_name.to_string()));
            }
        }

        let mut parameters = Map::new();
        parameters.insert("properties".to_string(), Value::Object(properties));
        if !required.is_empty() {
            parameters.insert("required".to_string(), Value::Array(required));
        }

        tools.push(ToolDef {
            name,
            description: Some(description),
            parameters: Some(Value::Object(parameters)),
        });
    }

    Ok(tools)
}

pub struct StreamDecoder {
    buffer: String,
    tools: Vec<ToolDef>,
    emitted: usize,
}

impl StreamDecoder {
    pub fn new(tools: Vec<ToolDef>) -> Self {
        Self {
            buffer: String::new(),
            tools,
            emitted: 0,
        }
    }

    pub fn feed(&mut self, chunk: &str) -> Result<Vec<ToolCall>, CompactError> {
        self.buffer.push_str(chunk);
        let calls = decode_calls(&self.buffer, &self.tools)?;
        let new_calls = calls[self.emitted.min(calls.len())..].to_vec();
        self.emitted = calls.len();
        Ok(new_calls)
    }

    pub fn finish(mut self) -> Result<Vec<ToolCall>, CompactError> {
        let calls = decode_calls(&self.buffer, &self.tools)?;
        let remaining = calls[self.emitted.min(calls.len())..].to_vec();
        self.emitted = calls.len();
        Ok(remaining)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_encode_and_decode_tools() {
        let tools = vec![ToolDef {
            name: "test_tool".to_string(),
            description: Some("A test tool".to_string()),
            parameters: Some(json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string" },
                    "limit": { "type": "integer" }
                },
                "required": ["query"]
            })),
        }];

        let compact = encode_tools(&tools).unwrap();
        assert!(compact.compacted_definitions.contains("test_tool("));
        assert!(compact.compacted_definitions.contains("query:str"));
        assert!(compact.compacted_definitions.contains("limit?:int"));

        let decoded = decode_tools(&compact).unwrap();
        assert_eq!(decoded.len(), 1);
        assert_eq!(decoded[0].name, "test_tool");
    }

    #[test]
    fn test_decode_calls() {
        let tools = vec![ToolDef {
            name: "search".to_string(),
            description: None,
            parameters: Some(json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string" }
                },
                "required": ["query"]
            })),
        }];

        let text = "Result: <<call search {\"query\":\"rust\"}>> done";
        let calls = decode_calls(text, &tools).unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "search");
        assert_eq!(calls[0].arguments["query"], "rust");
    }

    #[test]
    fn test_stream_decoder() {
        let tools = vec![ToolDef {
            name: "ping".to_string(),
            description: None,
            parameters: Some(json!({
                "type": "object",
                "properties": {}
            })),
        }];

        let mut decoder = StreamDecoder::new(tools);
        assert_eq!(decoder.feed("<<call").unwrap().len(), 0);
        assert_eq!(decoder.feed(" ping {}>>").unwrap().len(), 1);
        assert_eq!(decoder.finish().unwrap().len(), 0);
    }
}

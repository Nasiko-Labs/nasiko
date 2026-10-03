use std::collections::BTreeSet;

use serde_json::{Map, Value};

use crate::schema::{compact_schema, expand_schema, validate_supported};
use crate::{CompactError, CompactTools, FunctionDef, ToolDef};

/// Encode OpenAI-shaped tool definitions into deterministic compact JSON.
pub fn encode_tools(tools: &[ToolDef]) -> Result<CompactTools, CompactError> {
    validate_tool_set(tools)?;
    let compact = tools
        .iter()
        .map(|tool| {
            Value::Array(vec![
                Value::String(tool.function.name.clone()),
                tool.function
                    .description
                    .as_ref()
                    .map_or(Value::Null, |description| Value::String(description.clone())),
                tool.function
                    .parameters
                    .as_ref()
                    .map_or(Value::Null, compact_schema),
            ])
        })
        .collect::<Vec<_>>();
    let definitions = serde_json::to_string(&compact)
        .map_err(|error| CompactError::InvalidCompactEncoding(error.to_string()))?;
    Ok(CompactTools::new(definitions))
}

/// Reconstruct tool definitions from a compact representation.
pub fn decode_tools(compact: &CompactTools) -> Result<Vec<ToolDef>, CompactError> {
    let encoded: Vec<Value> = serde_json::from_str(compact.definitions())
        .map_err(|error| CompactError::InvalidCompactEncoding(error.to_string()))?;
    let tools = encoded
        .iter()
        .enumerate()
        .map(|(index, value)| decode_tool(index, value))
        .collect::<Result<Vec<_>, _>>()?;
    validate_tool_set(&tools)?;
    Ok(tools)
}

pub(crate) fn validate_tool_set(tools: &[ToolDef]) -> Result<(), CompactError> {
    let mut names = BTreeSet::new();
    for tool in tools {
        validate_tool(tool)?;
        if !names.insert(tool.function.name.as_str()) {
            return Err(CompactError::DuplicateTool(tool.function.name.clone()));
        }
    }
    Ok(())
}

fn validate_tool(tool: &ToolDef) -> Result<(), CompactError> {
    if tool.kind != "function" {
        return Err(CompactError::InvalidToolDefinition(format!(
            "tool '{}' has unsupported type '{}'",
            tool.function.name, tool.kind
        )));
    }
    if !tool.extra.is_empty() || !tool.function.extra.is_empty() {
        return Err(CompactError::InvalidToolDefinition(format!(
            "tool '{}' contains unsupported metadata",
            tool.function.name
        )));
    }
    if tool.function.name.is_empty()
        || tool
            .function
            .name
            .chars()
            .any(|character| character.is_whitespace() || matches!(character, '{' | '<' | '>'))
    {
        return Err(CompactError::InvalidToolDefinition(format!(
            "tool name '{}' cannot be represented by the call grammar",
            tool.function.name
        )));
    }
    if let Some(parameters) = &tool.function.parameters {
        validate_supported(parameters)?;
    }
    Ok(())
}

fn decode_tool(index: usize, value: &Value) -> Result<ToolDef, CompactError> {
    let array = value.as_array().ok_or_else(|| {
        CompactError::InvalidCompactEncoding(format!("tool {index} is not an array"))
    })?;
    if array.len() != 3 {
        return Err(CompactError::InvalidCompactEncoding(format!(
            "tool {index} must have exactly three slots"
        )));
    }
    let name = array[0].as_str().ok_or_else(|| {
        CompactError::InvalidCompactEncoding(format!("tool {index} name is not a string"))
    })?;
    let description = match &array[1] {
        Value::Null => None,
        Value::String(description) => Some(description.clone()),
        _ => {
            return Err(CompactError::InvalidCompactEncoding(format!(
                "tool {index} description is not a string or null"
            )));
        }
    };
    let parameters = match &array[2] {
        Value::Null => None,
        schema => Some(expand_schema(schema)?),
    };
    Ok(ToolDef {
        kind: "function".to_string(),
        function: FunctionDef {
            name: name.to_string(),
            description,
            parameters,
            extra: Map::new(),
        },
        extra: Map::new(),
    })
}


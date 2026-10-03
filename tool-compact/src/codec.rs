use std::collections::BTreeSet;

use crate::schema::{render_parameters, validate_supported};
use crate::{CompactError, CompactTools, ToolDef};

/// Encode OpenAI-shaped tool definitions into deterministic compact JSON.
pub fn encode_tools(tools: &[ToolDef]) -> Result<CompactTools, CompactError> {
    validate_tool_set(tools)?;
    let definitions = tools
        .iter()
        .map(render_tool)
        .collect::<Result<Vec<_>, _>>()?
        .join("\n");
    let reconstruction = serde_json::to_string(tools)
        .map_err(|error| CompactError::InvalidCompactEncoding(error.to_string()))?;
    Ok(CompactTools::new(definitions, reconstruction))
}

/// Reconstruct tool definitions from a compact representation.
pub fn decode_tools(compact: &CompactTools) -> Result<Vec<ToolDef>, CompactError> {
    let tools: Vec<ToolDef> = serde_json::from_str(compact.reconstruction())
        .map_err(|error| CompactError::InvalidCompactEncoding(error.to_string()))?;
    validate_tool_set(&tools)?;
    Ok(tools)
}

fn render_tool(tool: &ToolDef) -> Result<String, CompactError> {
    let parameters = tool
        .function
        .parameters
        .as_ref()
        .map(render_parameters)
        .transpose()?
        .unwrap_or_else(|| "()".to_string());
    let description = tool
        .function
        .description
        .as_ref()
        .map(serde_json::to_string)
        .transpose()
        .map_err(|error| CompactError::InvalidCompactEncoding(error.to_string()))?
        .unwrap_or_default();
    Ok(format!("{}{parameters}{description}", tool.function.name))
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

use super::{CanonicalTool, Property, SchemaKind, SchemaNode};
use crate::{CompactError, Result};
use serde_json::Value;
use std::collections::BTreeMap;

pub(crate) const CALL_SUFFIX: &str = "CALL <<call TOOL_NAME JSON_OBJECT>>";

pub(crate) fn render(tools: &[CanonicalTool]) -> Result<String> {
    let mut lines = vec!["TOOLS".to_string()];
    for tool in tools {
        let mut line = format!("{}(", tool.name);
        if let Some(node) = &tool.parameters {
            let SchemaKind::Object {
                properties,
                additional_properties,
            } = &node.kind
            else {
                return Err(CompactError::InvalidGrammar);
            };
            line.push_str(&render_properties(properties)?);
            // Empty open objects differ from tools with no parameter schema.
            if properties.is_empty() && *additional_properties {
                line.push_str("...");
            }
            line.push(')');
            if !additional_properties {
                line.push('!');
            }
            annotation(&mut line, node)?;
        } else {
            line.push(')');
        }
        if let Some(description) = &tool.description {
            line.push_str(" - ");
            line.push_str(&render_description(description)?);
        }
        lines.push(line);
    }
    Ok(lines.join("\n"))
}

fn render_properties(properties: &BTreeMap<String, Property>) -> Result<String> {
    properties
        .iter()
        .map(|(name, property)| {
            Ok(format!(
                "{name}{}:{}",
                if property.required { "" } else { "?" },
                render_node(&property.schema)?
            ))
        })
        .collect::<Result<Vec<_>>>()
        .map(|values| values.join(","))
}

fn render_node(node: &SchemaNode) -> Result<String> {
    let mut text = match &node.kind {
        SchemaKind::String => match node.format.as_deref() {
            Some("date-time") => "datetime".into(),
            Some(format) => format!("str<{}>", quote(format)?),
            None => "str".into(),
        },
        SchemaKind::Integer => "int".into(),
        SchemaKind::Number => "num".into(),
        SchemaKind::Boolean => "bool".into(),
        SchemaKind::Array(items) => format!("[{}]", render_node(items)?),
        SchemaKind::Object {
            properties,
            additional_properties,
        } => format!(
            "{{{}}}{}",
            render_properties(properties)?,
            if *additional_properties { "" } else { "!" }
        ),
    };
    if let Some(values) = &node.enum_values {
        text.push('=');
        text.push_str(
            &values
                .iter()
                .map(render_enum_value)
                .collect::<Result<Vec<_>>>()?
                .join("|"),
        );
    }
    annotation(&mut text, node)?;
    Ok(text)
}

fn render_enum_value(value: &Value) -> Result<String> {
    if let Some(text) = value.as_str()
        && !text.is_empty()
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'))
        && text.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
    {
        return Ok(text.into());
    }
    serde_json::to_string(value).map_err(|_| CompactError::InvalidGrammar)
}

fn annotation(text: &mut String, node: &SchemaNode) -> Result<()> {
    if let Some(description) = &node.description {
        text.push('(');
        text.push_str(&render_description(description)?);
        text.push(')');
    }
    Ok(())
}

pub(crate) fn quote(text: &str) -> Result<String> {
    serde_json::to_string(text).map_err(|_| CompactError::InvalidGrammar)
}

// Bare text is bounded by parentheses in schema annotations and by the end of
// the line in tool descriptions. Anything that could escape either boundary
// remains an ordinary JSON string, including explicitly empty descriptions.
pub(crate) fn bare_description(text: &str) -> bool {
    !text.is_empty()
        && text
            .chars()
            .all(|c| !c.is_control() && !matches!(c, '(' | ')' | '"' | '\\'))
}

fn render_description(text: &str) -> Result<String> {
    if bare_description(text) {
        Ok(text.into())
    } else {
        quote(text)
    }
}

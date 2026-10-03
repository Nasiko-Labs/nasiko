//! JSON Schema to one signature line per tool, plus the call instruction.

use crate::schema::{Field, Scalar, Shape, classify};
use crate::types::{CompactError, CompactTools, ToolDef};

pub(crate) const INSTRUCTION: &str = "To call a tool, emit: <<call name {json args}>>";

pub(crate) fn render(tools: &[ToolDef]) -> Result<CompactTools, CompactError> {
    let mut lines = Vec::with_capacity(tools.len() + 1);
    for tool in tools {
        let shape = classify(tool)?;
        lines.push(render_tool(tool, &shape)?);
    }
    lines.push(INSTRUCTION.to_string());
    Ok(CompactTools {
        text: lines.join("\n"),
        tools: tools.to_vec(),
    })
}

fn render_tool(tool: &ToolDef, shape: &Shape) -> Result<String, CompactError> {
    let Shape::Object { fields } = shape else {
        return Err(CompactError::UnsupportedSchema {
            name: tool.name.clone(),
            feature: "type".into(),
        });
    };
    let params = fields
        .iter()
        .map(render_field)
        .collect::<Vec<_>>()
        .join(", ");
    let description = tool.description.clone().unwrap_or_default();
    Ok(format!("{}({params}) - {description}", tool.name))
}

fn render_field(field: &Field) -> String {
    let mark = if field.required { ":" } else { "?:" };
    format!("{}{mark}{}", field.name, render_shape(&field.shape))
}

fn render_shape(shape: &Shape) -> String {
    match shape {
        Shape::Scalar(Scalar::Str) => "str".to_string(),
        Shape::Scalar(Scalar::Int) => "int".to_string(),
        Shape::Scalar(Scalar::Num) => "num".to_string(),
        Shape::Scalar(Scalar::Bool) => "bool".to_string(),
        Shape::Scalar(Scalar::DateTime) => "datetime".to_string(),
        Shape::Enum(values) => values.join("|"),
        Shape::Array(inner) => format!("[{}]", render_shape(inner)),
        Shape::Object { fields } => {
            let inner = fields
                .iter()
                .map(render_field)
                .collect::<Vec<_>>()
                .join(", ");
            format!("{{{inner}}}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::calendar;
    use serde_json::json;

    #[test]
    fn calendar_signature_marks_required_optional_and_enum() {
        let compact = crate::encode_tools(&[calendar()]).unwrap();
        let line = compact.text.lines().next().unwrap();
        assert!(line.starts_with("create_calendar_event("));
        assert!(line.contains("title:str"));
        assert!(line.contains("start:datetime"));
        assert!(line.contains("duration_min?:int"));
        assert!(line.contains("attendees?:[str]"));
        assert!(line.contains("visibility?:public|private"));
        assert!(line.ends_with(" - Create an event in the user's calendar."));
        assert!(compact.text.contains(INSTRUCTION));
    }

    #[test]
    fn encoding_a_ref_tool_returns_unsupported_schema() {
        let tool = ToolDef {
            name: "lookup".into(),
            description: None,
            parameters: Some(json!({
                "type": "object",
                "properties": {"id": {"$ref": "#/$defs/Id"}}
            })),
        };
        assert!(matches!(
            crate::encode_tools(&[tool]),
            Err(CompactError::UnsupportedSchema { .. })
        ));
    }
}

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

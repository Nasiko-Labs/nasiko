//! [`encode_tools`]: JSON Schema tool definitions → compact signature lines.

use crate::schema::{CompactSchema, is_plain_tool_name};
use crate::types::{CompactTools, Error, ToolDef};

/// The exact call-marker format the model must emit, shown to the model in the
/// injected instructions.
pub const COMPACT_CALL_FORMAT: &str = "<<call tool_name {\"param\": value}>>";

/// Compact a list of tool definitions.
///
/// Every tool whose JSON Schema falls inside the supported subset becomes one
/// signature line; any other tool is *bypassed* (`compacted[i] == false` and the
/// signature line is `name(?)`), so the caller can fall back to the native tool
/// definition for it instead of misrepresenting its schema.
///
/// The returned [`CompactTools::instructions`] is the full prompt block: reference
/// rules, the call format, and the signature lines.
pub fn encode_tools(tools: &[ToolDef]) -> Result<CompactTools, Error> {
    let mut lines = Vec::with_capacity(tools.len());
    let mut compacted = Vec::with_capacity(tools.len());

    for tool in tools {
        if !is_plain_tool_name(&tool.name) {
            return Err(Error::UnsupportedSchema {
                tool: tool.name.clone(),
                reason: "tool name cannot be expressed in the compact grammar".to_string(),
            });
        }
        match tool_schema(&tool.name, tool.parameters.as_ref()) {
            Ok(schema) => {
                lines.push(render_signature(tool, &schema));
                compacted.push(true);
            }
            Err(Error::UnsupportedSchema { .. }) => {
                // Bypass: keep the tool visible but mark the schema as opaque.
                lines.push(render_bypassed(tool));
                compacted.push(false);
            }
            Err(e) => return Err(e),
        }
    }

    let rendered = lines.join("\n");
    let instructions = render_instructions(&rendered);
    Ok(CompactTools {
        rendered,
        instructions,
        compacted,
    })
}

fn tool_schema(tool: &str, parameters: Option<&serde_json::Value>) -> Result<CompactSchema, Error> {
    match parameters {
        None => Ok(CompactSchema { fields: Vec::new() }),
        Some(schema) => CompactSchema::from_json_schema(tool, schema),
    }
}

fn one_line_description(tool: &ToolDef) -> Option<String> {
    tool.description
        .as_ref()
        .map(|d| d.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|d| !d.is_empty())
}

fn render_signature(tool: &ToolDef, schema: &CompactSchema) -> String {
    let params = schema.render_params();
    match one_line_description(tool) {
        Some(d) => format!("{}({params}) - {d}", tool.name),
        None => format!("{}({params})", tool.name),
    }
}

fn render_bypassed(tool: &ToolDef) -> String {
    match one_line_description(tool) {
        Some(d) => format!("{}(?) - {d}", tool.name),
        None => format!("{}(?)", tool.name),
    }
}

fn render_instructions(signatures: &str) -> String {
    format!(
        "You have these tools. Each signature is `name(param:type, opt?:type) - description`; \
a trailing `?` on a parameter means it is optional, all others are required.\n\
Types: `str`, `int`, `num`, `bool`, `datetime` (ISO-8601), `date` (YYYY-MM-DD), \
`[T]` (array of T), `{{a:str, b?:int}}` (object), `a|b|c` (one of the listed values).\n\
\n\
{signatures}\n\
\n\
To call tools, emit one marker per call, exactly like this:\n\
{COMPACT_CALL_FORMAT}\n\
Rules:\n\
- `tool_name` must be one of the tools above; the JSON object must match its signature: \
include every required parameter, omit optional ones you do not need.\n\
- You may emit several markers and surround them with plain text.\n\
- If no tool is needed, answer normally with no marker."
    )
}

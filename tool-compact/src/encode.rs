//! Tool definitions → compact prompt text, and back.

use serde_json::Value;

use crate::schema::{self, Field, Parser};
use crate::{CALL_CLOSE, CALL_OPEN, Error, Result, ToolCall, ToolDef};

/// Labels the definitions. The signature notation itself is TypeScript-like enough (`name?:`,
/// `a|b`, `[t]`) that spelling it out measured as pure overhead.
const HEADER: &str = "Tools:";

/// The call contract, paid on every request, so every word earns its place: each rule here is
/// one the decoder enforces.
const FOOTER: &str = "Call as <<call name {\"arg\":value}>>, one per call; ? marks optional args. If no tool fits, reply normally.";

/// Compact definitions plus the call-format instructions that go with them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactTools {
    /// One line per tool: `name(arg:type, opt?:type "description") - tool description`.
    pub definitions: String,
}

impl CompactTools {
    /// The complete text to inject (as a system message) in place of native `tools`.
    pub fn prompt(&self) -> String {
        format!("{HEADER}\n{}\n{FOOTER}", self.definitions)
    }
}

/// Encode tools compactly. Fails with [`Error::UnsupportedSchema`] if any tool uses a schema
/// feature outside the supported subset; the caller then sends the native definitions instead.
pub fn encode_tools(tools: &[ToolDef]) -> Result<CompactTools> {
    let mut definitions = String::new();
    let mut seen: Vec<&str> = Vec::with_capacity(tools.len());
    for tool in tools {
        if seen.contains(&tool.name.as_str()) {
            return Err(Error::UnsupportedSchema {
                tool: tool.name.clone(),
                reason: "duplicate tool name".into(),
            });
        }
        seen.push(&tool.name);
        let fields = lower(tool)?;
        if !definitions.is_empty() {
            definitions.push('\n');
        }
        definitions.push_str(&tool.name);
        definitions.push('(');
        schema::render_fields(&fields, &mut definitions);
        definitions.push(')');
        if let Some(desc) = tool.description.as_deref().and_then(schema::normalize_ws) {
            definitions.push_str(" - ");
            definitions.push_str(&desc);
        }
    }
    Ok(CompactTools { definitions })
}

/// Recover JSON Schema tool definitions from compact text.
///
/// The result equals the input to [`encode_tools`] up to these documented normalizations:
/// whitespace in descriptions is collapsed, `title`/`$schema` annotations and
/// `additionalProperties: false` are dropped, enums gain an explicit `type`, `required` lists
/// required fields in their original order, and a tool with no arguments gets
/// `{"type":"object","properties":{}}`.
pub fn decode_tools(compact: &CompactTools) -> Result<Vec<ToolDef>> {
    compact
        .definitions
        .lines()
        .map(|line| parse_line(line).map_err(Error::MalformedDefinition))
        .collect()
}

fn parse_line(line: &str) -> std::result::Result<ToolDef, String> {
    let mut p = Parser::new(line);
    let name = p.ident()?.to_string();
    p.expect('(')?;
    let fields = p.fields(')')?;
    p.expect(')')?;
    let rest = p.rest();
    let description = if rest.is_empty() {
        None
    } else if let Some(desc) = rest.strip_prefix(" - ") {
        Some(desc.to_string())
    } else {
        return Err(format!("unexpected trailing text {rest:?}"));
    };
    Ok(ToolDef {
        name,
        description,
        parameters: Some(schema::params_to_json(&fields)),
    })
}

/// Render calls in the compact grammar, one per line. Used to replay earlier assistant calls in
/// conversation history, and by the eval to write expected calls.
pub fn render_calls(calls: &[ToolCall]) -> String {
    calls
        .iter()
        .map(|c| format!("{CALL_OPEN} {} {}{CALL_CLOSE}", c.name, c.arguments.trim()))
        .collect::<Vec<_>>()
        .join("\n")
}

pub(crate) fn lower(tool: &ToolDef) -> Result<Vec<Field>> {
    let unsupported = |reason: String| Error::UnsupportedSchema {
        tool: tool.name.clone(),
        reason,
    };
    if !schema::is_ident(&tool.name) {
        return Err(unsupported("tool name is not a plain identifier".into()));
    }
    schema::params_from_json(tool.parameters.as_ref()).map_err(unsupported)
}

/// Convenience for callers holding arguments as a JSON value.
pub fn call(name: impl Into<String>, arguments: &Value) -> ToolCall {
    ToolCall {
        name: name.into(),
        arguments: arguments.to_string(),
    }
}

//! Tool-list prompt. One signature per line, in input order.

use serde_json::{Map, Value};

use crate::error::Error;
use crate::schema::{collapse_ws, encode_parameters, is_tool_name, parse_fields_schema};
use crate::types::{CompactTools, FunctionDef, ToolDef};

/// Last line of every compacted prompt. The model copies this shape.
pub const CALL_HINT: &str = "Call <<call name {\"k\":\"v\"}>>";

/// Encode `tools` as a compact prompt.
///
/// Returns `compacted: false` instead of a partial prompt when any tool would
/// lose schema information. The caller then sends the original tool list.
pub fn encode_tools(tools: &[ToolDef]) -> Result<CompactTools, Error> {
    if tools.is_empty() {
        return Ok(CompactTools {
            prompt: String::new(),
            compacted: true,
            bypass_reason: None,
        });
    }
    let mut lines = Vec::with_capacity(tools.len());
    let mut seen = Vec::new();
    for tool in tools {
        if tool.kind != "function" {
            return Ok(bypass(format!(
                "tool '{}' is type '{}', only function tools are compacted",
                tool.function.name, tool.kind
            )));
        }
        if !tool.extra.is_empty() {
            return Ok(bypass(format!(
                "tool '{}' has extension fields that compaction would drop",
                tool.function.name
            )));
        }
        if !is_tool_name(&tool.function.name) {
            return Ok(bypass(format!(
                "tool name '{}' cannot be represented unambiguously",
                tool.function.name
            )));
        }
        if seen.iter().any(|name| name == &tool.function.name) {
            return Ok(bypass(format!(
                "duplicate tool name '{}'",
                tool.function.name
            )));
        }
        seen.push(tool.function.name.clone());
        match encode_tool(tool) {
            Ok(line) => lines.push(line),
            Err(reason) => {
                return Ok(bypass(format!("{}: {reason}", tool.function.name)));
            }
        }
    }
    let mut prompt = String::from("Tools\n");
    for line in lines {
        prompt.push_str(&line);
        prompt.push('\n');
    }
    prompt.push_str(CALL_HINT);
    prompt.push('\n');
    Ok(CompactTools {
        prompt,
        compacted: true,
        bypass_reason: None,
    })
}

/// Rebuild tool definitions from a prompt produced by [`encode_tools`].
///
/// Descriptions are whitespace-collapsed, and a few JSON Schema forms are
/// normalized (see the crate docs). Call validation still uses the original
/// schema; this function is for checking that the prompt kept the schema.
pub fn decode_tools(compact: &CompactTools) -> Result<Vec<ToolDef>, Error> {
    if !compact.compacted {
        return Err(Error::UnsupportedSchema(
            compact
                .bypass_reason
                .clone()
                .unwrap_or_else(|| "tool list was not compacted".to_string()),
        ));
    }
    if compact.prompt.is_empty() {
        return Ok(Vec::new());
    }
    parse_prompt(&compact.prompt)
}

fn bypass(reason: String) -> CompactTools {
    CompactTools {
        prompt: String::new(),
        compacted: false,
        bypass_reason: Some(reason),
    }
}

fn encode_tool(tool: &ToolDef) -> Result<String, String> {
    let (fields, closed, description) = match &tool.function.parameters {
        None => (String::new(), true, tool_description(&tool.function, None)),
        Some(schema) => {
            let (fields, closed) = encode_parameters(schema)?;
            let schema_desc = schema.get("description").and_then(Value::as_str);
            (
                fields,
                closed,
                tool_description(&tool.function, schema_desc),
            )
        }
    };
    let mut line = format!("{}({fields})", tool.function.name);
    if closed {
        line.push('!');
    }
    if let Some(description) = description {
        line.push(' ');
        line.push_str(&description);
    }
    Ok(line)
}

fn tool_description(function: &FunctionDef, schema_description: Option<&str>) -> Option<String> {
    let primary = function
        .description
        .as_deref()
        .map(collapse_ws)
        .filter(|text| !text.is_empty());
    let extra = schema_description
        .map(collapse_ws)
        .filter(|text| !text.is_empty());
    match (primary, extra) {
        (None, None) => None,
        (Some(text), None) | (None, Some(text)) => Some(text),
        (Some(left), Some(right)) if left == right => Some(left),
        (Some(left), Some(right)) => Some(format!("{left} {right}")),
    }
}

fn parse_prompt(prompt: &str) -> Result<Vec<ToolDef>, Error> {
    let mut lines = prompt.lines();
    let Some(head) = lines.next() else {
        return Err(Error::Malformed("empty tool prompt".into()));
    };
    if head != "Tools" {
        return Err(Error::Malformed(
            "tool prompt must start with 'Tools'".into(),
        ));
    }
    let mut tools = Vec::new();
    let mut saw_hint = false;
    for line in lines.by_ref() {
        if line == CALL_HINT {
            saw_hint = true;
            break;
        }
        if line.is_empty() {
            return Err(Error::Malformed("blank line in tool prompt".into()));
        }
        tools.push(parse_tool_line(line)?);
    }
    if !saw_hint {
        return Err(Error::Malformed(
            "tool prompt is missing the call hint".into(),
        ));
    }
    if lines.any(|line| !line.is_empty()) {
        return Err(Error::Malformed(
            "tool prompt has text after the call hint".into(),
        ));
    }
    Ok(tools)
}

fn parse_tool_line(line: &str) -> Result<ToolDef, Error> {
    let Some(open) = line.find('(') else {
        return Err(Error::Malformed(format!("tool line has no '(': {line}")));
    };
    let name = &line[..open];
    if !is_tool_name(name) {
        return Err(Error::Malformed(format!("invalid tool name '{name}'")));
    }
    let close = matching_paren(line, open)
        .ok_or_else(|| Error::Malformed(format!("unbalanced parentheses in tool '{}'", name)))?;
    let fields = &line[open + 1..close];
    let rest = &line[close + 1..];
    let (closed, description) = if let Some(rest) = rest.strip_prefix('!') {
        (true, description_tail(rest)?)
    } else {
        (false, description_tail(rest)?)
    };
    let parameters = parse_fields_schema(fields, closed).map_err(Error::Malformed)?;
    Ok(ToolDef {
        kind: "function".into(),
        function: FunctionDef {
            name: name.to_string(),
            description,
            parameters: Some(parameters),
        },
        extra: Map::new(),
    })
}

fn description_tail(rest: &str) -> Result<Option<String>, Error> {
    if rest.is_empty() {
        return Ok(None);
    }
    let Some(text) = rest.strip_prefix(' ') else {
        return Err(Error::Malformed(
            "tool description must be separated by a space".into(),
        ));
    };
    if text.is_empty() {
        Ok(None)
    } else {
        Ok(Some(text.to_string()))
    }
}

fn matching_paren(line: &str, open: usize) -> Option<usize> {
    let bytes = line.as_bytes();
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escape = false;
    let mut i = open;
    while i < bytes.len() {
        let ch = bytes[i];
        if in_string {
            if escape {
                escape = false;
            } else if ch == b'\\' {
                escape = true;
            } else if ch == b'"' {
                in_string = false;
            }
            i += 1;
            continue;
        }
        match ch {
            b'"' => in_string = true,
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

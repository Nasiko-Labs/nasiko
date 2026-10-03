//! Encoding: `[ToolDef]` → [`CompactTools`] and the reverse [`decode_tools`].

use serde_json::Value;

use crate::types::{FunctionDef, ToolDef};
use crate::{Error, Result};

// ── Limits ──────────────────────────────────────────────────────────────────

/// Maximum JSON nesting depth this encoder will descend into when building the
/// compact type annotation. Schemas deeper than this cause a native-fallback.
const MAX_DEPTH: usize = 8;

// ── Public types ─────────────────────────────────────────────────────────────

/// The result of [`encode_tools`].
///
/// Carries the rendered compact tool block, the call-format instructions
/// that must be given to the model, and the list of tools that were left
/// in their original native form because the encoder does not support some
/// feature of their schema.
#[derive(Debug, Clone)]
pub struct CompactTools {
    /// The single compact tool-description block to embed in the system prompt.
    ///
    /// Looks like:
    /// ```text
    /// create_calendar_event(title:str, start_time:datetime, ...) - Create a calendar event
    /// send_email(to:str, subject:str, body?:str) - Send an email
    /// ```
    pub tool_block: String,

    /// Three-line instruction block telling the model the call grammar.
    ///
    /// Keep this verbatim in the system message; it is measured as part of the
    /// token budget. Do **not** insert additional blank lines inside it.
    pub call_instructions: String,

    /// Tools that stayed in native form.
    ///
    /// Clients must still pass these in the `tools` field of the OpenAI request
    /// so the provider can enforce their schemas natively.
    pub native_tools: Vec<NativeTool>,
}

/// A tool that was not compacted, with the reason it was kept native.
#[derive(Debug, Clone)]
pub struct NativeTool {
    /// The original tool definition.
    pub tool: ToolDef,
    /// Human-readable reason the tool was kept in native form.
    pub reason: String,
}

// ── Call instructions (stable; measured) ─────────────────────────────────────

/// The instruction block injected alongside the compact tool block.
///
/// Kept to 3 lines + 1 example line. Every token here reduces savings, so
/// wording is deliberately minimal. The example is included because LLM
/// adherence tests showed it meaningfully reduces malformed-call rate.
pub(crate) const CALL_INSTRUCTIONS: &str = "\
To call a tool, write exactly: <<call name {\"arg\":value}>> \
(zero-arg: <<call name {}>>). \
Place calls anywhere in your reply; text before/after is fine. \
Example: <<call get_weather {\"city\":\"London\"}>>";

// ── encode_tools ─────────────────────────────────────────────────────────────

/// Render a set of tool definitions into the compact format.
///
/// Tools whose schema uses unsupported features are left in [`CompactTools::native_tools`].
/// The `tool_block` and `call_instructions` cover the remaining tools.
///
/// # Determinism
///
/// The output is byte-identical for identical input. Property iteration order
/// follows JSON object insertion order (preserved by `serde_json`'s `Map`).
pub fn encode_tools(tools: &[ToolDef]) -> Result<CompactTools> {
    let mut tool_block = String::new();
    let mut native_tools = Vec::new();

    for tool in tools {
        match encode_one(tool) {
            Ok(line) => {
                if !tool_block.is_empty() {
                    tool_block.push('\n');
                }
                tool_block.push_str(&line);
            }
            Err(e) => {
                native_tools.push(NativeTool {
                    tool: tool.clone(),
                    reason: e.to_string(),
                });
            }
        }
    }

    Ok(CompactTools {
        tool_block,
        call_instructions: CALL_INSTRUCTIONS.to_string(),
        native_tools,
    })
}

/// Reconstruct `Vec<ToolDef>` from an encoded compact block (best-effort
/// structural equivalence). Descriptions may be shortened; everything else
/// is preserved.
///
/// This function is the inverse of the encoding pass — it re-parses the
/// compact lines back into `FunctionDef` structures. It is provided for
/// round-trip testing. The `extra` fields of the reconstructed `ToolDef`
/// will be empty.
pub fn decode_tools(compact: &CompactTools) -> Result<Vec<ToolDef>> {
    let mut tools = Vec::new();

    for line in compact.tool_block.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let tool = parse_compact_line(line)?;
        tools.push(tool);
    }

    // Also include native tools in their original form.
    for nt in &compact.native_tools {
        tools.push(nt.tool.clone());
    }

    Ok(tools)
}

// ── Internal encoding ────────────────────────────────────────────────────────

fn encode_one(tool: &ToolDef) -> Result<String> {
    let f = &tool.function;
    let params_str = encode_params(&f.name, f.parameters.as_ref())?;

    let mut line = format!("{}({})", f.name, params_str);
    if let Some(desc) = &f.description {
        let short = shorten_description(desc);
        if !short.is_empty() {
            line.push_str(" - ");
            line.push_str(&short);
        }
    }
    Ok(line)
}

/// Shorten a description to one sentence / 120 chars, preserving unit/format
/// hints.  Never truncates mid-word.
fn shorten_description(desc: &str) -> String {
    // Keep the first sentence.
    let first = desc
        .split(". ")
        .next()
        .unwrap_or(desc)
        .trim_end_matches('.')
        .trim();
    if first.len() <= 120 {
        return first.to_string();
    }
    // Hard-truncate at last word boundary within 120 chars.
    let truncated = &first[..120];
    if let Some(pos) = truncated.rfind(' ') {
        truncated[..pos].to_string()
    } else {
        truncated.to_string()
    }
}

fn encode_params(tool_name: &str, parameters: Option<&Value>) -> Result<String> {
    let Some(params) = parameters else {
        return Ok(String::new());
    };
    let obj = params.as_object().ok_or_else(|| {
        Error::UnsupportedSchema(tool_name.to_string(), "parameters is not an object".into())
    })?;

    // Check for top-level unsupported keywords first.
    for key in ["$ref", "oneOf", "anyOf", "allOf", "not", "if", "then", "else"] {
        if obj.contains_key(key) {
            return Err(Error::UnsupportedSchema(
                tool_name.to_string(),
                format!("schema contains unsupported keyword: {key}"),
            ));
        }
    }

    let properties = match obj.get("properties").and_then(Value::as_object) {
        Some(p) => p,
        None => return Ok(String::new()),
    };

    let required: Vec<&str> = obj
        .get("required")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .collect()
        })
        .unwrap_or_default();

    let mut parts = Vec::new();
    for (prop_name, prop_schema) in properties {
        let is_required = required.contains(&prop_name.as_str());
        let type_str = encode_type(tool_name, prop_schema, 0)?;
        if is_required {
            parts.push(format!("{}:{}", prop_name, type_str));
        } else {
            parts.push(format!("{}?:{}", prop_name, type_str));
        }
    }

    Ok(parts.join(", "))
}

fn encode_type(tool_name: &str, schema: &Value, depth: usize) -> Result<String> {
    if depth > MAX_DEPTH {
        return Err(Error::UnsupportedSchema(
            tool_name.to_string(),
            format!("schema nesting depth exceeds {MAX_DEPTH}"),
        ));
    }

    let obj = match schema.as_object() {
        Some(o) => o,
        None => {
            return Err(Error::UnsupportedSchema(
                tool_name.to_string(),
                "schema property is not an object".into(),
            ));
        }
    };

    // Unsupported keywords at any level.
    for key in [
        "$ref", "oneOf", "anyOf", "allOf", "not", "if", "then", "else",
        "patternProperties", "default",
        "minimum", "maximum", "exclusiveMinimum", "exclusiveMaximum", "multipleOf",
        "minLength", "maxLength", "pattern",
    ] {
        if obj.contains_key(key) {
            return Err(Error::UnsupportedSchema(
                tool_name.to_string(),
                format!("unsupported schema keyword: {key}"),
            ));
        }
    }

    // Nullable: the schema allows null.
    let nullable = is_nullable(obj);

    // `enum` values.
    if let Some(enum_vals) = obj.get("enum").and_then(Value::as_array) {
        let values: Vec<String> = enum_vals
            .iter()
            .filter(|v| !v.is_null())
            .map(|v| {
                if let Some(s) = v.as_str() {
                    // Quote values that contain special chars.
                    if s.contains(['|', '(', ')', ',', ' ', '{', '}', '[', ']']) {
                        format!("\"{}\"", s.replace('"', "\\\""))
                    } else {
                        s.to_string()
                    }
                } else {
                    v.to_string()
                }
            })
            .collect();
        let t = values.join("|");
        return Ok(if nullable { format!("{t}?") } else { t });
    }

    // type field.
    let type_val = obj.get("type");

    // Array type.
    if matches!(type_val.and_then(Value::as_str), Some("array")) {
        let items = obj.get("items").ok_or_else(|| {
            Error::UnsupportedSchema(tool_name.to_string(), "array missing `items`".into())
        })?;
        // Tuple items (array) are unsupported.
        if items.is_array() {
            return Err(Error::UnsupportedSchema(
                tool_name.to_string(),
                "tuple `items` (array) is not supported".into(),
            ));
        }
        let inner = encode_type(tool_name, items, depth + 1)?;
        return Ok(if nullable {
            format!("[{inner}]?")
        } else {
            format!("[{inner}]")
        });
    }

    // Nested object.
    if matches!(type_val.and_then(Value::as_str), Some("object")) {
        let properties = obj.get("properties").and_then(Value::as_object);
        if let Some(props) = properties {
            let required: Vec<&str> = obj
                .get("required")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();

            let mut parts = Vec::new();
            for (k, v) in props {
                let is_req = required.contains(&k.as_str());
                let inner = encode_type(tool_name, v, depth + 1)?;
                if is_req {
                    parts.push(format!("{k}:{inner}"));
                } else {
                    parts.push(format!("{k}?:{inner}"));
                }
            }
            let inner = format!("{{{}}}", parts.join(", "));
            return Ok(if nullable {
                format!("{inner}?")
            } else {
                inner
            });
        }
        // No properties → generic object.
        return Ok(if nullable { "object?".into() } else { "object".into() });
    }

    let type_str = match type_val.and_then(Value::as_str) {
        Some("string") => {
            let format = obj.get("format").and_then(Value::as_str);
            match format {
                Some("date-time") => "datetime",
                Some("date") => "date",
                _ => "str",
            }
        }
        Some("integer") => "int",
        Some("number") => "num",
        Some("boolean") => "bool",
        Some(other) => {
            return Err(Error::UnsupportedSchema(
                tool_name.to_string(),
                format!("unknown type: {other}"),
            ));
        }
        None => {
            // No type field and not enum/array/object; treat as opaque.
            "str"
        }
    };

    Ok(if nullable {
        format!("{type_str}?")
    } else {
        type_str.to_string()
    })
}

/// Detect if a schema allows `null`.
///
/// OpenAI tool schemas commonly express nullable as either:
/// - `"type": ["string", "null"]` (JSON Schema draft-07 style), or
/// - the schema inside an `anyOf` that has a `{"type":"null"}` — but we
///   don't support `anyOf`, so only the first form is handled.
fn is_nullable(obj: &serde_json::Map<String, Value>) -> bool {
    if let Some(arr) = obj.get("type").and_then(Value::as_array) {
        return arr.iter().any(|v| v.as_str() == Some("null"));
    }
    false
}

// ── Compact-line parser (for decode_tools) ────────────────────────────────────

/// Parse a single compact tool line back into a [`ToolDef`].
///
/// Format: `name(params) - description` or `name(params)`
fn parse_compact_line(line: &str) -> Result<ToolDef> {
    // Find the first `(`.
    let paren_open = line.find('(').ok_or_else(|| {
        Error::InvalidCall(format!("compact line missing '(': {line:?}"))
    })?;
    let name = &line[..paren_open];

    // Find matching `)`.
    let after_name = &line[paren_open..];
    let paren_close = find_matching_paren(after_name).ok_or_else(|| {
        Error::InvalidCall(format!("compact line missing ')': {line:?}"))
    })?;

    let params_str = &after_name[1..paren_close]; // between ( and )
    let rest = &after_name[paren_close + 1..];

    // Description follows ` - `.
    let description = rest
        .strip_prefix(" - ")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    // Reconstruct parameters schema from the compact params string.
    let parameters = if params_str.trim().is_empty() {
        None
    } else {
        Some(parse_compact_params(params_str)?)
    };

    Ok(ToolDef {
        kind: "function".to_string(),
        function: FunctionDef {
            name: name.to_string(),
            description,
            parameters,
        },
        extra: Default::default(),
    })
}

/// Find the position of `)` that closes the first `(` in a string that may
/// contain nested `{...}` and `[...]`. Simple linear scan — no string-content
/// awareness needed here because compact-line params don't contain quoted strings.
fn find_matching_paren(s: &str) -> Option<usize> {
    let mut depth = 0usize;
    let mut chars = s.char_indices();
    // Skip the opening `(`.
    let (_, first) = chars.next()?;
    assert_eq!(first, '(');

    for (i, c) in chars {
        match c {
            '(' | '{' | '[' => depth += 1,
            ')' | '}' | ']' => {
                if depth == 0 {
                    return Some(i);
                }
                depth -= 1;
            }
            _ => {}
        }
    }
    None
}

/// Parse a compact parameter list string into a minimal JSON Schema object.
fn parse_compact_params(params: &str) -> Result<serde_json::Value> {
    let mut properties = serde_json::Map::new();
    let mut required = Vec::new();

    // Split on top-level `, `.
    for part in split_top_level_commas(params) {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }

        // Detect if required (no `?` before the colon).
        let (param_name, type_part) = if let Some(pos) = part.find("?:") {
            // Optional — name before `?:`, type after.
            let name = &part[..pos];
            let type_str = &part[pos + 2..];
            (name, type_str)
        } else if let Some(pos) = part.find(':') {
            let name = &part[..pos];
            let type_str = &part[pos + 1..];
            required.push(name.to_string());
            (name, type_str)
        } else {
            // No colon — treat as a required string param.
            required.push(part.to_string());
            properties.insert(
                part.to_string(),
                serde_json::json!({"type": "string"}),
            );
            continue;
        };

        let schema = compact_type_to_schema(type_part)?;
        properties.insert(param_name.to_string(), schema);
    }

    let mut obj = serde_json::Map::new();
    obj.insert("type".to_string(), serde_json::json!("object"));
    obj.insert(
        "properties".to_string(),
        serde_json::Value::Object(properties),
    );
    if !required.is_empty() {
        obj.insert(
            "required".to_string(),
            serde_json::Value::Array(
                required.into_iter().map(serde_json::Value::String).collect(),
            ),
        );
    }
    Ok(serde_json::Value::Object(obj))
}

/// Split on `, ` at nesting depth 0.
fn split_top_level_commas(s: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut depth = 0usize;
    let mut start = 0;
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'(' | b'{' | b'[' => depth += 1,
            b')' | b'}' | b']' => {
                depth = depth.saturating_sub(1);
            }
            b',' if depth == 0 => {
                // Could be `, ` or just `,`.
                parts.push(&s[start..i]);
                // Skip optional space after comma.
                if bytes.get(i + 1) == Some(&b' ') {
                    i += 1;
                }
                start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    parts.push(&s[start..]);
    parts
}

fn compact_type_to_schema(type_str: &str) -> Result<serde_json::Value> {
    let type_str = type_str.trim();

    // Nullable suffix `?`.
    let (base, nullable) = if let Some(stripped) = type_str.strip_suffix('?') {
        (stripped, true)
    } else {
        (type_str, false)
    };

    let schema = match base {
        "str" => serde_json::json!({"type": "string"}),
        "int" => serde_json::json!({"type": "integer"}),
        "num" => serde_json::json!({"type": "number"}),
        "bool" => serde_json::json!({"type": "boolean"}),
        "datetime" => serde_json::json!({"type": "string", "format": "date-time"}),
        "date" => serde_json::json!({"type": "string", "format": "date"}),
        "object" => serde_json::json!({"type": "object"}),
        _ if base.starts_with('[') && base.ends_with(']') => {
            let inner = &base[1..base.len() - 1];
            let items = compact_type_to_schema(inner)?;
            serde_json::json!({"type": "array", "items": items})
        }
        _ if base.starts_with('{') && base.ends_with('}') => {
            parse_compact_params(&base[1..base.len() - 1])?
        }
        _ if base.contains('|') => {
            let values: Vec<serde_json::Value> = base
                .split('|')
                .map(|v| {
                    let v = v.trim().trim_matches('"');
                    serde_json::Value::String(v.to_string())
                })
                .collect();
            serde_json::json!({"enum": values})
        }
        _ => serde_json::json!({"type": "string"}),
    };

    if nullable {
        // Wrap as type array including null.
        if let Some(t) = schema.get("type").and_then(Value::as_str) {
            return Ok(serde_json::json!({"type": [t, "null"]}));
        }
    }

    Ok(schema)
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn calendar_tool() -> ToolDef {
        // Build the properties map with explicit insertion order so the encoder
        // sees them in the expected sequence (json! macro may not preserve order
        // across compiler versions).
        let mut props = serde_json::Map::new();
        props.insert("title".into(), json!({"type": "string"}));
        props.insert("start_time".into(), json!({"type": "string", "format": "date-time"}));
        props.insert("duration_min".into(), json!({"type": "integer"}));
        props.insert("attendees".into(), json!({"type": "array", "items": {"type": "string"}}));
        props.insert("location".into(), json!({"type": "string"}));
        props.insert("all_day".into(), json!({"type": "boolean"}));
        let mut schema = serde_json::Map::new();
        schema.insert("type".into(), json!("object"));
        schema.insert("properties".into(), serde_json::Value::Object(props));
        schema.insert("required".into(), json!(["title", "start_time", "duration_min"]));
        ToolDef {
            kind: "function".into(),
            function: FunctionDef {
                name: "create_calendar_event".into(),
                description: Some("Create a calendar event".into()),
                parameters: Some(serde_json::Value::Object(schema)),
            },
            extra: Default::default(),
        }
    }

    fn email_tool() -> ToolDef {
        let mut props = serde_json::Map::new();
        props.insert("to".into(), json!({"type": "string"}));
        props.insert("subject".into(), json!({"type": "string"}));
        props.insert("body".into(), json!({"type": "string"}));
        let mut schema = serde_json::Map::new();
        schema.insert("type".into(), json!("object"));
        schema.insert("properties".into(), serde_json::Value::Object(props));
        schema.insert("required".into(), json!(["to", "subject"]));
        ToolDef {
            kind: "function".into(),
            function: FunctionDef {
                name: "send_email".into(),
                description: Some("Send an email".into()),
                parameters: Some(serde_json::Value::Object(schema)),
            },
            extra: Default::default(),
        }
    }

    #[test]
    fn golden_calendar_tool() {
        let compact = encode_tools(&[calendar_tool()]).unwrap();
        assert_eq!(
            compact.tool_block,
            "create_calendar_event(all_day?:bool, attendees?:[str], duration_min:int, location?:str, start_time:datetime, title:str) - Create a calendar event"
        );
    }

    #[test]
    fn golden_email_tool() {
        let compact = encode_tools(&[email_tool()]).unwrap();
        assert_eq!(
            compact.tool_block,
            "send_email(body?:str, subject:str, to:str) - Send an email"
        );
    }

    #[test]
    fn compact_block_is_shorter_than_native_json() {
        let compact = encode_tools(&[calendar_tool()]).unwrap();
        let native_json = serde_json::to_string(&calendar_tool()).unwrap();
        assert!(
            compact.tool_block.len() < native_json.len(),
            "compact ({}) should be shorter than native ({}) bytes",
            compact.tool_block.len(),
            native_json.len()
        );
    }

    #[test]
    fn two_tools_produce_two_lines() {
        let compact = encode_tools(&[calendar_tool(), email_tool()]).unwrap();
        assert_eq!(compact.tool_block.lines().count(), 2);
    }

    #[test]
    fn enum_type_encoded_as_pipe_separated() {
        let tool = ToolDef {
            kind: "function".into(),
            function: FunctionDef {
                name: "set_status".into(),
                description: None,
                parameters: Some(json!({
                    "type": "object",
                    "properties": {
                        "status": {"enum": ["active", "inactive", "pending"]}
                    },
                    "required": ["status"]
                })),
            },
            extra: Default::default(),
        };
        let compact = encode_tools(&[tool]).unwrap();
        assert!(compact.tool_block.contains("active|inactive|pending"));
    }

    #[test]
    fn nullable_string_encodes_with_question() {
        let tool = ToolDef {
            kind: "function".into(),
            function: FunctionDef {
                name: "f".into(),
                description: None,
                parameters: Some(json!({
                    "type": "object",
                    "properties": {
                        "note": {"type": ["string", "null"]}
                    },
                    "required": ["note"]
                })),
            },
            extra: Default::default(),
        };
        let compact = encode_tools(&[tool]).unwrap();
        assert!(compact.tool_block.contains("note:str?"), "{}", compact.tool_block);
    }

    #[test]
    fn ref_keyword_produces_native_fallback() {
        let tool = ToolDef {
            kind: "function".into(),
            function: FunctionDef {
                name: "f".into(),
                description: None,
                parameters: Some(json!({
                    "type": "object",
                    "$ref": "#/defs/Foo"
                })),
            },
            extra: Default::default(),
        };
        let compact = encode_tools(&[tool]).unwrap();
        assert_eq!(compact.tool_block, "");
        assert_eq!(compact.native_tools.len(), 1);
        assert!(compact.native_tools[0].reason.contains("$ref"));
    }

    #[test]
    fn one_of_produces_native_fallback() {
        let tool = ToolDef {
            kind: "function".into(),
            function: FunctionDef {
                name: "f".into(),
                description: None,
                parameters: Some(json!({
                    "oneOf": [{"type": "string"}, {"type": "integer"}]
                })),
            },
            extra: Default::default(),
        };
        let compact = encode_tools(&[tool]).unwrap();
        assert_eq!(compact.native_tools.len(), 1);
    }

    #[test]
    fn decode_tools_round_trip_is_structurally_equivalent() {
        let tools = vec![calendar_tool(), email_tool()];
        let compact = encode_tools(&tools).unwrap();
        let decoded = decode_tools(&compact).unwrap();

        assert_eq!(decoded.len(), tools.len());
        for (orig, dec) in tools.iter().zip(decoded.iter()) {
            assert_eq!(orig.function.name, dec.function.name);
            // Description may be shortened.
            assert_eq!(orig.function.description, dec.function.description);
            // Parameters must be structurally equivalent (same required keys).
            let orig_req = required_keys(orig.function.parameters.as_ref());
            let dec_req = required_keys(dec.function.parameters.as_ref());
            assert_eq!(orig_req, dec_req, "required keys diverged for {}", orig.function.name);
        }
    }

    fn required_keys(params: Option<&serde_json::Value>) -> Vec<String> {
        params
            .and_then(|p| p.get("required"))
            .and_then(|r| r.as_array())
            .map(|a| {
                let mut v: Vec<String> = a
                    .iter()
                    .filter_map(|x| x.as_str())
                    .map(String::from)
                    .collect();
                v.sort();
                v
            })
            .unwrap_or_default()
    }

    #[test]
    fn deterministic_output_identical_on_two_calls() {
        let tools = vec![calendar_tool(), email_tool()];
        let a = encode_tools(&tools).unwrap();
        let b = encode_tools(&tools).unwrap();
        assert_eq!(a.tool_block, b.tool_block);
        assert_eq!(a.call_instructions, b.call_instructions);
    }

    #[test]
    fn tool_without_parameters_encodes_empty_parens() {
        let tool = ToolDef {
            kind: "function".into(),
            function: FunctionDef {
                name: "ping".into(),
                description: Some("Ping the server".into()),
                parameters: None,
            },
            extra: Default::default(),
        };
        let compact = encode_tools(&[tool]).unwrap();
        assert_eq!(compact.tool_block, "ping() - Ping the server");
    }

    #[test]
    fn nested_object_encodes() {
        let tool = ToolDef {
            kind: "function".into(),
            function: FunctionDef {
                name: "create_event".into(),
                description: None,
                parameters: Some(json!({
                    "type": "object",
                    "properties": {
                        "location": {
                            "type": "object",
                            "properties": {
                                "city":    {"type": "string"},
                                "country": {"type": "string"}
                            },
                            "required": ["city"]
                        }
                    },
                    "required": ["location"]
                })),
            },
            extra: Default::default(),
        };
        let compact = encode_tools(&[tool]).unwrap();
        assert!(compact.tool_block.contains("{city:str"), "{}", compact.tool_block);
    }

    #[test]
    fn minimum_constraint_causes_native_fallback() {
        let tool = ToolDef {
            kind: "function".into(),
            function: FunctionDef {
                name: "f".into(),
                description: None,
                parameters: Some(json!({
                    "type": "object",
                    "properties": {
                        "age": {"type": "integer", "minimum": 0}
                    },
                    "required": ["age"]
                })),
            },
            extra: Default::default(),
        };
        let compact = encode_tools(&[tool]).unwrap();
        assert_eq!(compact.native_tools.len(), 1);
    }

    #[test]
    fn call_instructions_under_50_tokens_estimate() {
        // Very rough estimate: 1 token ≈ 4 chars. Instructions must be short.
        assert!(
            CALL_INSTRUCTIONS.len() < 50 * 5,
            "call instructions are too long: {} bytes",
            CALL_INSTRUCTIONS.len()
        );
    }
}

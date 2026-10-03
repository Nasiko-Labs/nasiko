//! Encoding: native [`ToolDef`]s → a [`CompactTools`] rendering.
//!
//! ## Supported JSON Schema features
//!
//! * `type`: `string`, `integer`, `number`, `boolean`, `array`, `object`.
//! * `string` with `"format":"date-time"` → `datetime`.
//! * `enum` (rendered inline as `a|b|c`; takes precedence over `type`).
//! * `array` `items` (rendered as `[elem]`, recursively).
//! * `required` (members without `?`; everything else gets `?`).
//! * `description` on the function (kept; property descriptions are dropped to save tokens —
//!   the name + type usually disambiguate, and the function description carries intent).
//!
//! ## Unsupported (caller should bypass compaction for tools using these)
//!
//! * `oneOf` / `anyOf` / `allOf` combinators, `$ref`, `additionalProperties` schemas.
//! * Nested object *shapes* (a nested object renders as the opaque `obj`; its inner
//!   properties are not expanded). Round-tripping via [`super::decode_tools`] therefore
//!   recovers `obj`, not the nested shape.
//!
//! These are reported by returning [`CompactError::InvalidSchema`] only when the schema is
//! actually malformed; well-formed-but-unsupported shapes degrade to `obj`/`any` so encoding
//! never silently corrupts a call — the decoder still validates against the *original* schema.

use serde_json::Value;

use crate::{CompactError, CompactTools, Result, ToolDef};

/// The fixed call-format instructions. Deliberately terse and stable: the model only needs the
/// marker shape and the "JSON object arguments / one marker per call / no marker when no tool
/// applies" rules. Changing this string changes what every model is asked to emit.
pub(crate) const CALL_INSTRUCTIONS: &str = "\
Call a tool by emitting this marker inline: <<call name {\"arg\": value}>>. \
Arguments are one JSON object; one marker per call; several markers allowed; required args \
first; emit no marker if no tool fits. Tools (name(args) - desc; ?=optional):";

/// Render a set of tools as a compact block plus call instructions.
///
/// Returns [`CompactError::InvalidSchema`] if a tool's `parameters` is present but is not a
/// JSON object, or its `properties`/`required` have the wrong JSON types — a malformed schema
/// must fail closed rather than render a misleading signature.
pub fn encode_tools(tools: &[ToolDef]) -> Result<CompactTools> {
    let mut lines = Vec::with_capacity(tools.len());
    for tool in tools {
        lines.push(render_tool(tool)?);
    }
    Ok(CompactTools {
        tools_block: lines.join("\n"),
        instructions: CALL_INSTRUCTIONS.to_string(),
    })
}

fn render_tool(tool: &ToolDef) -> Result<String> {
    let params = tool.parameters.as_ref();
    let args = match params {
        None | Some(Value::Null) => String::new(),
        Some(Value::Object(schema)) => render_params(&tool.name, schema)?,
        Some(_) => {
            return Err(CompactError::InvalidSchema {
                tool: tool.name.clone(),
                reason: "parameters must be a JSON Schema object".to_string(),
            });
        }
    };
    let mut line = format!("{}({})", tool.name, args);
    if let Some(desc) = &tool.description {
        let desc = desc.trim();
        if !desc.is_empty() {
            line.push_str(" - ");
            line.push_str(desc);
        }
    }
    Ok(line)
}

fn render_params(tool: &str, schema: &serde_json::Map<String, Value>) -> Result<String> {
    let props = match schema.get("properties") {
        None | Some(Value::Null) => return Ok(String::new()),
        Some(Value::Object(p)) => p,
        Some(_) => {
            return Err(CompactError::InvalidSchema {
                tool: tool.to_string(),
                reason: "`properties` must be an object".to_string(),
            });
        }
    };
    let required = required_set(tool, schema)?;

    let mut parts = Vec::with_capacity(props.len());
    for (name, prop) in props {
        let optional = !required.contains(name.as_str());
        let marker = if optional { "?" } else { "" };
        parts.push(format!("{name}{marker}:{}", type_str(prop)));
    }
    Ok(parts.join(", "))
}

/// The set of required property names, validated to be an array of strings.
fn required_set<'a>(
    tool: &str,
    schema: &'a serde_json::Map<String, Value>,
) -> Result<std::collections::HashSet<&'a str>> {
    match schema.get("required") {
        None | Some(Value::Null) => Ok(std::collections::HashSet::new()),
        Some(Value::Array(items)) => {
            let mut set = std::collections::HashSet::with_capacity(items.len());
            for item in items {
                let name = item.as_str().ok_or_else(|| CompactError::InvalidSchema {
                    tool: tool.to_string(),
                    reason: "`required` entries must be strings".to_string(),
                })?;
                set.insert(name);
            }
            Ok(set)
        }
        Some(_) => Err(CompactError::InvalidSchema {
            tool: tool.to_string(),
            reason: "`required` must be an array".to_string(),
        }),
    }
}

/// The compact type token for a single JSON Schema property node. `enum` wins over `type`;
/// unknown or absent types degrade to `any` (never an error — the decoder still validates
/// against the original schema, so a degraded render can't produce an invalid call).
pub(crate) fn type_str(prop: &Value) -> String {
    let Some(obj) = prop.as_object() else {
        return "any".to_string();
    };

    if let Some(Value::Array(variants)) = obj.get("enum") {
        let rendered: Vec<String> = variants.iter().map(render_enum_variant).collect();
        if !rendered.is_empty() {
            return rendered.join("|");
        }
    }

    match obj.get("type").and_then(Value::as_str) {
        Some("string") => match obj.get("format").and_then(Value::as_str) {
            Some("date-time") => "datetime".to_string(),
            _ => "str".to_string(),
        },
        Some("integer") => "int".to_string(),
        Some("number") => "float".to_string(),
        Some("boolean") => "bool".to_string(),
        Some("array") => {
            let elem = obj.get("items").map(type_str).unwrap_or_else(|| "any".to_string());
            format!("[{elem}]")
        }
        Some("object") => "obj".to_string(),
        _ => "any".to_string(),
    }
}

fn render_enum_variant(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

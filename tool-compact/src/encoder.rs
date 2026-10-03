//! Compact tool encoding and schema round-trip.

use std::collections::HashMap;

use serde_json::{Map, Value};

use crate::error::CompactError;
use crate::grammar::CALL_FORMAT;
use crate::names::{assign_aliases, is_valid_compact_ident};
use crate::types::{CompactTools, ToolCall, ToolDef};
use crate::validator::check_supported;

/// Encode tools into a compact prompt fragment.
///
/// Nasiko MCP gateway names (`{uuid_hex16}__{suffix}`) are aliased to a short
/// model-facing identifier; [`CompactTools::resolve_original_name`] restores the
/// exact original for execution. Non-gateway names are unchanged.
///
/// Returns [`CompactError::UnsupportedSchema`] when any tool uses a feature we cannot
/// preserve — callers must bypass compaction for that request (fail closed at the seam).
/// Returns [`CompactError::InvalidToolName`] when names cannot be safely aliased
/// (also a bypass signal at the router seam).
pub fn encode_tools(tools: &[ToolDef]) -> Result<CompactTools, CompactError> {
    if tools.is_empty() {
        return Ok(CompactTools {
            prompt: String::new(),
            tools: Vec::new(),
            original_by_alias: HashMap::new(),
        });
    }

    let originals: Vec<String> = tools.iter().map(|t| t.name.clone()).collect();
    let aliases = assign_aliases(&originals)?;

    let mut lines = Vec::with_capacity(tools.len() + 2);
    let mut kept = Vec::with_capacity(tools.len());
    let mut original_by_alias = HashMap::with_capacity(tools.len());

    for (tool, (compact_name, original_name)) in tools.iter().zip(aliases.into_iter()) {
        debug_assert_eq!(tool.name, original_name);
        check_supported(tool.parameters.as_ref())?;
        let mut aliased = tool.clone();
        aliased.name = compact_name.clone();
        lines.push(render_tool_line(&aliased)?);
        original_by_alias.insert(compact_name, original_name);
        kept.push(aliased);
    }

    lines.push(String::new());
    lines.push(CALL_FORMAT.to_string());

    Ok(CompactTools {
        prompt: lines.join("\n"),
        tools: kept,
        original_by_alias,
    })
}

/// Reconstruct [`ToolDef`]s from a [`CompactTools`] (schema meaning preserved).
pub fn decode_tools(compact: &CompactTools) -> Result<Vec<ToolDef>, CompactError> {
    Ok(compact.tools.clone())
}

/// Render known calls in the compact call grammar (for eval round-trips).
pub fn render_calls(calls: &[ToolCall]) -> Result<String, CompactError> {
    let mut out = Vec::with_capacity(calls.len());
    for call in calls {
        if !call.arguments.is_object() {
            return Err(CompactError::InvalidArguments);
        }
        let compact_args =
            serde_json::to_string(&call.arguments).map_err(|_| CompactError::InvalidArguments)?;
        out.push(format!("<<{} {}>>", call.name, compact_args));
    }
    Ok(out.join("\n"))
}

/// Build a [`ToolCall`] from name + JSON object.
pub fn tool_call(name: impl Into<String>, arguments: Value) -> ToolCall {
    ToolCall {
        name: name.into(),
        arguments,
    }
}

fn render_tool_line(tool: &ToolDef) -> Result<String, CompactError> {
    // Aliases are assigned before render; defend against programmer error.
    if !is_valid_compact_ident(&tool.name) {
        return Err(CompactError::InvalidToolName(tool.name.clone()));
    }
    let sig = render_signature(tool.parameters.as_ref())?;
    // Omit tool descriptions that only restate the tool name (paid on every request).
    // Keep descriptions that add meaning the name does not already convey.
    match tool.description.as_deref() {
        Some(raw)
            if !raw.trim().is_empty() && !description_redundant_with_name(&tool.name, raw) =>
        {
            let desc = shorten_chars(raw, 80);
            if desc.is_empty() {
                Ok(format!("{}({})", tool.name, sig))
            } else {
                Ok(format!("{}({}) - {}", tool.name, sig, desc))
            }
        }
        _ => Ok(format!("{}({})", tool.name, sig)),
    }
}

fn shorten_chars(s: &str, max: usize) -> String {
    let trimmed = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if trimmed.chars().count() <= max {
        return trimmed;
    }
    let mut out = String::new();
    for (i, ch) in trimmed.chars().enumerate() {
        if i + 1 >= max {
            out.push('…');
            break;
        }
        out.push(ch);
    }
    out
}

const DESC_STOP: &[&str] = &[
    "a", "an", "the", "of", "for", "in", "to", "from", "with", "and", "or", "on", "by", "at", "as",
    "is", "are", "user", "users", "account",
];

/// Words that restate the name/schema without adding domain meaning.
const DESC_RESTATEMENT: &[&str] = &[
    "line",
    "text",
    "plain",
    "plain-text",
    "plaintext",
    "event",
    "emails",
    "email",
    "address",
    "value",
    "field",
    "param",
    "parameter",
    "string",
    "number",
    "minutes",
    "minute",
    "time",
    "iso",
    "8601",
    "start",
    "end",
    "optional",
    "required",
    "list",
    "array",
    "object",
    "bool",
    "boolean",
    "true",
    "false",
    "name",
    "id",
    "identifier",
    "ids",
];

fn name_tokens(name: &str) -> Vec<String> {
    name.split('_')
        .filter(|s| !s.is_empty())
        .map(|s| s.to_ascii_lowercase())
        .collect()
}

fn desc_words(s: &str) -> Vec<String> {
    // "user's" → "users" after apostrophe strip; filtered via DESC_STOP.
    let norm = s.to_ascii_lowercase().replace('\'', "");
    norm.split(|c: char| !c.is_ascii_alphanumeric() && c != '-')
        .filter(|w| w.len() >= 2)
        .map(|w| w.to_string())
        .collect()
}

fn content_words(s: &str) -> Vec<String> {
    desc_words(s)
        .into_iter()
        .filter(|w| !DESC_STOP.contains(&w.as_str()))
        .collect()
}

/// True when every content word is already implied by the identifier (or is a
/// restatement filler). Used to drop redundant tool/property prose.
fn description_redundant_with_name(name: &str, desc: &str) -> bool {
    let ntoks = name_tokens(name);
    let content = content_words(desc);
    if content.is_empty() {
        return true;
    }
    content.iter().all(|w| {
        ntoks
            .iter()
            .any(|t| t == w || t.contains(w.as_str()) || w.contains(t.as_str()))
            || DESC_RESTATEMENT.contains(&w.as_str())
    })
}

/// Short disambiguation hint: content words not already present in the name.
fn short_prop_hint(name: &str, desc: &str) -> String {
    let ntoks = name_tokens(name);
    let mut kept: Vec<String> = Vec::new();
    for w in content_words(desc) {
        if ntoks.iter().any(|t| t == &w) {
            continue;
        }
        kept.push(w);
        if kept.len() >= 2 {
            break;
        }
    }
    if kept.is_empty() {
        return shorten_chars(desc, 16);
    }
    shorten_chars(&kept.join(" "), 24)
}

fn render_signature(parameters: Option<&Value>) -> Result<String, CompactError> {
    let Some(schema) = parameters else {
        return Ok(String::new());
    };
    let Value::Object(map) = schema else {
        return Err(CompactError::UnsupportedSchema(
            "parameters must be object schema".into(),
        ));
    };
    let props = map
        .get("properties")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let required: Vec<&str> = map
        .get("required")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();

    // Precompute types so we only emit property descriptions when siblings share
    // a type *and* the name alone is not enough to disambiguate.
    let mut typed: Vec<(String, bool, Value, String)> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for key in &required {
        if let Some(prop) = props.get(*key) {
            let ty = render_type(prop)?;
            typed.push(((*key).to_string(), true, prop.clone(), ty));
            seen.insert(*key);
        }
    }
    for (name, prop) in &props {
        if seen.contains(name.as_str()) {
            continue;
        }
        let ty = render_type(prop)?;
        typed.push((name.clone(), false, prop.clone(), ty));
    }
    let mut type_counts: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();
    for (_, _, _, ty) in &typed {
        *type_counts.entry(ty.clone()).or_insert(0) += 1;
    }

    let mut ordered = Vec::new();
    for (name, req, prop, ty) in &typed {
        let same_type = type_counts.get(ty).copied().unwrap_or(0) > 1;
        ordered.push(render_field(name, prop, *req, ty, same_type)?);
    }
    Ok(ordered.join(", "))
}

/// Required: `name:type!` · Optional: `name?:type`
///
/// Property hints are emitted only when same-type siblings exist **and** the
/// description adds meaning the property name does not already convey. Hints are
/// shortened to discriminative content words:
/// `recipient:str! "email address"`, `employee_id:str! "internal identifier"`.
fn render_field(
    name: &str,
    schema: &Value,
    required: bool,
    ty: &str,
    same_type: bool,
) -> Result<String, CompactError> {
    let mut base = if required {
        format!("{name}:{ty}!")
    } else {
        format!("{name}?:{ty}")
    };
    if same_type && let Some(hint) = property_desc_hint(name, schema) {
        base.push(' ');
        base.push_str(&hint);
    }
    Ok(base)
}

/// Compact JSON-quoted property description, or `None` when absent/redundant.
fn property_desc_hint(name: &str, schema: &Value) -> Option<String> {
    let raw = schema.get("description")?.as_str()?;
    if raw.trim().is_empty() || description_redundant_with_name(name, raw) {
        return None;
    }
    let short = short_prop_hint(name, raw);
    if short.is_empty() {
        return None;
    }
    serde_json::to_string(&short).ok()
}

fn render_type(schema: &Value) -> Result<String, CompactError> {
    let Value::Object(map) = schema else {
        return Err(CompactError::UnsupportedSchema(
            "property schema must be object".into(),
        ));
    };

    if let Some(enum_vals) = map.get("enum").and_then(Value::as_array) {
        let parts: Result<Vec<_>, _> = enum_vals.iter().map(render_enum_literal).collect();
        return Ok(parts?.join("|"));
    }

    let ty = map.get("type").and_then(Value::as_str).unwrap_or("object");

    match ty {
        "string" => {
            if map.get("format").and_then(Value::as_str) == Some("date-time") {
                Ok("datetime".into())
            } else {
                Ok("str".into())
            }
        }
        "integer" => Ok("int".into()),
        "number" => Ok("float".into()),
        "boolean" => Ok("bool".into()),
        "null" => Ok("null".into()),
        "array" => {
            let empty = Value::Object(Map::new());
            let items = map.get("items").unwrap_or(&empty);
            let inner = render_type(items)?;
            Ok(format!("[{inner}]"))
        }
        "object" => {
            if map.get("properties").and_then(Value::as_object).is_some() {
                // Reuse top-level signature rendering for nested objects (same
                // required/optional + same-type description rules).
                let sig = render_signature(Some(schema))?;
                Ok(format!("{{{sig}}}"))
            } else {
                Ok("object".into())
            }
        }
        other => Err(CompactError::UnsupportedSchema(format!("type {other}"))),
    }
}

fn render_enum_literal(v: &Value) -> Result<String, CompactError> {
    match v {
        Value::String(s) => {
            if s.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
            {
                Ok(s.clone())
            } else {
                Ok(serde_json::to_string(v)
                    .map_err(|_| CompactError::UnsupportedSchema("enum literal".into()))?)
            }
        }
        Value::Number(n) => Ok(n.to_string()),
        Value::Bool(b) => Ok(b.to_string()),
        Value::Null => Ok("null".into()),
        _ => Err(CompactError::UnsupportedSchema(
            "complex enum literal".into(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decode_calls;
    use serde_json::json;

    fn calendar() -> ToolDef {
        ToolDef {
            name: "create_calendar_event".into(),
            description: Some("Create an event in the user's calendar.".into()),
            parameters: Some(json!({
                "type": "object",
                "properties": {
                    "title": {"type": "string", "description": "Event title"},
                    "start": {"type": "string", "format": "date-time"},
                    "duration_min": {"type": "integer"},
                    "attendees": {"type": "array", "items": {"type": "string"}},
                    "visibility": {"type": "string", "enum": ["public", "private"]}
                },
                "required": ["title", "start"]
            })),
        }
    }

    #[test]
    fn encode_marks_required_with_bang() {
        let c = encode_tools(&[calendar()]).unwrap();
        assert!(
            c.prompt.contains("title:str!") && c.prompt.contains("start:datetime!"),
            "prompt was: {}",
            c.prompt
        );
        assert!(c.prompt.contains("visibility?:public|private"));
        assert!(c.prompt.contains("CALL <<name {json}>>"));
        // Tool name already conveys "create calendar event" — description omitted.
        assert!(
            !c.prompt.contains("Create an event"),
            "redundant tool description should be omitted; prompt={}",
            c.prompt
        );
    }

    #[test]
    fn decode_tools_roundtrip() {
        let tools = vec![calendar()];
        let c = encode_tools(&tools).unwrap();
        assert_eq!(decode_tools(&c).unwrap(), tools);
    }

    #[test]
    fn keeps_tool_description_when_name_is_opaque() {
        let tool = ToolDef {
            name: "ping".into(),
            description: Some("Check HTTP health of the payment gateway.".into()),
            parameters: Some(json!({"type": "object", "properties": {}})),
        };
        let prompt = encode_tools(&[tool]).unwrap().prompt;
        assert!(
            prompt.contains("ping()")
                && prompt.contains("Check HTTP health of the payment gateway."),
            "non-redundant tool description must be kept; prompt={prompt}"
        );
    }

    #[test]
    fn gateway_uuid_prefix_aliases_to_suffix() {
        let tool = ToolDef {
            name: "1234567890abcdef__create_calendar_event".into(),
            description: Some("Create an event in the user's calendar.".into()),
            parameters: Some(json!({
                "type": "object",
                "properties": {
                    "title": {"type": "string"},
                    "start": {"type": "string", "format": "date-time"}
                },
                "required": ["title", "start"]
            })),
        };
        let c = encode_tools(&[tool]).unwrap();
        assert_eq!(c.tools()[0].name, "create_calendar_event");
        assert_eq!(
            c.resolve_original_name("create_calendar_event"),
            Some("1234567890abcdef__create_calendar_event")
        );
        assert!(c.prompt.contains("create_calendar_event("));
        assert!(!c.prompt.contains("1234567890abcdef__"));
    }

    #[test]
    fn gateway_roundtrip_alias_to_original() {
        let original = "1234567890abcdef__create_calendar_event";
        let tools = vec![ToolDef {
            name: original.into(),
            description: None,
            parameters: Some(json!({
                "type": "object",
                "properties": {
                    "title": {"type": "string"}
                },
                "required": ["title"]
            })),
        }];
        let c = encode_tools(&tools).unwrap();
        assert_eq!(c.tools()[0].name, "create_calendar_event");
        let calls = decode_calls(
            r#"<<create_calendar_event {"title":"Meeting"}>>"#,
            c.tools(),
        )
        .unwrap();
        assert_eq!(calls[0].name, "create_calendar_event");
        assert_eq!(c.resolve_original_name(&calls[0].name), Some(original));
    }

    #[test]
    fn gateway_collision_distinct_aliases() {
        let tools = vec![
            ToolDef {
                name: "1111111111111111__search".into(),
                description: None,
                parameters: Some(json!({"type": "object", "properties": {}})),
            },
            ToolDef {
                name: "2222222222222222__search".into(),
                description: None,
                parameters: Some(json!({"type": "object", "properties": {}})),
            },
        ];
        let c = encode_tools(&tools).unwrap();
        assert_eq!(c.tools()[0].name, "search_1111111111111111");
        assert_eq!(c.tools()[1].name, "search_2222222222222222");
        assert_ne!(c.tools()[0].name, c.tools()[1].name);
        assert_eq!(
            c.resolve_original_name("search_1111111111111111"),
            Some("1111111111111111__search")
        );
        assert_eq!(
            c.resolve_original_name("search_2222222222222222"),
            Some("2222222222222222__search")
        );
    }

    #[test]
    fn short_hex_prefix_rejected() {
        let tool = ToolDef {
            name: "12345678__create_calendar_event".into(),
            description: None,
            parameters: Some(json!({"type": "object", "properties": {}})),
        };
        assert!(matches!(
            encode_tools(&[tool]),
            Err(CompactError::InvalidToolName(_))
        ));
    }

    #[test]
    fn digit_leading_non_gateway_rejected() {
        let tool = ToolDef {
            name: "9bad_tool".into(),
            description: None,
            parameters: Some(json!({"type": "object", "properties": {}})),
        };
        assert!(matches!(
            encode_tools(&[tool]),
            Err(CompactError::InvalidToolName(_))
        ));
    }

    #[test]
    fn unknown_alias_resolve_returns_none() {
        let c = encode_tools(&[calendar()]).unwrap();
        assert!(c.resolve_original_name("nope").is_none());
    }
}

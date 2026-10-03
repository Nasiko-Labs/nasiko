//! Encoder: JSON-Schema tool definitions -> compact signatures.

use std::collections::HashSet;

use serde_json::{Map, Value};

use crate::{is_valid_tool_name, EncodeError, ToolDef};

const INSTRUCTIONS: &str = "To call a tool, emit: <<call NAME {json args}>>\n\
Args: one JSON object. `?` = optional param. Types: str int num bool null datetime date obj, [T]=array, {..}=object, a|b=enum.\n\
Multiple calls allowed (one <<call ...>> each). If no tool is needed, answer normally.";

const RESERVED: [&str; 10] = [
    "str", "int", "num", "bool", "null", "datetime", "date", "obj", "true", "false",
];

const ALLOWED_KEYWORDS: [&str; 9] = [
    "type",
    "description",
    "properties",
    "required",
    "items",
    "enum",
    "format",
    "additionalProperties",
    "title",
];

/// Encoding options.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodeOptions {
    /// Emit per-parameter descriptions below each signature (default: true).
    pub param_descriptions: bool,
}

impl Default for EncodeOptions {
    fn default() -> Self {
        Self {
            param_descriptions: true,
        }
    }
}

/// Compact tool text to inject into a prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactTools {
    /// One signature (plus optional description lines) per tool.
    pub definitions: String,
    /// Call-format instructions.
    pub instructions: String,
}

impl CompactTools {
    /// Definitions plus instructions, ready to place in a system message.
    pub fn system_prompt(&self) -> String {
        format!(
            "Available tools:\n{}\n\n{}",
            self.definitions, self.instructions
        )
    }
}

/// Encode tools with default options.
pub fn encode_tools(tools: &[ToolDef]) -> Result<CompactTools, EncodeError> {
    encode_tools_with(tools, &EncodeOptions::default())
}

/// Encode tools. Any unsupported or malformed schema is an error; callers should
/// then send the native tool definitions instead.
pub fn encode_tools_with(
    tools: &[ToolDef],
    opts: &EncodeOptions,
) -> Result<CompactTools, EncodeError> {
    let mut seen = HashSet::new();
    let mut blocks = Vec::with_capacity(tools.len());
    for tool in tools {
        if !is_valid_tool_name(&tool.name) {
            return Err(EncodeError::InvalidToolName(tool.name.clone()));
        }
        if !seen.insert(tool.name.as_str()) {
            return Err(EncodeError::DuplicateTool(tool.name.clone()));
        }
        blocks.push(encode_one(tool, opts)?);
    }
    Ok(CompactTools {
        definitions: blocks.join("\n"),
        instructions: INSTRUCTIONS.to_string(),
    })
}

fn encode_one(tool: &ToolDef, opts: &EncodeOptions) -> Result<String, EncodeError> {
    let mut signature = String::new();
    let mut described: Vec<(String, String)> = Vec::new();
    if let Some(schema) = &tool.parameters {
        let obj = schema
            .as_object()
            .ok_or_else(|| malformed(&tool.name, "parameters must be a JSON object"))?;
        check_keywords(obj, &tool.name)?;
        match obj.get("type").and_then(Value::as_str) {
            Some("object") | None => {}
            Some(_) => return Err(malformed(&tool.name, "parameters must have type `object`")),
        }
        if obj.get("additionalProperties") == Some(&Value::Bool(true)) {
            return Err(unsupported(&tool.name, "free-form top-level arguments"));
        }
        signature = render_params(obj, &tool.name)?;
        if opts.param_descriptions {
            collect_descriptions(obj, "", &mut described);
        }
    }
    let mut out = format!("{}({})", tool.name, signature);
    if let Some(desc) = tool
        .description
        .as_deref()
        .map(collapse)
        .filter(|d| !d.is_empty())
    {
        out.push_str(" - ");
        out.push_str(&desc);
    }
    for (path, desc) in described {
        out.push_str(&format!("\n  {path}: {desc}"));
    }
    Ok(out)
}

fn render_params(obj: &Map<String, Value>, path: &str) -> Result<String, EncodeError> {
    let props = match obj.get("properties") {
        None => return Ok(String::new()),
        Some(p) => p
            .as_object()
            .ok_or_else(|| malformed(path, "`properties` must be an object"))?,
    };
    let required = parse_required(obj, props, path)?;
    let mut parts = Vec::with_capacity(props.len());
    for name in &required {
        parts.push(render_param(name, &props[*name], true, path)?);
    }
    for (name, schema) in props {
        if !required.contains(&name.as_str()) {
            parts.push(render_param(name, schema, false, path)?);
        }
    }
    Ok(parts.join(", "))
}

fn render_param(
    name: &str,
    schema: &Value,
    required: bool,
    path: &str,
) -> Result<String, EncodeError> {
    if !is_valid_param_name(name) {
        return Err(unsupported(path, &format!("parameter name `{name}`")));
    }
    let ty = render_type(schema, &format!("{path}.{name}"))?;
    Ok(format!("{name}{}:{ty}", if required { "" } else { "?" }))
}

fn parse_required<'a>(
    obj: &'a Map<String, Value>,
    props: &Map<String, Value>,
    path: &str,
) -> Result<Vec<&'a str>, EncodeError> {
    let mut list: Vec<&str> = Vec::new();
    if let Some(value) = obj.get("required") {
        let arr = value
            .as_array()
            .ok_or_else(|| malformed(path, "`required` must be an array"))?;
        for item in arr {
            let name = item
                .as_str()
                .ok_or_else(|| malformed(path, "`required` entries must be strings"))?;
            if !props.contains_key(name) {
                return Err(malformed(
                    path,
                    &format!("required field `{name}` is not in properties"),
                ));
            }
            if !list.contains(&name) {
                list.push(name);
            }
        }
    }
    Ok(list)
}

fn render_type(schema: &Value, path: &str) -> Result<String, EncodeError> {
    let obj = schema
        .as_object()
        .ok_or_else(|| malformed(path, "schema must be an object"))?;
    check_keywords(obj, path)?;
    if let Some(values) = obj.get("enum") {
        let arr = values
            .as_array()
            .filter(|a| !a.is_empty())
            .ok_or_else(|| malformed(path, "`enum` must be a non-empty array"))?;
        let rendered = arr
            .iter()
            .map(|v| render_enum_value(v, path))
            .collect::<Result<Vec<_>, _>>()?;
        return Ok(rendered.join("|"));
    }
    let ty = obj
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| unsupported(path, "missing or non-string `type`"))?;
    match ty {
        "string" => match obj.get("format").and_then(Value::as_str) {
            None => Ok("str".into()),
            Some("date-time") => Ok("datetime".into()),
            Some("date") => Ok("date".into()),
            Some(other) => Err(unsupported(path, &format!("format `{other}`"))),
        },
        "integer" => Ok("int".into()),
        "number" => Ok("num".into()),
        "boolean" => Ok("bool".into()),
        "null" => Ok("null".into()),
        "array" => {
            let items = obj
                .get("items")
                .ok_or_else(|| unsupported(path, "array without `items`"))?;
            Ok(format!("[{}]", render_type(items, &format!("{path}[]"))?))
        }
        "object" => {
            if obj.contains_key("properties") {
                Ok(format!("{{{}}}", render_params(obj, path)?))
            } else {
                Ok("obj".into())
            }
        }
        other => Err(unsupported(path, &format!("type `{other}`"))),
    }
}

fn render_enum_value(v: &Value, path: &str) -> Result<String, EncodeError> {
    match v {
        Value::String(s) if is_bare_word(s) => Ok(s.clone()),
        Value::String(_) | Value::Number(_) | Value::Bool(_) | Value::Null => Ok(v.to_string()),
        _ => Err(unsupported(path, "enum values must be scalars")),
    }
}

fn is_bare_word(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(crate::is_name_char) && !RESERVED.contains(&s)
}

fn is_valid_param_name(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn check_keywords(obj: &Map<String, Value>, path: &str) -> Result<(), EncodeError> {
    for key in obj.keys() {
        if !ALLOWED_KEYWORDS.contains(&key.as_str()) && key != "$schema" {
            return Err(unsupported(path, &format!("keyword `{key}`")));
        }
    }
    match obj.get("additionalProperties") {
        None | Some(Value::Bool(false)) => Ok(()),
        Some(Value::Bool(true)) if !obj.contains_key("properties") => Ok(()),
        Some(_) => Err(unsupported(path, "`additionalProperties` other than false")),
    }
}

fn collect_descriptions(obj: &Map<String, Value>, prefix: &str, out: &mut Vec<(String, String)>) {
    let Some(props) = obj.get("properties").and_then(Value::as_object) else {
        return;
    };
    for (name, schema) in props {
        let path = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}.{name}")
        };
        walk_description(schema, &path, out);
    }
}

fn walk_description(schema: &Value, path: &str, out: &mut Vec<(String, String)>) {
    let Some(obj) = schema.as_object() else {
        return;
    };
    if let Some(desc) = obj.get("description").and_then(Value::as_str).map(collapse) {
        if !desc.is_empty() {
            out.push((path.to_string(), desc));
        }
    }
    match obj.get("type").and_then(Value::as_str) {
        Some("object") => collect_descriptions(obj, path, out),
        Some("array") => {
            if let Some(items) = obj.get("items") {
                walk_description(items, &format!("{path}[]"), out);
            }
        }
        _ => {}
    }
}

fn collapse(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn malformed(path: &str, reason: &str) -> EncodeError {
    EncodeError::MalformedSchema {
        path: path.to_string(),
        reason: reason.to_string(),
    }
}

fn unsupported(path: &str, reason: &str) -> EncodeError {
    EncodeError::Unsupported {
        path: path.to_string(),
        reason: reason.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::tools;
    use serde_json::json;

    fn tool(name: &str, desc: Option<&str>, params: Option<Value>) -> ToolDef {
        ToolDef {
            name: name.into(),
            description: desc.map(String::from),
            parameters: params,
        }
    }

    fn enc(t: ToolDef) -> Result<String, EncodeError> {
        encode_tools(&[t]).map(|c| c.definitions)
    }

    #[test]
    fn calendar_example() {
        let out = encode_tools(&tools()).unwrap().definitions;
        assert!(out.starts_with("create_calendar_event(title:str, start:datetime, "));
        for part in [
            "attendees?:[str]",
            "duration_min?:int",
            "visibility?:public|private",
        ] {
            assert!(out.contains(part), "{part} missing in {out}");
        }
        assert!(out.contains(") - Create an event in the user's calendar."));
        assert!(out.contains("\n  start: Start time, ISO 8601"));
        assert!(out.contains("\nping()"));
    }

    #[test]
    fn no_args_and_empty_properties() {
        assert_eq!(enc(tool("ping", None, None)).unwrap(), "ping()");
        let p = json!({"type":"object","properties":{}});
        assert_eq!(
            enc(tool("ping", Some("Check."), Some(p))).unwrap(),
            "ping() - Check."
        );
    }

    #[test]
    fn nested_objects_and_arrays() {
        let p = json!({"type":"object","properties":{
            "address":{"type":"object","properties":{"street":{"type":"string"},"zip":{"type":"string"}},"required":["street"]},
            "rows":{"type":"array","items":{"type":"object","properties":{"id":{"type":"integer"}},"required":["id"]}},
            "grid":{"type":"array","items":{"type":"array","items":{"type":"number"}}},
            "meta":{"type":"object"},
            "ok":{"type":"boolean"}
        },"required":["address"]});
        let out = enc(tool("t", None, Some(p))).unwrap();
        assert!(out.contains("address:{street:str, zip?:str}"), "{out}");
        assert!(out.contains("rows?:[{id:int}]"), "{out}");
        assert!(out.contains("grid?:[[num]]"), "{out}");
        assert!(out.contains("meta?:obj"), "{out}");
        assert!(out.contains("ok?:bool"), "{out}");
    }

    #[test]
    fn enum_quoting() {
        let p = json!({"type":"object","properties":{
            "a":{"enum":["str","a b","x"]},
            "n":{"enum":[1,2]}
        },"required":["a","n"]});
        let out = enc(tool("t", None, Some(p))).unwrap();
        assert!(out.contains("a:\"str\"|\"a b\"|x"), "{out}");
        assert!(out.contains("n:1|2"), "{out}");
    }

    #[test]
    fn nested_descriptions_are_kept() {
        let p = json!({"type":"object","properties":{
            "to":{"type":"array","items":{"type":"string","description":"email"},"description":"Recipients"}
        }});
        let out = enc(tool("t", None, Some(p))).unwrap();
        assert!(out.contains("\n  to: Recipients"));
        assert!(out.contains("\n  to[]: email"));
    }

    #[test]
    fn descriptions_can_be_disabled_and_whitespace_collapsed() {
        let opts = EncodeOptions {
            param_descriptions: false,
        };
        let t = tool("t", Some("a\n  b"), tools()[0].parameters.clone());
        let out = encode_tools_with(&[t], &opts).unwrap().definitions;
        assert!(out.ends_with(") - a b"), "{out}");
        assert!(!out.contains("\n  "));
    }

    #[test]
    fn rejects_bad_input() {
        assert!(matches!(
            enc(tool("bad name", None, None)),
            Err(EncodeError::InvalidToolName(_))
        ));
        let dup = [tool("a", None, None), tool("a", None, None)];
        assert!(matches!(
            encode_tools(&dup),
            Err(EncodeError::DuplicateTool(_))
        ));
        assert!(matches!(
            enc(tool("t", None, Some(json!("x")))),
            Err(EncodeError::MalformedSchema { .. })
        ));
        let missing =
            json!({"type":"object","properties":{"a":{"type":"string"}},"required":["b"]});
        assert!(matches!(
            enc(tool("t", None, Some(missing))),
            Err(EncodeError::MalformedSchema { .. })
        ));
    }

    #[test]
    fn unsupported_features_are_reported_not_dropped() {
        let cases = [
            json!({"type":"object","properties":{"a":{"anyOf":[{"type":"string"},{"type":"integer"}]}}}),
            json!({"type":"object","properties":{"a":{"type":"string","pattern":"^a"}}}),
            json!({"type":"object","properties":{"a":{"type":"integer","minimum":1}}}),
            json!({"type":"object","properties":{"a":{"type":"string","format":"email"}}}),
            json!({"type":"object","properties":{"a":{"type":["string","null"]}}}),
            json!({"type":"object","properties":{"a":{"type":"array"}}}),
            json!({"type":"object","properties":{"a":{"type":"string","default":"x"}}}),
            json!({"type":"object","additionalProperties":true}),
        ];
        for p in cases {
            assert!(
                matches!(
                    enc(tool("t", None, Some(p.clone()))),
                    Err(EncodeError::Unsupported { .. })
                ),
                "{p}"
            );
        }
    }
}

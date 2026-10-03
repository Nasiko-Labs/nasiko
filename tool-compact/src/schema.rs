//! The supported JSON Schema subset, its compact rendering, and validation.
//!
//! Supported: `type` string/integer/number/boolean/array/object (with `format:
//! date-time|date` on strings), string `enum`s, nested objects and arrays, `required`.
//! Anything else (`$ref`, `oneOf`/`anyOf`/`allOf`/`not`, `if`/`then`, non-string enums,
//! `patternProperties`, nesting deeper than [`MAX_DEPTH`]) is rejected so the caller
//! bypasses the tool instead of misrepresenting its schema.

use serde_json::Value;

use crate::types::{Error, ToolDef};

/// Maximum nesting depth of objects/arrays inside one signature.
pub const MAX_DEPTH: usize = 4;

/// The compact type model: everything a signature line can express.
#[derive(Debug, Clone, PartialEq)]
pub enum CType {
    Str,
    Int,
    Num,
    Bool,
    DateTime,
    Date,
    Array(Box<CType>),
    Object(Vec<Field>),
    Enum(Vec<String>),
    /// `{}` / `true`: unconstrained value.
    Any,
}

/// One object field: name, type, and whether it is required.
#[derive(Debug, Clone, PartialEq)]
pub struct Field {
    pub name: String,
    pub ty: CType,
    pub required: bool,
}

/// A tool's argument schema in compact form.
#[derive(Debug, Clone, PartialEq)]
pub struct CompactSchema {
    pub fields: Vec<Field>,
}

impl CompactSchema {
    /// Convert a JSON Schema object into the compact model.
    pub fn from_json_schema(tool: &str, schema: &Value) -> Result<Self, Error> {
        let ty = ctype_of(tool, schema, 0)?;
        match ty {
            CType::Object(fields) => Ok(Self { fields }),
            CType::Any => Ok(Self { fields: Vec::new() }),
            other => Err(Error::UnsupportedSchema {
                tool: tool.to_string(),
                reason: format!("top-level parameters schema must be an object, got {other:?}"),
            }),
        }
    }

    /// Validate a decoded arguments object against this schema. Extra properties are
    /// allowed; missing required fields, wrong types and bad enum values fail.
    pub fn validate(&self, tool: &str, args: &Value) -> Result<(), Error> {
        let obj = args.as_object().ok_or_else(|| Error::InvalidArguments {
            tool: tool.to_string(),
            reason: "arguments must be a JSON object".to_string(),
        })?;
        for f in &self.fields {
            match obj.get(&f.name) {
                None if f.required => {
                    return Err(Error::InvalidArguments {
                        tool: tool.to_string(),
                        reason: format!("missing required field {:?}", f.name),
                    });
                }
                None => {}
                Some(v) => validate_value(tool, &f.name, v, &f.ty)?,
            }
        }
        Ok(())
    }

    /// Render the parameter list: `title:str, start:datetime, attendees?:[str]`.
    pub fn render_params(&self) -> String {
        self.fields
            .iter()
            .map(|f| {
                if f.required {
                    format!("{}:{}", f.name, render_type(&f.ty))
                } else {
                    format!("{}?:{}", f.name, render_type(&f.ty))
                }
            })
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// Rebuild a JSON Schema object from the compact model (used by `decode_tools`).
    pub fn to_json_schema(&self) -> Value {
        let mut properties = serde_json::Map::new();
        let mut required = Vec::new();
        for f in &self.fields {
            properties.insert(f.name.clone(), ctype_to_schema(&f.ty));
            if f.required {
                required.push(Value::String(f.name.clone()));
            }
        }
        let mut obj = serde_json::Map::new();
        obj.insert("type".to_string(), Value::String("object".to_string()));
        obj.insert("properties".to_string(), Value::Object(properties));
        if !required.is_empty() {
            obj.insert("required".to_string(), Value::Array(required));
        }
        Value::Object(obj)
    }
}

fn unsupported(tool: &str, reason: impl Into<String>) -> Error {
    Error::UnsupportedSchema {
        tool: tool.to_string(),
        reason: reason.into(),
    }
}

/// Keys that take a schema outside the supported subset.
const BANNED_KEYS: &[&str] = &[
    "$ref",
    "$defs",
    "definitions",
    "oneOf",
    "anyOf",
    "allOf",
    "not",
    "if",
    "then",
    "else",
    "patternProperties",
    "propertyNames",
    "contains",
    "prefixItems",
];

fn ctype_of(tool: &str, schema: &Value, depth: usize) -> Result<CType, Error> {
    if depth > MAX_DEPTH {
        return Err(unsupported(tool, "schema nesting exceeds supported depth"));
    }
    // Boolean schemas: `true` means anything goes.
    if let Some(b) = schema.as_bool() {
        return if b {
            Ok(CType::Any)
        } else {
            Err(unsupported(tool, "schema `false` matches nothing"))
        };
    }
    let obj = schema
        .as_object()
        .ok_or_else(|| unsupported(tool, "schema must be an object"))?;
    for key in BANNED_KEYS {
        if obj.contains_key(*key) {
            return Err(unsupported(
                tool,
                format!("unsupported schema keyword `{key}`"),
            ));
        }
    }
    // String enums (possibly with a sibling "type").
    if let Some(e) = obj.get("enum") {
        let arr = e
            .as_array()
            .ok_or_else(|| unsupported(tool, "`enum` must be an array"))?;
        let mut values = Vec::with_capacity(arr.len());
        for v in arr {
            let s = v
                .as_str()
                .ok_or_else(|| unsupported(tool, "only string enums are supported"))?;
            if !is_plain_enum_value(s) {
                return Err(unsupported(
                    tool,
                    "enum value contains characters the compact grammar cannot express",
                ));
            }
            values.push(s.to_string());
        }
        if values.is_empty() {
            return Err(unsupported(tool, "empty `enum`"));
        }
        return Ok(CType::Enum(values));
    }

    let ty = obj.get("type").and_then(Value::as_str).unwrap_or("");
    match ty {
        "string" => match obj.get("format").and_then(Value::as_str) {
            Some("date-time") => Ok(CType::DateTime),
            Some("date") => Ok(CType::Date),
            Some(_) => Ok(CType::Str), // other formats: keep as plain string
            None => Ok(CType::Str),
        },
        "integer" => Ok(CType::Int),
        "number" => Ok(CType::Num),
        "boolean" => Ok(CType::Bool),
        "array" => {
            let items = obj.get("items").unwrap_or(&Value::Bool(true));
            Ok(CType::Array(Box::new(ctype_of(tool, items, depth + 1)?)))
        }
        "object" | "" => {
            // No "type" with "properties" is treated as an object (lenient input).
            let empty = serde_json::Map::new();
            let props = obj
                .get("properties")
                .and_then(Value::as_object)
                .unwrap_or(&empty);
            let required: Vec<&str> = obj
                .get("required")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            if obj
                .get("additionalProperties")
                .is_some_and(|v| v == &Value::Bool(false))
            {
                // Fine — we simply don't emit unknown fields; decoding allows extras.
            }
            let mut fields = Vec::with_capacity(props.len());
            for (name, sub) in props {
                if !is_plain_name(name) {
                    return Err(unsupported(
                        tool,
                        format!(
                            "parameter name {name:?} cannot be expressed in the compact grammar"
                        ),
                    ));
                }
                fields.push(Field {
                    name: name.clone(),
                    ty: ctype_of(tool, sub, depth + 1)?,
                    required: required.contains(&name.as_str()),
                });
            }
            Ok(CType::Object(fields))
        }
        other => Err(unsupported(tool, format!("unsupported type `{other}`"))),
    }
}

/// Enum values must survive the `a|b|c` rendering: no grammar metacharacters.
fn is_plain_enum_value(s: &str) -> bool {
    !s.is_empty()
        && !s.chars().any(|c| {
            matches!(
                c,
                '|' | ',' | '(' | ')' | '[' | ']' | '{' | '}' | '?' | '\n' | '\r'
            )
        })
}

/// Parameter (and tool) names: no whitespace or grammar metacharacters.
fn is_plain_name(s: &str) -> bool {
    !s.is_empty()
        && !s.chars().any(|c| {
            c.is_whitespace()
                || matches!(
                    c,
                    '(' | ')' | '[' | ']' | '{' | '}' | ',' | ':' | '?' | '|' | '>'
                )
        })
}

pub fn is_plain_tool_name(s: &str) -> bool {
    is_plain_name(s) && !s.contains('<')
}

fn render_type(ty: &CType) -> String {
    match ty {
        CType::Str => "str".to_string(),
        CType::Int => "int".to_string(),
        CType::Num => "num".to_string(),
        CType::Bool => "bool".to_string(),
        CType::DateTime => "datetime".to_string(),
        CType::Date => "date".to_string(),
        CType::Array(inner) => format!("[{}]", render_type(inner)),
        CType::Object(fields) => {
            let inner = fields
                .iter()
                .map(|f| {
                    if f.required {
                        format!("{}:{}", f.name, render_type(&f.ty))
                    } else {
                        format!("{}?:{}", f.name, render_type(&f.ty))
                    }
                })
                .collect::<Vec<_>>()
                .join(", ");
            format!("{{{inner}}}")
        }
        CType::Enum(values) => values.join("|"),
        CType::Any => "any".to_string(),
    }
}

fn ctype_to_schema(ty: &CType) -> Value {
    let mut o = serde_json::Map::new();
    match ty {
        CType::Str => {
            o.insert("type".to_string(), Value::String("string".to_string()));
        }
        CType::DateTime => {
            o.insert("type".to_string(), Value::String("string".to_string()));
            o.insert("format".to_string(), Value::String("date-time".to_string()));
        }
        CType::Date => {
            o.insert("type".to_string(), Value::String("string".to_string()));
            o.insert("format".to_string(), Value::String("date".to_string()));
        }
        CType::Int => {
            o.insert("type".to_string(), Value::String("integer".to_string()));
        }
        CType::Num => {
            o.insert("type".to_string(), Value::String("number".to_string()));
        }
        CType::Bool => {
            o.insert("type".to_string(), Value::String("boolean".to_string()));
        }
        CType::Array(inner) => {
            o.insert("type".to_string(), Value::String("array".to_string()));
            o.insert("items".to_string(), ctype_to_schema(inner));
        }
        CType::Object(fields) => {
            let mut props = serde_json::Map::new();
            let mut required = Vec::new();
            for f in fields {
                props.insert(f.name.clone(), ctype_to_schema(&f.ty));
                if f.required {
                    required.push(Value::String(f.name.clone()));
                }
            }
            o.insert("type".to_string(), Value::String("object".to_string()));
            o.insert("properties".to_string(), Value::Object(props));
            if !required.is_empty() {
                o.insert("required".to_string(), Value::Array(required));
            }
        }
        CType::Enum(values) => {
            o.insert("type".to_string(), Value::String("string".to_string()));
            o.insert(
                "enum".to_string(),
                Value::Array(values.iter().map(|v| Value::String(v.clone())).collect()),
            );
        }
        CType::Any => {
            // Unconstrained: empty schema.
        }
    }
    Value::Object(o)
}

fn invalid(tool: &str, path: &str, reason: impl Into<String>) -> Error {
    Error::InvalidArguments {
        tool: tool.to_string(),
        reason: format!("{path}: {}", reason.into()),
    }
}

fn validate_value(tool: &str, path: &str, value: &Value, ty: &CType) -> Result<(), Error> {
    match ty {
        CType::Str | CType::DateTime | CType::Date => {
            if value.is_string() {
                Ok(())
            } else {
                Err(invalid(tool, path, "expected a string"))
            }
        }
        CType::Int => {
            if value.is_i64() || value.is_u64() {
                Ok(())
            } else {
                Err(invalid(tool, path, "expected an integer"))
            }
        }
        CType::Num => {
            if value.is_number() {
                Ok(())
            } else {
                Err(invalid(tool, path, "expected a number"))
            }
        }
        CType::Bool => {
            if value.is_boolean() {
                Ok(())
            } else {
                Err(invalid(tool, path, "expected a boolean"))
            }
        }
        CType::Enum(values) => match value.as_str() {
            Some(s) if values.iter().any(|v| v == s) => Ok(()),
            _ => Err(invalid(
                tool,
                path,
                format!("expected one of {}", values.join("|")),
            )),
        },
        CType::Array(inner) => {
            let arr = value
                .as_array()
                .ok_or_else(|| invalid(tool, path, "expected an array"))?;
            for (i, v) in arr.iter().enumerate() {
                validate_value(tool, &format!("{path}[{i}]"), v, inner)?;
            }
            Ok(())
        }
        CType::Object(fields) => {
            let obj = value
                .as_object()
                .ok_or_else(|| invalid(tool, path, "expected an object"))?;
            for f in fields {
                match obj.get(&f.name) {
                    None if f.required => {
                        return Err(invalid(
                            tool,
                            path,
                            format!("missing required field {:?}", f.name),
                        ));
                    }
                    None => {}
                    Some(v) => validate_value(tool, &format!("{}.{}", path, f.name), v, &f.ty)?,
                }
            }
            Ok(())
        }
        CType::Any => Ok(()),
    }
}

// ─── decode_tools: signature lines back to ToolDef ────────────────────────────

/// Parse compact signature lines (as produced by [`crate::encode_tools`]) back into
/// tool definitions. Used for schema round-trip checks: the decoded schemas must
/// accept exactly the same arguments as the originals.
pub fn decode_tools(compact: &crate::types::CompactTools) -> Result<Vec<ToolDef>, Error> {
    let mut out = Vec::new();
    for (i, line) in compact.rendered.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        out.push(
            parse_signature_line(line).map_err(|reason| {
                Error::Malformed(format!("signature line {}: {reason}", i + 1))
            })?,
        );
    }
    Ok(out)
}

fn parse_signature_line(line: &str) -> Result<ToolDef, String> {
    // name(params) - description
    let open = line.find('(').ok_or("missing '('")?;
    let name = line[..open].trim().to_string();
    if !is_plain_tool_name(&name) {
        return Err(format!("bad tool name {name:?}"));
    }
    let rest = &line[open..];
    let close = find_matching_paren(rest).ok_or("unbalanced parentheses")?;
    let params_src = &rest[1..close];
    let after = rest[close + 1..].trim_start();
    let description = after
        .strip_prefix('-')
        .map(|d| d.trim().to_string())
        .filter(|d| !d.is_empty());

    let fields = if params_src.trim() == "?" {
        // Bypassed tool: name(?) — schema unknown, keep parameters absent.
        return Ok(ToolDef::new(name, description, None));
    } else {
        parse_params(params_src)?
    };
    let schema = CompactSchema { fields }.to_json_schema();
    Ok(ToolDef::new(name, description, Some(schema)))
}

fn find_matching_paren(s: &str) -> Option<usize> {
    let mut depth = 0usize;
    for (i, c) in s.char_indices() {
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

/// Split on top-level commas (depth counts all bracket kinds).
fn split_top_level(s: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut depth = 0usize;
    let mut start = 0usize;
    for (i, c) in s.char_indices() {
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                parts.push(s[start..i].trim());
                start = i + 1;
            }
            _ => {}
        }
    }
    let last = s[start..].trim();
    if !last.is_empty() || !parts.is_empty() {
        parts.push(last);
    }
    parts.into_iter().filter(|p| !p.is_empty()).collect()
}

fn parse_params(s: &str) -> Result<Vec<Field>, String> {
    let mut fields = Vec::new();
    for part in split_top_level(s) {
        let colon = part
            .find(':')
            .ok_or(format!("param {part:?} missing ':'"))?;
        let (name, ty) = part.split_at(colon);
        let ty = &ty[1..];
        let (name, required) = match name.strip_suffix('?') {
            Some(n) => (n, false),
            None => (name, true),
        };
        if !is_plain_name(name) {
            return Err(format!("bad parameter name {name:?}"));
        }
        fields.push(Field {
            name: name.to_string(),
            ty: parse_type(ty)?,
            required,
        });
    }
    Ok(fields)
}

fn parse_type(s: &str) -> Result<CType, String> {
    let s = s.trim();
    match s {
        "str" => Ok(CType::Str),
        "int" => Ok(CType::Int),
        "num" => Ok(CType::Num),
        "bool" => Ok(CType::Bool),
        "datetime" => Ok(CType::DateTime),
        "date" => Ok(CType::Date),
        "any" => Ok(CType::Any),
        _ => {
            if let Some(inner) = s.strip_prefix('[').and_then(|t| t.strip_suffix(']')) {
                return Ok(CType::Array(Box::new(parse_type(inner)?)));
            }
            if let Some(inner) = s.strip_prefix('{').and_then(|t| t.strip_suffix('}')) {
                return Ok(CType::Object(parse_params(inner)?));
            }
            if s.contains('|') {
                let values: Vec<String> = s.split('|').map(str::to_string).collect();
                if values.iter().all(|v| is_plain_enum_value(v)) {
                    return Ok(CType::Enum(values));
                }
                return Err(format!("bad enum type {s:?}"));
            }
            Err(format!("unknown type {s:?}"))
        }
    }
}

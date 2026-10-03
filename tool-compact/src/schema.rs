//! JSON Schema subset used by compact tool signatures.
//!
//! Encoding and validation share one support check. A keyword this module cannot
//! represent or check makes the tool ineligible for compaction. Validation of a
//! call always uses the original schema, not the compact text.

use serde_json::{Map, Number, Value};

use crate::cursor::Cur;

const ANNOTATIONS: &[&str] = &[
    "description",
    "title",
    "examples",
    "example",
    "default",
    "$schema",
    "$id",
    "$comment",
    "deprecated",
    "readOnly",
    "writeOnly",
    "$defs",
    "definitions",
];

pub(crate) fn collapse_ws(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub(crate) fn is_prop_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

pub(crate) fn is_tool_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.' || c == ':')
}

/// Why this schema cannot be compacted, or `None` when the subset covers it.
pub(crate) fn unsupported_reason(schema: &Value) -> Option<String> {
    ensure_supported(schema).err()
}

fn ensure_supported(schema: &Value) -> Result<(), String> {
    let obj = schema
        .as_object()
        .ok_or_else(|| "schema must be a JSON object".to_string())?;
    if obj.contains_key("anyOf") && obj.contains_key("oneOf") {
        return Err("anyOf combined with oneOf is unsupported".into());
    }
    for (key, value) in obj {
        match key.as_str() {
            k if ANNOTATIONS.contains(&k) => {}
            "nullable" => {
                if !value.is_boolean() {
                    return Err("nullable must be a boolean".into());
                }
            }
            "type" => check_type_keyword(value)?,
            "properties" => {
                let map = value
                    .as_object()
                    .ok_or_else(|| "properties must be an object".to_string())?;
                for (name, sub) in map {
                    if !is_prop_name(name) {
                        return Err(format!("unsupported property name '{name}'"));
                    }
                    ensure_supported(sub)?;
                }
            }
            "required" => {
                let arr = value
                    .as_array()
                    .ok_or_else(|| "required must be an array".to_string())?;
                for entry in arr {
                    let Some(name) = entry.as_str() else {
                        return Err("required entries must be strings".into());
                    };
                    if !is_prop_name(name) {
                        return Err(format!("unsupported required name '{name}'"));
                    }
                }
            }
            "items" => {
                if value.is_array() {
                    return Err("tuple item schemas are unsupported".into());
                }
                ensure_supported(value)?;
            }
            "additionalProperties" => {
                if !value.is_boolean() {
                    return Err("additionalProperties schemas are unsupported".into());
                }
            }
            "enum" => check_enum(value)?,
            "const" => {
                if enum_value_reason(value).is_some() {
                    return Err("const value cannot be encoded safely".into());
                }
            }
            "format" => {
                let fmt = value
                    .as_str()
                    .ok_or_else(|| "format must be a string".to_string())?;
                if format_token(fmt).is_none() {
                    return Err(format!("unsupported format '{fmt}'"));
                }
                require_string_type(obj, "format")?;
            }
            "minimum" | "maximum" => {
                if !value.is_number() {
                    return Err(format!("{key} must be a number"));
                }
                require_numeric_type(obj, key)?;
            }
            "exclusiveMinimum" | "exclusiveMaximum" => match value {
                Value::Bool(true) => {
                    let partner = if key == "exclusiveMinimum" {
                        "minimum"
                    } else {
                        "maximum"
                    };
                    if !obj.get(partner).is_some_and(Value::is_number) {
                        return Err(format!("{key}: true requires {partner}"));
                    }
                    require_numeric_type(obj, key)?;
                }
                Value::Bool(false) => {}
                Value::Number(_) => require_numeric_type(obj, key)?,
                _ => return Err(format!("{key} must be a number or boolean")),
            },
            "minLength" | "maxLength" => {
                require_u64(value, key)?;
                require_string_type(obj, key)?;
            }
            "minItems" | "maxItems" => {
                require_u64(value, key)?;
                require_array_type(obj, key)?;
            }
            "uniqueItems" => {
                if !value.is_boolean() {
                    return Err("uniqueItems must be a boolean".into());
                }
                require_array_type(obj, key)?;
            }
            "pattern" => {
                let pat = value
                    .as_str()
                    .ok_or_else(|| "pattern must be a string".to_string())?;
                if regex::Regex::new(pat).is_err() {
                    return Err("pattern is not a valid regular expression".into());
                }
                require_string_type(obj, key)?;
            }
            "anyOf" | "oneOf" => {
                for (other, _) in obj {
                    if other == key || ANNOTATIONS.contains(&other.as_str()) {
                        continue;
                    }
                    return Err(format!("{key} combined with '{other}' is unsupported"));
                }
                let arr = value
                    .as_array()
                    .ok_or_else(|| format!("{key} must be an array"))?;
                if arr.is_empty() {
                    return Err(format!("{key} is empty"));
                }
                for branch in arr {
                    ensure_supported(branch)?;
                }
            }
            other => return Err(format!("unsupported keyword '{other}'")),
        }
    }
    if let Some(required) = obj.get("required").and_then(Value::as_array) {
        let props = obj.get("properties").and_then(Value::as_object);
        for name in required.iter().filter_map(Value::as_str) {
            if !props.is_some_and(|p| p.contains_key(name)) {
                return Err(format!("required property '{name}' has no schema"));
            }
        }
    }
    if (obj.contains_key("properties")
        || obj.contains_key("required")
        || obj.contains_key("additionalProperties"))
        && obj.contains_key("type")
        && obj.get("type").and_then(Value::as_str) != Some("object")
    {
        return Err("object keywords require type object".into());
    }
    if obj.contains_key("items") && obj.get("type").and_then(Value::as_str) != Some("array") {
        return Err("items requires type array".into());
    }
    Ok(())
}

fn check_type_keyword(value: &Value) -> Result<(), String> {
    match value {
        Value::String(ty) if is_known_type(ty) => Ok(()),
        Value::Array(arr) if !arr.is_empty() => {
            for ty in arr {
                let Some(name) = ty.as_str() else {
                    return Err("type array entries must be strings".into());
                };
                if !is_known_type(name) {
                    return Err(format!("unsupported type '{name}'"));
                }
            }
            Ok(())
        }
        Value::Array(_) => Err("type array is empty".into()),
        _ => Err("type must be a string or an array of strings".into()),
    }
}

fn is_known_type(name: &str) -> bool {
    matches!(
        name,
        "string" | "integer" | "number" | "boolean" | "null" | "array" | "object"
    )
}

fn check_enum(value: &Value) -> Result<(), String> {
    let arr = value
        .as_array()
        .ok_or_else(|| "enum must be an array".to_string())?;
    if arr.is_empty() {
        return Err("enum is empty".into());
    }
    for entry in arr {
        if let Some(reason) = enum_value_reason(entry) {
            return Err(reason);
        }
    }
    Ok(())
}

fn enum_value_reason(value: &Value) -> Option<String> {
    match value {
        Value::String(_) | Value::Number(_) | Value::Bool(_) | Value::Null => None,
        _ => Some("enum values must be scalars".into()),
    }
}

fn require_u64(value: &Value, key: &str) -> Result<(), String> {
    if value.as_u64().is_some() {
        Ok(())
    } else {
        Err(format!("{key} must be a non-negative integer"))
    }
}

fn require_string_type(obj: &Map<String, Value>, key: &str) -> Result<(), String> {
    match obj.get("type") {
        Some(Value::String(ty)) if ty == "string" => Ok(()),
        Some(Value::Array(arr))
            if arr.len() == 1 && arr.first().and_then(Value::as_str) == Some("string") =>
        {
            Ok(())
        }
        _ => Err(format!("{key} requires type string")),
    }
}

fn require_numeric_type(obj: &Map<String, Value>, key: &str) -> Result<(), String> {
    match obj.get("type").and_then(Value::as_str) {
        Some("integer" | "number") => Ok(()),
        _ => Err(format!("{key} requires type integer or number")),
    }
}

fn require_array_type(obj: &Map<String, Value>, key: &str) -> Result<(), String> {
    match obj.get("type").and_then(Value::as_str) {
        Some("array") => Ok(()),
        _ => Err(format!("{key} requires type array")),
    }
}

fn format_token(fmt: &str) -> Option<&'static str> {
    match fmt {
        "date-time" => Some("datetime"),
        "date" => Some("date"),
        "time" => Some("time"),
        "email" => Some("email"),
        "uri" => Some("uri"),
        "uuid" => Some("uuid"),
        _ => None,
    }
}

/// Field list and whether the parameter object rejects unknown properties.
///
/// The root schema must describe one object. Nested objects are encoded inside
/// the field types.
pub(crate) fn encode_parameters(schema: &Value) -> Result<(String, bool), String> {
    ensure_supported(schema)?;
    let obj = schema
        .as_object()
        .ok_or_else(|| "parameters must be a JSON object".to_string())?;
    if obj.contains_key("anyOf")
        || obj.contains_key("oneOf")
        || obj.contains_key("enum")
        || obj.contains_key("const")
        || obj.get("nullable") == Some(&Value::Bool(true))
    {
        return Err("tool parameters must be a single object schema".into());
    }
    if let Some(ty) = obj.get("type") {
        if ty.as_str() != Some("object") {
            return Err("tool parameters must be a JSON object schema".into());
        }
    } else if obj.keys().any(|key| {
        !ANNOTATIONS.contains(&key.as_str())
            && !matches!(
                key.as_str(),
                "properties" | "required" | "additionalProperties"
            )
    }) {
        return Err("tool parameters must be a JSON object schema".into());
    }
    let closed = obj.get("additionalProperties") == Some(&Value::Bool(false));
    Ok((encode_fields(obj)?, closed))
}

fn encode_node(schema: &Value) -> Result<String, String> {
    let obj = schema
        .as_object()
        .ok_or_else(|| "schema must be a JSON object".to_string())?;
    if obj.is_empty() {
        return Ok("json".into());
    }
    if let Some(branches) = obj.get("anyOf") {
        return encode_group("anyOf", branches);
    }
    if let Some(branches) = obj.get("oneOf") {
        return encode_group("oneOf", branches);
    }
    let nullable = obj
        .get("nullable")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let mut encoded = encode_core(obj)?;
    if nullable {
        if encoded.contains('|') || encoded.contains('=') {
            encoded = format!("anyOf({encoded}|null)");
        } else {
            encoded = format!("{encoded}|null");
        }
    }
    Ok(encoded)
}

fn encode_group(key: &str, branches: &Value) -> Result<String, String> {
    let arr = branches
        .as_array()
        .ok_or_else(|| format!("{key} must be an array"))?;
    let mut parts = Vec::with_capacity(arr.len());
    for branch in arr {
        parts.push(encode_node(branch)?);
    }
    Ok(format!("{key}({})", parts.join("|")))
}

fn encode_core(obj: &Map<String, Value>) -> Result<String, String> {
    if let Some(Value::Array(types)) = obj.get("type") {
        if obj.contains_key("enum") || obj.contains_key("const") {
            return Err("type union combined with enum is unsupported".into());
        }
        let mut parts = Vec::with_capacity(types.len());
        for ty in types {
            let name = ty
                .as_str()
                .ok_or_else(|| "type array entries must be strings".to_string())?;
            parts.push(prim_token(name)?);
        }
        return Ok(parts.join("|"));
    }

    if obj.contains_key("properties")
        || obj.get("type").and_then(Value::as_str) == Some("object")
        || (obj.get("type").is_none() && obj.contains_key("required"))
    {
        return encode_object(obj);
    }
    if obj.get("type").and_then(Value::as_str) == Some("array") {
        return encode_array(obj);
    }

    let type_name = obj.get("type").and_then(Value::as_str);
    let mut atom = match type_name {
        Some(name) => prim_token(name)?,
        None if obj.contains_key("enum") || obj.contains_key("const") => {
            infer_scalar_token(obj.get("enum").or(obj.get("const")))?
        }
        None => "json".to_string(),
    };
    if type_name == Some("string")
        && let Some(fmt) = obj.get("format").and_then(Value::as_str)
    {
        atom = format_token(fmt)
            .ok_or_else(|| format!("unsupported format '{fmt}'"))?
            .to_string();
    }
    atom.push_str(&encode_suffixes(obj)?);
    Ok(atom)
}

fn infer_scalar_token(value: Option<&Value>) -> Result<String, String> {
    let samples: Vec<&Value> = match value {
        Some(Value::Array(arr)) => arr.iter().collect(),
        Some(other) => vec![other],
        None => return Ok("json".into()),
    };
    let mut saw_float = false;
    let mut kind: Option<&str> = None;
    for sample in samples {
        let next = match sample {
            Value::String(_) => "str",
            Value::Bool(_) => "bool",
            Value::Null => "null",
            Value::Number(n) if n.is_i64() || n.is_u64() => "int",
            Value::Number(_) => {
                saw_float = true;
                "num"
            }
            _ => return Err("enum values must be scalars".into()),
        };
        if let Some(prev) = kind
            && prev != next
            && !matches!((prev, next), ("int", "num") | ("num", "int"))
        {
            return Err("mixed enum types are unsupported".into());
        }
        kind = Some(if saw_float { "num" } else { next });
    }
    Ok(kind.unwrap_or("json").to_string())
}

fn prim_token(type_name: &str) -> Result<String, String> {
    let token = match type_name {
        "string" => "str",
        "integer" => "int",
        "number" => "num",
        "boolean" => "bool",
        "null" => "null",
        "object" => return Err("object type is encoded from its fields".into()),
        "array" => return Err("array type is encoded from its items".into()),
        other => return Err(format!("unsupported type '{other}'")),
    };
    Ok(token.to_string())
}

fn encode_object(obj: &Map<String, Value>) -> Result<String, String> {
    if obj.contains_key("enum") || obj.contains_key("const") {
        return Err("object enums are unsupported".into());
    }
    let fields = encode_fields(obj)?;
    let mut encoded = format!("{{{fields}}}");
    if obj.get("additionalProperties") == Some(&Value::Bool(false)) {
        encoded.push('!');
    }
    Ok(encoded)
}

pub(crate) fn encode_fields(obj: &Map<String, Value>) -> Result<String, String> {
    let Some(props) = obj.get("properties").and_then(Value::as_object) else {
        if obj.contains_key("required") {
            return Err("required properties without a properties map".into());
        }
        return Ok(String::new());
    };
    let required = required_names(obj)?;
    for name in &required {
        if !props.contains_key(name) {
            return Err(format!("required property '{name}' has no schema"));
        }
    }
    let mut parts = Vec::new();
    for (name, sub) in props {
        let req = required.iter().any(|r| r == name);
        parts.push(encode_field(name, sub, req)?);
    }
    Ok(parts.join(","))
}

fn required_names(obj: &Map<String, Value>) -> Result<Vec<String>, String> {
    let Some(arr) = obj.get("required").and_then(Value::as_array) else {
        return Ok(Vec::new());
    };
    let mut names = Vec::new();
    for entry in arr {
        let Some(name) = entry.as_str() else {
            return Err("required entries must be strings".into());
        };
        if !names.iter().any(|n| n == name) {
            names.push(name.to_string());
        }
    }
    Ok(names)
}

fn encode_field(name: &str, schema: &Value, required: bool) -> Result<String, String> {
    let mut out = String::new();
    out.push_str(name);
    if !required {
        out.push('?');
    }
    out.push(':');
    out.push_str(&encode_node(schema)?);
    if let Some(desc) = schema.get("description").and_then(Value::as_str) {
        let collapsed = collapse_ws(desc);
        if !collapsed.is_empty() {
            out.push(' ');
            out.push_str(&serde_json::to_string(&collapsed).map_err(|e| e.to_string())?);
        }
    }
    Ok(out)
}

fn encode_array(obj: &Map<String, Value>) -> Result<String, String> {
    let empty = Value::Object(Map::new());
    let items = obj.get("items").unwrap_or(&empty);
    let mut encoded = format!("[{}]", encode_node(items)?);
    encoded.push_str(&encode_suffixes(obj)?);
    Ok(encoded)
}

fn encode_suffixes(obj: &Map<String, Value>) -> Result<String, String> {
    let mut out = String::new();
    out.push_str(&encode_bounds(obj)?);
    if obj.contains_key("minLength") || obj.contains_key("maxLength") {
        out.push_str(&encode_range(obj.get("minLength"), obj.get("maxLength"))?);
    }
    if obj.contains_key("minItems") || obj.contains_key("maxItems") {
        out.push_str(&encode_range(obj.get("minItems"), obj.get("maxItems"))?);
    }
    if let Some(pat) = obj.get("pattern").and_then(Value::as_str) {
        out.push('~');
        out.push_str(&serde_json::to_string(pat).map_err(|e| e.to_string())?);
    }
    if obj.get("uniqueItems") == Some(&Value::Bool(true)) {
        out.push_str("!dup");
    }
    if let Some(values) = enum_values(obj)? {
        out.push('=');
        let mut parts = Vec::with_capacity(values.len());
        for value in values {
            parts.push(encode_enum_value(value)?);
        }
        out.push_str(&parts.join("|"));
    }
    Ok(out)
}

fn enum_values(obj: &Map<String, Value>) -> Result<Option<Vec<&Value>>, String> {
    if let Some(arr) = obj.get("enum").and_then(Value::as_array) {
        if obj.contains_key("const") {
            return Err("enum combined with const is unsupported".into());
        }
        return Ok(Some(arr.iter().collect()));
    }
    if let Some(value) = obj.get("const") {
        return Ok(Some(vec![value]));
    }
    Ok(None)
}

fn encode_enum_value(value: &Value) -> Result<String, String> {
    match value {
        Value::String(text) => {
            if bare_enum_string(text) {
                Ok(text.clone())
            } else {
                serde_json::to_string(text).map_err(|e| e.to_string())
            }
        }
        Value::Number(n) => Ok(n.to_string()),
        Value::Bool(b) => Ok(b.to_string()),
        Value::Null => Ok("null".into()),
        _ => Err("enum values must be scalars".into()),
    }
}

fn bare_enum_string(text: &str) -> bool {
    if text.is_empty() || matches!(text, "true" | "false" | "null") {
        return false;
    }
    if text
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_digit() || c == '-')
    {
        return false;
    }
    text.chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '+'))
}

fn encode_bounds(obj: &Map<String, Value>) -> Result<String, String> {
    let mut out = String::new();
    match obj.get("exclusiveMinimum") {
        Some(Value::Bool(true)) => {
            let min = obj
                .get("minimum")
                .ok_or_else(|| "exclusiveMinimum requires minimum".to_string())?;
            out.push('>');
            out.push_str(&number_token(min)?);
        }
        Some(Value::Number(n)) => {
            if let Some(min) = obj.get("minimum") {
                out.push_str(">=");
                out.push_str(&number_token(min)?);
            }
            out.push('>');
            out.push_str(&n.to_string());
        }
        _ => {
            if let Some(min) = obj.get("minimum") {
                out.push_str(">=");
                out.push_str(&number_token(min)?);
            }
        }
    }
    match obj.get("exclusiveMaximum") {
        Some(Value::Bool(true)) => {
            let max = obj
                .get("maximum")
                .ok_or_else(|| "exclusiveMaximum requires maximum".to_string())?;
            out.push('<');
            out.push_str(&number_token(max)?);
        }
        Some(Value::Number(n)) => {
            if let Some(max) = obj.get("maximum") {
                out.push_str("<=");
                out.push_str(&number_token(max)?);
            }
            out.push('<');
            out.push_str(&n.to_string());
        }
        _ => {
            if let Some(max) = obj.get("maximum") {
                out.push_str("<=");
                out.push_str(&number_token(max)?);
            }
        }
    }
    Ok(out)
}

fn number_token(value: &Value) -> Result<String, String> {
    match value {
        Value::Number(n) => Ok(n.to_string()),
        _ => Err("bound must be a number".into()),
    }
}

fn encode_range(min: Option<&Value>, max: Option<&Value>) -> Result<String, String> {
    let min = match min {
        Some(v) => number_token(v)?,
        None => String::new(),
    };
    let max = match max {
        Some(v) => number_token(v)?,
        None => String::new(),
    };
    Ok(format!("#{min}..{max}"))
}

pub(crate) fn parse_fields_schema(input: &str, closed: bool) -> Result<Value, String> {
    let mut cur = Cur::new(input);
    let mut schema = parse_fields(&mut cur)?;
    if !cur.eof() {
        return Err("trailing input in field list".into());
    }
    if closed {
        schema
            .as_object_mut()
            .ok_or_else(|| "field list is not an object schema".to_string())?
            .insert("additionalProperties".into(), Value::Bool(false));
    }
    Ok(schema)
}

fn parse_union(cur: &mut Cur<'_>) -> Result<Value, String> {
    let alts = parse_union_alts(cur)?;
    union_to_schema(alts)
}

fn parse_union_alts(cur: &mut Cur<'_>) -> Result<Vec<Value>, String> {
    let mut alts = vec![parse_single(cur)?];
    while cur.eat('|') {
        alts.push(parse_single(cur)?);
    }
    Ok(alts)
}

fn union_to_schema(mut alts: Vec<Value>) -> Result<Value, String> {
    if alts.len() == 1 {
        return alts.pop().ok_or_else(|| "empty type union".to_string());
    }
    if alts.iter().all(is_plain_type) {
        let types = alts
            .iter()
            .filter_map(|alt| alt.get("type").cloned())
            .collect();
        return Ok(json_type_array(types));
    }
    Ok(serde_json::json!({"anyOf": alts}))
}

fn is_plain_type(schema: &Value) -> bool {
    let Some(obj) = schema.as_object() else {
        return false;
    };
    obj.len() == 1 && obj.get("type").and_then(Value::as_str).is_some()
}

fn json_type_array(types: Vec<Value>) -> Value {
    serde_json::json!({"type": types})
}

fn parse_single(cur: &mut Cur<'_>) -> Result<Value, String> {
    let mut schema = parse_atom(cur)?;
    loop {
        if cur.starts(">=") || cur.starts("<=") || matches!(cur.peek(), Some('>' | '<')) {
            parse_bound(cur, &mut schema)?;
        } else if cur.eat('#') {
            parse_range(cur, &mut schema)?;
        } else if cur.eat('~') {
            let pat = parse_json_string(cur)?;
            insert(&mut schema, "pattern", Value::String(pat))?;
        } else if cur.eat_str("!dup") {
            insert(&mut schema, "uniqueItems", Value::Bool(true))?;
        } else if cur.eat('!') {
            insert(&mut schema, "additionalProperties", Value::Bool(false))?;
        } else if cur.eat('=') {
            parse_enum(cur, &mut schema)?;
            break;
        } else {
            break;
        }
    }
    Ok(schema)
}

fn parse_atom(cur: &mut Cur<'_>) -> Result<Value, String> {
    if cur.eat('[') {
        let items = parse_union(cur)?;
        if !cur.eat(']') {
            return Err("unclosed array type".into());
        }
        return Ok(serde_json::json!({"type": "array", "items": items}));
    }
    if cur.eat('{') {
        let schema = parse_fields(cur)?;
        if !cur.eat('}') {
            return Err("unclosed object type".into());
        }
        return Ok(schema);
    }
    let ident = parse_ident(cur)?;
    if ident == "anyOf" || ident == "oneOf" {
        if !cur.eat('(') {
            return Err(format!("expected '(' after {ident}"));
        }
        let alts = parse_union_alts(cur)?;
        if !cur.eat(')') {
            return Err(format!("unclosed {ident}"));
        }
        return Ok(serde_json::json!({ ident: alts }));
    }
    atom_schema(&ident)
}

fn atom_schema(ident: &str) -> Result<Value, String> {
    let schema = match ident {
        "str" => serde_json::json!({"type": "string"}),
        "int" => serde_json::json!({"type": "integer"}),
        "num" => serde_json::json!({"type": "number"}),
        "bool" => serde_json::json!({"type": "boolean"}),
        "null" => serde_json::json!({"type": "null"}),
        "json" => Value::Object(Map::new()),
        "datetime" => serde_json::json!({"type": "string", "format": "date-time"}),
        "date" => serde_json::json!({"type": "string", "format": "date"}),
        "time" => serde_json::json!({"type": "string", "format": "time"}),
        "email" => serde_json::json!({"type": "string", "format": "email"}),
        "uri" => serde_json::json!({"type": "string", "format": "uri"}),
        "uuid" => serde_json::json!({"type": "string", "format": "uuid"}),
        other => return Err(format!("unknown type '{other}'")),
    };
    Ok(schema)
}

fn parse_fields(cur: &mut Cur<'_>) -> Result<Value, String> {
    let mut props = Map::new();
    let mut required = Vec::new();
    if matches!(cur.peek(), Some(')' | '}')) {
        return Ok(object_schema(props, required));
    }
    loop {
        let (name, optional, schema) = parse_field(cur)?;
        if props.contains_key(&name) {
            return Err(format!("duplicate field '{name}'"));
        }
        if !optional {
            required.push(Value::String(name.clone()));
        }
        props.insert(name, schema);
        if cur.eat(',') {
            if matches!(cur.peek(), Some(')' | '}')) {
                return Err("trailing comma in field list".into());
            }
            continue;
        }
        break;
    }
    Ok(object_schema(props, required))
}

fn object_schema(props: Map<String, Value>, required: Vec<Value>) -> Value {
    let mut map = Map::new();
    map.insert("type".into(), Value::String("object".into()));
    map.insert("properties".into(), Value::Object(props));
    if !required.is_empty() {
        map.insert("required".into(), Value::Array(required));
    }
    Value::Object(map)
}

fn parse_field(cur: &mut Cur<'_>) -> Result<(String, bool, Value), String> {
    let name = parse_ident(cur)?;
    let optional = cur.eat('?');
    if !cur.eat(':') {
        return Err(format!("expected ':' after field '{name}'"));
    }
    let mut schema = parse_union(cur)?;
    if cur.peek().is_some_and(char::is_whitespace) {
        cur.skip_ws();
        if cur.peek() == Some('"') {
            let desc = parse_json_string(cur)?;
            insert(&mut schema, "description", Value::String(desc))?;
        }
    }
    Ok((name, optional, schema))
}

fn parse_ident(cur: &mut Cur<'_>) -> Result<String, String> {
    let start = cur.i;
    let Some(first) = cur.peek() else {
        return Err("expected a name".into());
    };
    if !(first.is_ascii_alphabetic() || first == '_') {
        return Err(format!("expected a name, found '{first}'"));
    }
    cur.bump();
    while cur
        .peek()
        .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        cur.bump();
    }
    Ok(cur.s[start..cur.i].to_string())
}

fn parse_bound(cur: &mut Cur<'_>, schema: &mut Value) -> Result<(), String> {
    let key = if cur.eat_str(">=") {
        "minimum"
    } else if cur.eat_str("<=") {
        "maximum"
    } else if cur.eat('>') {
        "exclusiveMinimum"
    } else if cur.eat('<') {
        "exclusiveMaximum"
    } else {
        return Err("expected a numeric bound".into());
    };
    let number = parse_number(cur)?;
    insert(schema, key, Value::Number(number))
}

fn parse_range(cur: &mut Cur<'_>, schema: &mut Value) -> Result<(), String> {
    let min = if cur.peek().is_some_and(|c| c.is_ascii_digit()) {
        Some(parse_u64(cur)?)
    } else {
        None
    };
    if !cur.eat_str("..") {
        return Err("expected '..' in a length bound".into());
    }
    let max = if cur.peek().is_some_and(|c| c.is_ascii_digit()) {
        Some(parse_u64(cur)?)
    } else {
        None
    };
    if min.is_none() && max.is_none() {
        return Err("empty length bound".into());
    }
    let array = schema.get("type").and_then(Value::as_str) == Some("array");
    if let Some(min) = min {
        let key = if array { "minItems" } else { "minLength" };
        insert(schema, key, Value::from(min))?;
    }
    if let Some(max) = max {
        let key = if array { "maxItems" } else { "maxLength" };
        insert(schema, key, Value::from(max))?;
    }
    Ok(())
}

fn parse_u64(cur: &mut Cur<'_>) -> Result<u64, String> {
    let token = parse_number_token(cur)?;
    token
        .parse()
        .map_err(|_| format!("expected an integer, found '{token}'"))
}

fn parse_number(cur: &mut Cur<'_>) -> Result<Number, String> {
    let token = parse_number_token(cur)?;
    serde_json::from_str::<Value>(&token)
        .ok()
        .and_then(|v| match v {
            Value::Number(n) => Some(n),
            _ => None,
        })
        .ok_or_else(|| format!("invalid number '{token}'"))
}

fn parse_number_token(cur: &mut Cur<'_>) -> Result<String, String> {
    let start = cur.i;
    cur.eat('-');
    if !cur.peek().is_some_and(|c| c.is_ascii_digit()) {
        return Err("expected a number".into());
    }
    while cur.peek().is_some_and(|c| c.is_ascii_digit()) {
        cur.bump();
    }
    if cur.eat('.') {
        if !cur.peek().is_some_and(|c| c.is_ascii_digit()) {
            return Err("expected digits after decimal point".into());
        }
        while cur.peek().is_some_and(|c| c.is_ascii_digit()) {
            cur.bump();
        }
    }
    Ok(cur.s[start..cur.i].to_string())
}

fn parse_enum(cur: &mut Cur<'_>, schema: &mut Value) -> Result<(), String> {
    let type_name = schema
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let mut values = Vec::new();
    loop {
        values.push(parse_enum_value(cur, &type_name)?);
        if cur.eat('|') {
            continue;
        }
        break;
    }
    if values.is_empty() {
        return Err("empty enum".into());
    }
    insert(schema, "enum", Value::Array(values))
}

fn parse_enum_value(cur: &mut Cur<'_>, type_name: &str) -> Result<Value, String> {
    if cur.peek() == Some('"') {
        return Ok(Value::String(parse_json_string(cur)?));
    }
    if cur.peek().is_some_and(|c| c.is_ascii_digit() || c == '-') {
        let number = parse_number(cur)?;
        if type_name == "integer" && !(number.is_i64() || number.is_u64()) {
            return Err("integer enum value is not an integer".into());
        }
        return Ok(Value::Number(number));
    }
    let bare = read_bare(cur)?;
    match type_name {
        "boolean" => match bare.as_str() {
            "true" => Ok(Value::Bool(true)),
            "false" => Ok(Value::Bool(false)),
            _ => Err(format!("invalid boolean enum value '{bare}'")),
        },
        "null" => {
            if bare == "null" {
                Ok(Value::Null)
            } else {
                Err(format!("invalid null enum value '{bare}'"))
            }
        }
        "integer" | "number" => Err(format!("expected a numeric enum value, found '{bare}'")),
        _ => Ok(Value::String(bare)),
    }
}

fn read_bare(cur: &mut Cur<'_>) -> Result<String, String> {
    let start = cur.i;
    let Some(first) = cur.peek() else {
        return Err("empty enum value".into());
    };
    if !(first.is_ascii_alphanumeric() || matches!(first, '_' | '-' | '.' | '+')) {
        return Err("invalid enum value".into());
    }
    while cur
        .peek()
        .is_some_and(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '+'))
    {
        cur.bump();
    }
    Ok(cur.s[start..cur.i].to_string())
}

fn parse_json_string(cur: &mut Cur<'_>) -> Result<String, String> {
    if !cur.eat('"') {
        return Err("expected a JSON string".into());
    }
    let start = cur.i - 1;
    let mut escape = false;
    loop {
        let Some(ch) = cur.bump() else {
            return Err("unterminated JSON string".into());
        };
        if escape {
            escape = false;
            continue;
        }
        if ch == '\\' {
            escape = true;
            continue;
        }
        if ch == '"' {
            break;
        }
    }
    let slice = &cur.s[start..cur.i];
    let parsed: Value = serde_json::from_str(slice).map_err(|e| e.to_string())?;
    parsed
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| "expected a JSON string".to_string())
}

fn insert(schema: &mut Value, key: &str, value: Value) -> Result<(), String> {
    let obj = schema
        .as_object_mut()
        .ok_or_else(|| "internal schema was not an object".to_string())?;
    if obj.contains_key(key) {
        return Err(format!("duplicate constraint '{key}'"));
    }
    obj.insert(key.to_string(), value);
    Ok(())
}

/// Check `value` against the original tool JSON Schema.
pub(crate) fn validate(value: &Value, schema: &Value) -> Result<(), String> {
    let Some(obj) = schema.as_object() else {
        return Err("schema must be a JSON object".into());
    };
    if let Some(reason) = unsupported_reason(schema) {
        return Err(format!("schema cannot be validated ({reason})"));
    }
    if obj.is_empty() {
        return Ok(());
    }
    if let Some(branches) = obj.get("anyOf").and_then(Value::as_array) {
        let mut errors = Vec::new();
        for branch in branches {
            match validate(value, branch) {
                Ok(()) => return Ok(()),
                Err(err) => errors.push(err),
            }
        }
        return Err(format!(
            "no anyOf alternative matched ({})",
            errors.join("; ")
        ));
    }
    if let Some(branches) = obj.get("oneOf").and_then(Value::as_array) {
        let matched = branches
            .iter()
            .filter(|branch| validate(value, branch).is_ok())
            .count();
        if matched != 1 {
            return Err(format!("oneOf matched {matched} alternatives, expected 1"));
        }
        return Ok(());
    }
    if let Some(expected) = obj.get("const")
        && value != expected
    {
        return Err(format!("expected const {expected}"));
    }
    if let Some(entries) = obj.get("enum").and_then(Value::as_array)
        && !entries.iter().any(|entry| entry == value)
    {
        return Err("value is not in the enum".into());
    }
    if let Some(ty) = obj.get("type") {
        let nullable = obj
            .get("nullable")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        check_instance_type(value, ty, nullable)?;
    }
    match value {
        Value::String(text) => check_string(text, obj)?,
        Value::Number(number) => check_number(number, obj)?,
        Value::Array(items) => check_array(items, obj)?,
        Value::Object(map) => check_object(map, obj)?,
        _ => {}
    }
    Ok(())
}

fn check_instance_type(value: &Value, ty: &Value, nullable: bool) -> Result<(), String> {
    if nullable && value.is_null() {
        return Ok(());
    }
    let names: Vec<&str> = match ty {
        Value::String(name) => vec![name.as_str()],
        Value::Array(arr) => arr.iter().filter_map(Value::as_str).collect(),
        _ => return Err("type must be a string or an array of strings".into()),
    };
    if names.iter().any(|name| instance_matches(value, name)) {
        Ok(())
    } else {
        Err(format!(
            "expected {}, got {}",
            names.join("|"),
            json_kind(value)
        ))
    }
}

fn instance_matches(value: &Value, type_name: &str) -> bool {
    match type_name {
        "string" => value.is_string(),
        "integer" => value.as_number().is_some_and(|n| n.is_i64() || n.is_u64()),
        "number" => value.is_number(),
        "boolean" => value.is_boolean(),
        "null" => value.is_null(),
        "array" => value.is_array(),
        "object" => value.is_object(),
        _ => false,
    }
}

fn json_kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(n) if n.is_i64() || n.is_u64() => "integer",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn check_string(text: &str, schema: &Map<String, Value>) -> Result<(), String> {
    let len = text.chars().count();
    if let Some(min) = schema.get("minLength").and_then(Value::as_u64)
        && (len as u64) < min
    {
        return Err(format!("shorter than minLength {min}"));
    }
    if let Some(max) = schema.get("maxLength").and_then(Value::as_u64)
        && (len as u64) > max
    {
        return Err(format!("longer than maxLength {max}"));
    }
    if let Some(pat) = schema.get("pattern").and_then(Value::as_str) {
        let re = regex::Regex::new(pat).map_err(|e| format!("invalid pattern: {e}"))?;
        if !re.is_match(text) {
            return Err(format!("does not match pattern {pat}"));
        }
    }
    if let Some(fmt) = schema.get("format").and_then(Value::as_str) {
        check_format(text, fmt)?;
    }
    Ok(())
}

fn check_format(text: &str, fmt: &str) -> Result<(), String> {
    let ok = match fmt {
        "date-time" => is_date_time(text),
        "date" => is_date(text),
        "time" => is_time(text),
        "email" => is_email(text),
        "uri" => is_uri(text),
        "uuid" => is_uuid(text),
        other => return Err(format!("unsupported format '{other}'")),
    };
    if ok {
        Ok(())
    } else {
        Err(format!("value does not match format {fmt}"))
    }
}

fn is_date(text: &str) -> bool {
    let b = text.as_bytes();
    b.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && digits(&b[0..4])
        && digits(&b[5..7])
        && digits(&b[8..10])
}

fn is_time(text: &str) -> bool {
    let (hms, frac) = match text.split_once('.') {
        Some((hms, frac)) => (hms, Some(frac)),
        None => (text, None),
    };
    let b = hms.as_bytes();
    if b.len() != 8
        || b[2] != b':'
        || b[5] != b':'
        || !digits(&b[0..2])
        || !digits(&b[3..5])
        || !digits(&b[6..8])
    {
        return false;
    }
    match frac {
        None => true,
        Some(frac) => !frac.is_empty() && frac.bytes().all(|c| c.is_ascii_digit()),
    }
}

fn is_date_time(text: &str) -> bool {
    let Some((date, rest)) = text.split_once('T') else {
        return false;
    };
    if !is_date(date) {
        return false;
    }
    let Some((time, zone)) = split_zone(rest) else {
        return false;
    };
    is_time(time) && is_zone(zone)
}

fn split_zone(rest: &str) -> Option<(&str, &str)> {
    if let Some(time) = rest.strip_suffix('Z') {
        return Some((time, "Z"));
    }
    let bytes = rest.as_bytes();
    let mut i = 8.min(bytes.len());
    while i < bytes.len() {
        if bytes[i] == b'+' || bytes[i] == b'-' {
            return Some((&rest[..i], &rest[i..]));
        }
        i += 1;
    }
    None
}

fn is_zone(zone: &str) -> bool {
    if zone == "Z" {
        return true;
    }
    let b = zone.as_bytes();
    b.len() == 6
        && (b[0] == b'+' || b[0] == b'-')
        && b[3] == b':'
        && digits(&b[1..3])
        && digits(&b[4..6])
}

fn is_email(text: &str) -> bool {
    let Some((local, domain)) = text.split_once('@') else {
        return false;
    };
    !local.is_empty()
        && !domain.is_empty()
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && !local.chars().any(char::is_whitespace)
        && !domain.chars().any(char::is_whitespace)
}

fn is_uri(text: &str) -> bool {
    let Some((scheme, rest)) = text.split_once(':') else {
        return false;
    };
    let mut chars = scheme.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    first.is_ascii_alphabetic()
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '.' | '-'))
        && !rest.is_empty()
}

fn is_uuid(text: &str) -> bool {
    let b = text.as_bytes();
    if b.len() != 36 {
        return false;
    }
    let groups = [8usize, 4, 4, 4, 12];
    let mut i = 0;
    for (index, group) in groups.iter().enumerate() {
        if index > 0 {
            if b[i] != b'-' {
                return false;
            }
            i += 1;
        }
        if !b[i..i + group].iter().all(|c| c.is_ascii_hexdigit()) {
            return false;
        }
        i += group;
    }
    true
}

fn digits(bytes: &[u8]) -> bool {
    !bytes.is_empty() && bytes.iter().all(|c| c.is_ascii_digit())
}

fn check_number(number: &Number, schema: &Map<String, Value>) -> Result<(), String> {
    let Some(value) = number.as_f64() else {
        return Err("number is outside the comparable range".into());
    };
    if !value.is_finite() {
        return Err("number is not finite".into());
    }
    let emin = schema.get("exclusiveMinimum");
    let emax = schema.get("exclusiveMaximum");
    if let Some(bound) = emin.and_then(Value::as_f64)
        && value <= bound
    {
        return Err(format!("must be > {bound}"));
    }
    if let Some(min) = schema.get("minimum").and_then(Value::as_f64) {
        if matches!(emin, Some(Value::Bool(true))) {
            if value <= min {
                return Err(format!("must be > {min}"));
            }
        } else if value < min {
            return Err(format!("must be >= {min}"));
        }
    } else if matches!(emin, Some(Value::Bool(true))) {
        return Err("exclusiveMinimum without minimum".into());
    }
    if let Some(bound) = emax.and_then(Value::as_f64)
        && value >= bound
    {
        return Err(format!("must be < {bound}"));
    }
    if let Some(max) = schema.get("maximum").and_then(Value::as_f64) {
        if matches!(emax, Some(Value::Bool(true))) {
            if value >= max {
                return Err(format!("must be < {max}"));
            }
        } else if value > max {
            return Err(format!("must be <= {max}"));
        }
    } else if matches!(emax, Some(Value::Bool(true))) {
        return Err("exclusiveMaximum without maximum".into());
    }
    Ok(())
}

fn check_array(items: &[Value], schema: &Map<String, Value>) -> Result<(), String> {
    if let Some(min) = schema.get("minItems").and_then(Value::as_u64)
        && (items.len() as u64) < min
    {
        return Err(format!("fewer than minItems {min}"));
    }
    if let Some(max) = schema.get("maxItems").and_then(Value::as_u64)
        && (items.len() as u64) > max
    {
        return Err(format!("more than maxItems {max}"));
    }
    if schema.get("uniqueItems") == Some(&Value::Bool(true)) {
        for i in 0..items.len() {
            for j in (i + 1)..items.len() {
                if items[i] == items[j] {
                    return Err("duplicate array item".into());
                }
            }
        }
    }
    if let Some(item_schema) = schema.get("items") {
        for (index, item) in items.iter().enumerate() {
            validate(item, item_schema).map_err(|err| format!("[{index}]: {err}"))?;
        }
    }
    Ok(())
}

fn check_object(map: &Map<String, Value>, schema: &Map<String, Value>) -> Result<(), String> {
    let props = schema.get("properties").and_then(Value::as_object);
    if let Some(required) = schema.get("required").and_then(Value::as_array) {
        for name in required.iter().filter_map(Value::as_str) {
            if !map.contains_key(name) {
                return Err(format!("missing required property '{name}'"));
            }
        }
    }
    let additional = schema
        .get("additionalProperties")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    for (key, value) in map {
        if let Some(sub) = props.and_then(|props| props.get(key)) {
            validate(value, sub).map_err(|err| format!("{key}: {err}"))?;
        } else if !additional {
            return Err(format!("unexpected property '{key}'"));
        }
    }
    Ok(())
}

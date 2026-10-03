//! Schema decoder: reconstruct ToolDefs from compact signature text.

use serde_json::{Map, Value, json};

use crate::slice::{safe_slice, safe_slice_from, safe_slice_to};
use crate::types::{CompactError, CompactTools, ToolDef};

/// Decode compact signatures into full ToolDefs.
pub fn decode_tools(compact: &CompactTools) -> Result<Vec<ToolDef>, CompactError> {
    let mut tools = Vec::new();
    for line in compact.text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let tool = decode_tool_signature(trimmed)?;
        tools.push(tool);
    }
    Ok(tools)
}

/// Decode a single signature:
/// `name(p:type, q?:type=default /* desc */) - description`
pub fn decode_tool_signature(sig: &str) -> Result<ToolDef, CompactError> {
    let paren_open = sig
        .find('(')
        .ok_or_else(|| CompactError::Malformed(format!("Missing '(' in signature: {sig}")))?;

    let name = safe_slice_to(sig, paren_open).trim().to_string();

    let paren_close = find_matching_close_paren(safe_slice_from(sig, paren_open))
        .map(|idx| paren_open + idx)
        .ok_or_else(|| CompactError::Malformed(format!("Unbalanced ')' in signature: {sig}")))?;

    let params_str = safe_slice(sig, paren_open + 1, paren_close).trim();

    let description = if paren_close + 1 < sig.len() {
        let rest = safe_slice_from(sig, paren_close + 1).trim();
        if let Some(desc) = rest.strip_prefix('-') {
            let d = desc.trim();
            if !d.is_empty() {
                Some(d.to_string())
            } else {
                None
            }
        } else {
            None
        }
    } else {
        None
    };

    let parameters = if params_str.is_empty() {
        Some(json!({
            "type": "object",
            "properties": {}
        }))
    } else {
        let (properties, required) = parse_parameter_list(params_str)?;
        let mut obj = Map::new();
        obj.insert("type".to_string(), Value::String("object".to_string()));
        obj.insert("properties".to_string(), Value::Object(properties));
        if !required.is_empty() {
            obj.insert(
                "required".to_string(),
                Value::Array(required.into_iter().map(Value::String).collect()),
            );
        }
        Some(Value::Object(obj))
    };

    Ok(ToolDef {
        name,
        description,
        parameters,
    })
}

fn parse_parameter_list(
    params_str: &str,
) -> Result<(Map<String, Value>, Vec<String>), CompactError> {
    let mut properties = Map::new();
    let mut required = Vec::new();

    let tokens = split_top_level(params_str, ',')?;
    for token in tokens {
        let trimmed = token.trim();
        if trimmed.is_empty() {
            continue;
        }

        let colon_idx = trimmed
            .find(':')
            .ok_or_else(|| CompactError::Malformed(format!("Missing ':' in param: {trimmed}")))?;

        let param_decl = safe_slice_to(trimmed, colon_idx).trim();
        let (param_name, is_req) = if let Some(stripped) = param_decl.strip_suffix('?') {
            (stripped.trim().to_string(), false)
        } else {
            (param_decl.to_string(), true)
        };

        if is_req {
            required.push(param_name.clone());
        }

        let type_and_rest = safe_slice_from(trimmed, colon_idx + 1).trim();
        let schema = parse_param_type_and_rest(type_and_rest)?;
        properties.insert(param_name, schema);
    }

    Ok((properties, required))
}

fn parse_param_type_and_rest(input: &str) -> Result<Value, CompactError> {
    let mut s = input.trim();

    // Check for trailing comment /* desc */
    let mut description: Option<String> = None;
    if let Some(comment_start) = s.rfind("/*") {
        let comment_rest = safe_slice_from(s, comment_start);
        if let Some(comment_end) = comment_rest.find("*/") {
            let desc_content = safe_slice(s, comment_start + 2, comment_start + comment_end).trim();
            description = Some(desc_content.to_string());
            s = safe_slice_to(s, comment_start).trim();
        }
    }

    // Check for default value: =val
    let mut default_val: Option<Value> = None;
    if let Some(eq_idx) = find_top_level_equal(s) {
        let val_str = safe_slice_from(s, eq_idx + 1).trim();
        default_val = Some(parse_json_or_literal(val_str));
        s = safe_slice_to(s, eq_idx).trim();
    }

    let mut schema = parse_single_type(s)?;

    if let Some(obj) = schema.as_object_mut() {
        if let Some(desc) = description {
            obj.insert("description".to_string(), Value::String(desc));
        }
        if let Some(def) = default_val {
            obj.insert("default".to_string(), def);
        }
    }

    Ok(schema)
}

fn parse_single_type(s: &str) -> Result<Value, CompactError> {
    let trimmed = s.trim();

    // Const check: =val
    if let Some(const_val) = trimmed.strip_prefix('=') {
        let val = parse_json_or_literal(const_val);
        return Ok(json!({ "const": val }));
    }

    // Enum check: val1|val2|...
    if trimmed.contains('|') {
        let variants = split_top_level(trimmed, '|')?;
        let mut enum_vals = Vec::new();
        for v in variants {
            let v_trim = v.trim();
            if (v_trim.starts_with('"') && v_trim.ends_with('"'))
                || (v_trim.starts_with('\'') && v_trim.ends_with('\''))
            {
                if v_trim.len() >= 2 {
                    let unquoted = unescape_string(safe_slice(v_trim, 1, v_trim.len() - 1));
                    enum_vals.push(Value::String(unquoted));
                }
            } else if let Ok(num) = v_trim.parse::<i64>() {
                enum_vals.push(json!(num));
            } else if v_trim == "true" || v_trim == "false" {
                enum_vals.push(json!(v_trim == "true"));
            } else {
                enum_vals.push(Value::String(v_trim.to_string()));
            }
        }
        return Ok(json!({ "enum": enum_vals }));
    }

    // Array check: [T]
    if trimmed.starts_with('[') && trimmed.ends_with(']') && trimmed.len() >= 2 {
        let inner = safe_slice(trimmed, 1, trimmed.len() - 1).trim();
        let items_schema = if inner.is_empty() || inner == "any" {
            json!({})
        } else {
            parse_single_type(inner)?
        };
        return Ok(json!({
            "type": "array",
            "items": items_schema
        }));
    }

    // Object check: {k: T, ...}
    if trimmed.starts_with('{') && trimmed.ends_with('}') && trimmed.len() >= 2 {
        let inner = safe_slice(trimmed, 1, trimmed.len() - 1).trim();
        if inner.is_empty() {
            return Ok(json!({
                "type": "object",
                "properties": {}
            }));
        }
        let (properties, required) = parse_parameter_list(inner)?;
        let mut obj = Map::new();
        obj.insert("type".to_string(), Value::String("object".to_string()));
        obj.insert("properties".to_string(), Value::Object(properties));
        if !required.is_empty() {
            obj.insert(
                "required".to_string(),
                Value::Array(required.into_iter().map(Value::String).collect()),
            );
        }
        return Ok(Value::Object(obj));
    }

    // String formats
    match trimmed {
        "str" | "string" => return Ok(json!({ "type": "string" })),
        "datetime" => return Ok(json!({ "type": "string", "format": "date-time" })),
        "date" => return Ok(json!({ "type": "string", "format": "date" })),
        "time" => return Ok(json!({ "type": "string", "format": "time" })),
        "email" => return Ok(json!({ "type": "string", "format": "email" })),
        "uri" => return Ok(json!({ "type": "string", "format": "uri" })),
        "uuid" => return Ok(json!({ "type": "string", "format": "uuid" })),
        "int" | "integer" => return Ok(json!({ "type": "integer" })),
        "num" | "number" => return Ok(json!({ "type": "number" })),
        "bool" | "boolean" => return Ok(json!({ "type": "boolean" })),
        "null" => return Ok(json!({ "type": "null" })),
        "any" => return Ok(json!({})),
        _ => {}
    }

    // String with pattern or bounds: str(/pattern/) or str(len 1..80)
    if let Some(inner) = trimmed
        .strip_prefix("str(")
        .and_then(|s| s.strip_suffix(')'))
    {
        let mut obj = Map::new();
        obj.insert("type".to_string(), Value::String("string".to_string()));
        let inner_trim = inner.trim();
        if inner_trim.starts_with('/') && inner_trim.ends_with('/') && inner_trim.len() >= 2 {
            let pat = safe_slice(inner_trim, 1, inner_trim.len() - 1);
            obj.insert("pattern".to_string(), Value::String(pat.to_string()));
        } else if let Some(len_part) = inner_trim.strip_prefix("len") {
            parse_bounds_into(&mut obj, len_part.trim(), "minLength", "maxLength")?;
        }
        return Ok(Value::Object(obj));
    }

    // Integer bounds: int(1..60)
    if let Some(inner) = trimmed
        .strip_prefix("int(")
        .and_then(|s| s.strip_suffix(')'))
    {
        let mut obj = Map::new();
        obj.insert("type".to_string(), Value::String("integer".to_string()));
        parse_bounds_into(&mut obj, inner.trim(), "minimum", "maximum")?;
        return Ok(Value::Object(obj));
    }

    // Number bounds: num(0.0..1.0)
    if let Some(inner) = trimmed
        .strip_prefix("num(")
        .and_then(|s| s.strip_suffix(')'))
    {
        let mut obj = Map::new();
        obj.insert("type".to_string(), Value::String("number".to_string()));
        parse_bounds_into(&mut obj, inner.trim(), "minimum", "maximum")?;
        return Ok(Value::Object(obj));
    }

    Ok(json!({ "type": trimmed }))
}

fn parse_bounds_into(
    obj: &mut Map<String, Value>,
    bounds: &str,
    min_key: &str,
    max_key: &str,
) -> Result<(), CompactError> {
    let b = bounds.trim();
    if let Some(idx) = b.find("..") {
        let left = safe_slice_to(b, idx).trim();
        let right = safe_slice_from(b, idx + 2).trim();
        if !left.is_empty() {
            if let Ok(n) = left.parse::<i64>() {
                obj.insert(min_key.to_string(), json!(n));
            } else if let Ok(f) = left.parse::<f64>() {
                obj.insert(min_key.to_string(), json!(f));
            }
        }
        if !right.is_empty() {
            if let Ok(n) = right.parse::<i64>() {
                obj.insert(max_key.to_string(), json!(n));
            } else if let Ok(f) = right.parse::<f64>() {
                obj.insert(max_key.to_string(), json!(f));
            }
        }
    } else if let Some(stripped) = b.strip_prefix(">=") {
        let val = stripped.trim();
        if let Ok(n) = val.parse::<i64>() {
            obj.insert(min_key.to_string(), json!(n));
        } else if let Ok(f) = val.parse::<f64>() {
            obj.insert(min_key.to_string(), json!(f));
        }
    } else if let Some(stripped) = b.strip_prefix("<=") {
        let val = stripped.trim();
        if let Ok(n) = val.parse::<i64>() {
            obj.insert(max_key.to_string(), json!(n));
        } else if let Ok(f) = val.parse::<f64>() {
            obj.insert(max_key.to_string(), json!(f));
        }
    }
    Ok(())
}

fn find_matching_close_paren(s: &str) -> Option<usize> {
    let mut depth = 0;
    let mut in_str = false;
    let mut in_escape = false;

    for (i, c) in s.char_indices() {
        if in_escape {
            in_escape = false;
            continue;
        }
        if c == '\\' && in_str {
            in_escape = true;
            continue;
        }
        if c == '"' {
            in_str = !in_str;
            continue;
        }
        if in_str {
            continue;
        }

        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

fn find_top_level_equal(s: &str) -> Option<usize> {
    let mut paren_depth = 0;
    let mut brace_depth = 0;
    let mut bracket_depth = 0;
    let mut in_str = false;
    let mut in_escape = false;

    for (i, c) in s.char_indices() {
        if in_escape {
            in_escape = false;
            continue;
        }
        if c == '\\' && in_str {
            in_escape = true;
            continue;
        }
        if c == '"' {
            in_str = !in_str;
            continue;
        }
        if in_str {
            continue;
        }

        match c {
            '(' => paren_depth += 1,
            ')' => paren_depth -= 1,
            '{' => brace_depth += 1,
            '}' => brace_depth -= 1,
            '[' => bracket_depth += 1,
            ']' => bracket_depth -= 1,
            '=' if paren_depth == 0 && brace_depth == 0 && bracket_depth == 0 => {
                if i > 0 {
                    let prev_slice = safe_slice_to(s, i);
                    let prev = prev_slice.chars().last();
                    if prev == Some('>') || prev == Some('<') {
                        continue;
                    }
                }
                return Some(i);
            }
            _ => {}
        }
    }
    None
}

fn split_top_level(s: &str, delimiter: char) -> Result<Vec<String>, CompactError> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut paren_depth = 0;
    let mut brace_depth = 0;
    let mut bracket_depth = 0;
    let mut in_str = false;
    let mut in_escape = false;
    let mut in_comment = false;

    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];

        if in_comment {
            current.push(c);
            if c == '*' && i + 1 < chars.len() && chars[i + 1] == '/' {
                current.push('/');
                i += 2;
                in_comment = false;
                continue;
            }
            i += 1;
            continue;
        }

        if !in_str && c == '/' && i + 1 < chars.len() && chars[i + 1] == '*' {
            current.push('/');
            current.push('*');
            i += 2;
            in_comment = true;
            continue;
        }

        if in_escape {
            current.push(c);
            in_escape = false;
            i += 1;
            continue;
        }

        if c == '\\' && in_str {
            current.push(c);
            in_escape = true;
            i += 1;
            continue;
        }

        if c == '"' {
            current.push(c);
            in_str = !in_str;
            i += 1;
            continue;
        }

        if in_str {
            current.push(c);
            i += 1;
            continue;
        }

        match c {
            '(' => paren_depth += 1,
            ')' => paren_depth -= 1,
            '{' => brace_depth += 1,
            '}' => brace_depth -= 1,
            '[' => bracket_depth += 1,
            ']' => bracket_depth -= 1,
            _ => {}
        }

        if c == delimiter && paren_depth == 0 && brace_depth == 0 && bracket_depth == 0 {
            tokens.push(current.trim().to_string());
            current.clear();
        } else {
            current.push(c);
        }
        i += 1;
    }

    if !current.trim().is_empty() {
        tokens.push(current.trim().to_string());
    }

    Ok(tokens)
}

fn parse_json_or_literal(s: &str) -> Value {
    if let Ok(val) = serde_json::from_str::<Value>(s) {
        val
    } else if (s.starts_with('"') && s.ends_with('"')) || (s.starts_with('\'') && s.ends_with('\''))
    {
        if s.len() >= 2 {
            Value::String(unescape_string(safe_slice(s, 1, s.len() - 1)))
        } else {
            Value::String(String::new())
        }
    } else {
        Value::String(s.to_string())
    }
}

fn unescape_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_escape = false;
    for c in s.chars() {
        if in_escape {
            match c {
                'n' => out.push('\n'),
                'r' => out.push('\r'),
                't' => out.push('\t'),
                '\\' => out.push('\\'),
                '"' => out.push('"'),
                '\'' => out.push('\''),
                _ => {
                    out.push('\\');
                    out.push(c);
                }
            }
            in_escape = false;
        } else if c == '\\' {
            in_escape = true;
        } else {
            out.push(c);
        }
    }
    out
}

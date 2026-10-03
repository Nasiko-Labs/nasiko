//! Tool schema encoding into compact signatures.

use serde_json::Value;
use std::fmt::Write;

use crate::slice::safe_slice_to;
use crate::types::{CompactError, CompactTools, InstructionVariant, ToolDef};

/// Encode a slice of tool definitions into compact signatures and instructions.
pub fn encode_tools(tools: &[ToolDef]) -> Result<CompactTools, CompactError> {
    encode_tools_with_variant(tools, InstructionVariant::default())
}

/// Encode tools with a specific instruction variant.
pub fn encode_tools_with_variant(
    tools: &[ToolDef],
    variant: InstructionVariant,
) -> Result<CompactTools, CompactError> {
    if tools.is_empty() {
        return Ok(CompactTools {
            text: String::new(),
            tool_names: Vec::new(),
            instructions: String::new(),
            full_prompt: String::new(),
        });
    }

    let mut tool_names = Vec::with_capacity(tools.len());
    let mut signatures = Vec::with_capacity(tools.len());

    for tool in tools {
        // Validate unsupported features
        if let Some(params) = &tool.parameters {
            check_unsupported(&tool.name, params)?;
        }

        tool_names.push(tool.name.clone());
        let sig = render_tool_signature(tool)?;
        signatures.push(sig);
    }

    let text = signatures.join("\n");
    let instructions = variant.text().to_string();
    let full_prompt = format!("{instructions}\n\n[Tools]\n{text}");

    Ok(CompactTools {
        text,
        tool_names,
        instructions,
        full_prompt,
    })
}

/// Render a single tool definition as a compact signature:
/// `name(p:type, q?:type, ...) - description`
pub fn render_tool_signature(tool: &ToolDef) -> Result<String, CompactError> {
    let mut out = String::new();
    out.push_str(&tool.name);
    out.push('(');

    if let Some(params) = &tool.parameters {
        render_parameters(&mut out, &tool.name, params)?;
    }

    out.push(')');

    if let Some(desc) = &tool.description {
        let first_sentence = extract_first_sentence(desc);
        if !first_sentence.is_empty() {
            out.push_str(" - ");
            out.push_str(&first_sentence);
        }
    }

    Ok(out)
}

fn render_parameters(
    out: &mut String,
    tool_name: &str,
    params: &Value,
) -> Result<(), CompactError> {
    let Some(obj) = params.as_object() else {
        return Ok(());
    };

    let properties = match obj.get("properties").and_then(Value::as_object) {
        Some(props) => props,
        None => return Ok(()),
    };

    let required: Vec<&str> = obj
        .get("required")
        .and_then(Value::as_array)
        .map(|arr| arr.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();

    let mut first = true;
    for (prop_name, prop_schema) in properties {
        if !first {
            out.push_str(", ");
        }
        first = false;

        out.push_str(prop_name);
        let is_required = required.contains(&prop_name.as_str());
        if !is_required {
            out.push('?');
        }
        out.push(':');

        render_type(out, tool_name, prop_name, prop_schema)?;

        // Default value
        if let Some(default_val) = prop_schema.get("default") {
            out.push('=');
            render_json_compact(out, default_val);
        }

        // Parameter description (if informative)
        if let Some(desc) = prop_schema.get("description").and_then(Value::as_str) {
            let type_str = prop_schema
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or("");
            if should_keep_param_desc(prop_name, type_str, desc) {
                let cleaned_desc = normalize_whitespace(desc);
                let _ = write!(out, " /* {cleaned_desc} */");
            }
        }
    }

    Ok(())
}

fn render_type(
    out: &mut String,
    tool_name: &str,
    prop_name: &str,
    schema: &Value,
) -> Result<(), CompactError> {
    // Check const
    if let Some(c) = schema.get("const") {
        out.push('=');
        render_json_compact(out, c);
        return Ok(());
    }

    // Check enum
    if let Some(enum_vals) = schema.get("enum").and_then(Value::as_array) {
        let mut first = true;
        for val in enum_vals {
            if !first {
                out.push('|');
            }
            first = false;
            render_enum_variant(out, val);
        }
        return Ok(());
    }

    // Handle anyOf / oneOf for nullable or primitive union
    if let Some(unions) = schema
        .get("anyOf")
        .or_else(|| schema.get("oneOf"))
        .and_then(Value::as_array)
    {
        let mut first = true;
        for sub in unions {
            if !first {
                out.push('|');
            }
            first = false;
            render_type(out, tool_name, prop_name, sub)?;
        }
        return Ok(());
    }

    let type_val = schema.get("type");
    match type_val {
        Some(Value::String(s)) => match s.as_str() {
            "string" => render_string_type(out, schema),
            "integer" => render_int_type(out, schema),
            "number" => render_num_type(out, schema),
            "boolean" => out.push_str("bool"),
            "null" => out.push_str("null"),
            "array" => render_array_type(out, tool_name, prop_name, schema)?,
            "object" => render_object_type(out, tool_name, schema)?,
            _ => out.push_str(s),
        },
        Some(Value::Array(types)) => {
            // Union of types, e.g. ["string", "null"]
            let mut first = true;
            for t in types {
                if let Some(s) = t.as_str() {
                    if !first {
                        out.push('|');
                    }
                    first = false;
                    match s {
                        "string" => out.push_str("str"),
                        "integer" => out.push_str("int"),
                        "number" => out.push_str("num"),
                        "boolean" => out.push_str("bool"),
                        "null" => out.push_str("null"),
                        _ => out.push_str(s),
                    }
                }
            }
        }
        _ => out.push_str("any"),
    }

    Ok(())
}

fn render_string_type(out: &mut String, schema: &Value) {
    if let Some(format) = schema.get("format").and_then(Value::as_str) {
        match format {
            "date-time" => {
                out.push_str("datetime");
                return;
            }
            "date" | "time" | "email" | "uri" | "uuid" => {
                out.push_str(format);
                return;
            }
            _ => {}
        }
    }

    if let Some(pattern) = schema.get("pattern").and_then(Value::as_str) {
        let _ = write!(out, "str(/{pattern}/)");
        return;
    }

    let min_len = schema.get("minLength").and_then(Value::as_i64);
    let max_len = schema.get("maxLength").and_then(Value::as_i64);

    match (min_len, max_len) {
        (Some(min), Some(max)) => {
            let _ = write!(out, "str(len {min}..{max})");
        }
        (Some(min), None) => {
            let _ = write!(out, "str(len >={min})");
        }
        (None, Some(max)) => {
            let _ = write!(out, "str(len <={max})");
        }
        (None, None) => {
            out.push_str("str");
        }
    }
}

fn render_int_type(out: &mut String, schema: &Value) {
    let min = schema.get("minimum").and_then(Value::as_i64);
    let max = schema.get("maximum").and_then(Value::as_i64);

    match (min, max) {
        (Some(min), Some(max)) => {
            let _ = write!(out, "int({min}..{max})");
        }
        (Some(min), None) => {
            let _ = write!(out, "int(>={min})");
        }
        (None, Some(max)) => {
            let _ = write!(out, "int(<={max})");
        }
        (None, None) => {
            out.push_str("int");
        }
    }
}

fn render_num_type(out: &mut String, schema: &Value) {
    let min = schema.get("minimum").and_then(Value::as_f64);
    let max = schema.get("maximum").and_then(Value::as_f64);

    match (min, max) {
        (Some(min), Some(max)) => {
            let _ = write!(out, "num({min}..{max})");
        }
        (Some(min), None) => {
            let _ = write!(out, "num(>={min})");
        }
        (None, Some(max)) => {
            let _ = write!(out, "num(<={max})");
        }
        (None, None) => {
            out.push_str("num");
        }
    }
}

fn render_array_type(
    out: &mut String,
    tool_name: &str,
    prop_name: &str,
    schema: &Value,
) -> Result<(), CompactError> {
    out.push('[');
    if let Some(items) = schema.get("items") {
        render_type(out, tool_name, prop_name, items)?;
    } else {
        out.push_str("any");
    }
    out.push(']');
    Ok(())
}

fn render_object_type(
    out: &mut String,
    tool_name: &str,
    schema: &Value,
) -> Result<(), CompactError> {
    out.push('{');
    if let Some(props) = schema.get("properties").and_then(Value::as_object) {
        let required: Vec<&str> = schema
            .get("required")
            .and_then(Value::as_array)
            .map(|arr| arr.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();

        let mut first = true;
        for (k, v) in props {
            if !first {
                out.push_str(", ");
            }
            first = false;
            out.push_str(k);
            if !required.contains(&k.as_str()) {
                out.push('?');
            }
            out.push(':');
            render_type(out, tool_name, k, v)?;
        }
    }
    out.push('}');
    Ok(())
}

fn render_enum_variant(out: &mut String, val: &Value) {
    match val {
        Value::String(s) => {
            if s.contains('|')
                || s.contains(' ')
                || s.contains(',')
                || s.contains('(')
                || s.contains(')')
                || s.contains('"')
                || s.contains('\'')
                || s.is_empty()
            {
                out.push('"');
                for c in s.chars() {
                    if c == '"' || c == '\\' {
                        out.push('\\');
                    }
                    out.push(c);
                }
                out.push('"');
            } else {
                out.push_str(s);
            }
        }
        _ => render_json_compact(out, val),
    }
}

fn render_json_compact(out: &mut String, val: &Value) {
    match val {
        Value::String(s) => {
            out.push('"');
            out.push_str(s);
            out.push('"');
        }
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => {
            let _ = write!(out, "{n}");
        }
        Value::Null => out.push_str("null"),
        Value::Array(arr) => {
            out.push('[');
            for (i, v) in arr.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                render_json_compact(out, v);
            }
            out.push(']');
        }
        Value::Object(obj) => {
            out.push('{');
            for (i, (k, v)) in obj.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(k);
                out.push(':');
                render_json_compact(out, v);
            }
            out.push('}');
        }
    }
}

/// Extract first sentence from docstring, normalizing whitespace.
pub fn extract_first_sentence(text: &str) -> String {
    let normalized = normalize_whitespace(text);
    if let Some(idx) = normalized.find(". ") {
        safe_slice_to(&normalized, idx + 1).to_string()
    } else if let Some(idx) = normalized.find('.') {
        if idx == normalized.len() - 1 {
            normalized
        } else {
            safe_slice_to(&normalized, idx + 1).to_string()
        }
    } else {
        normalized
    }
}

/// Check if parameter description adds semantic information beyond name & type.
pub fn should_keep_param_desc(name: &str, type_str: &str, desc: &str) -> bool {
    let lower_desc = desc.trim().to_ascii_lowercase();
    let lower_name = name.trim().to_ascii_lowercase();

    // Check trivial descriptions
    if lower_desc == lower_name
        || lower_desc == format!("the {lower_name}")
        || lower_desc == format!("{lower_name} to use")
        || lower_desc == type_str
        || lower_desc == format!("the {type_str}")
        || lower_desc.is_empty()
    {
        return false;
    }

    // Always keep if contains distinguishing information (units, constraints, formats, examples)
    let indicators = [
        "celsius",
        "fahrenheit",
        "kelvin",
        "seconds",
        "minutes",
        "hours",
        "days",
        "ms",
        "bytes",
        "kb",
        "mb",
        "gb",
        "iso",
        "rfc",
        "format",
        "comma",
        "separated",
        "url",
        "http",
        "id",
        "slug",
        "email",
        "example",
        "e.g.",
        "such as",
        "must",
        "range",
        "between",
        "unique",
        "regex",
        "timezone",
        "utc",
        "gmt",
    ];

    for ind in indicators {
        if lower_desc.contains(ind) {
            return true;
        }
    }

    // If description is significantly longer and detailed, keep it
    if lower_desc.len() > lower_name.len() + 15 {
        return true;
    }

    false
}

fn normalize_whitespace(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn check_unsupported(tool_name: &str, schema: &Value) -> Result<(), CompactError> {
    if let Value::Object(map) = schema {
        for (k, v) in map {
            match k.as_str() {
                "$ref" => {
                    return Err(CompactError::Unsupported {
                        tool: tool_name.to_string(),
                        feature: "$ref".to_string(),
                    });
                }
                "allOf" => {
                    return Err(CompactError::Unsupported {
                        tool: tool_name.to_string(),
                        feature: "allOf".to_string(),
                    });
                }
                "if" | "then" | "else" => {
                    return Err(CompactError::Unsupported {
                        tool: tool_name.to_string(),
                        feature: "if/then/else".to_string(),
                    });
                }
                "patternProperties" => {
                    return Err(CompactError::Unsupported {
                        tool: tool_name.to_string(),
                        feature: "patternProperties".to_string(),
                    });
                }
                "dependentSchemas" => {
                    return Err(CompactError::Unsupported {
                        tool: tool_name.to_string(),
                        feature: "dependentSchemas".to_string(),
                    });
                }
                "additionalProperties" if v.is_object() => {
                    return Err(CompactError::Unsupported {
                        tool: tool_name.to_string(),
                        feature: "additionalProperties with schema".to_string(),
                    });
                }
                "oneOf" | "anyOf" if !is_simple_union(v) => {
                    return Err(CompactError::Unsupported {
                        tool: tool_name.to_string(),
                        feature: format!("{k} (complex schema union)"),
                    });
                }
                _ => {}
            }
            check_unsupported(tool_name, v)?;
        }
    } else if let Value::Array(arr) = schema {
        for item in arr {
            check_unsupported(tool_name, item)?;
        }
    }
    Ok(())
}

fn is_simple_union(val: &Value) -> bool {
    let Some(arr) = val.as_array() else {
        return false;
    };
    for item in arr {
        let Some(obj) = item.as_object() else {
            return false;
        };
        if obj.len() != 1 || !obj.contains_key("type") {
            return false;
        }
    }
    true
}

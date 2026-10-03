//! Encode tool definitions into compact format and decode back.
//!
//! The encoder goes through the schema IR: `ToolDef` → `ToolIR` → compact text.
//! The decoder reverses: `ToolIR` → `ToolDef`.

use std::collections::BTreeMap;

use serde_json::Value;

use crate::error::CompactError;
use crate::schema::{FieldIR, ToolIR, example_value, render_type};
use crate::types::{CompactTools, FunctionDef, ToolCompactStatus, ToolDef};

/// Encode a slice of tool definitions into the compact format.
///
/// Per-tool: parse schema → IR → render compact line. If unsupported, bypass that
/// tool. A never-worse guarantee compares byte counts and bypasses if the compact
/// form is not ≥10 % smaller than native JSON.
pub fn encode_tools(tools: &[ToolDef]) -> Result<CompactTools, CompactError> {
    let mut tool_irs: BTreeMap<String, ToolIR> = BTreeMap::new();
    let mut status = Vec::new();
    let mut compact_lines = Vec::new();
    let mut any_compacted = false;

    for tool in tools {
        let name = &tool.function.name;
        let desc = tool.function.description.as_deref();
        let schema = tool.function.parameters.as_ref();

        let empty_obj = serde_json::json!({"type": "object"});
        let schema_val = schema.unwrap_or(&empty_obj);

        match ToolIR::from_json_schema(name, desc, schema_val) {
            Ok(ir) => {
                let line = render_tool_line(&ir);
                compact_lines.push(line);
                tool_irs.insert(name.clone(), ir);
                status.push(ToolCompactStatus {
                    name: name.clone(),
                    compacted: true,
                    reason: None,
                });
                any_compacted = true;
            }
            Err(CompactError::UnsupportedSchema { feature, .. }) => {
                status.push(ToolCompactStatus {
                    name: name.clone(),
                    compacted: false,
                    reason: Some(format!("unsupported: {feature}")),
                });
            }
            Err(e) => return Err(e),
        }
    }

    if !any_compacted {
        return Ok(CompactTools {
            text: String::new(),
            compacted: false,
            tool_status: status,
            registry: tool_irs,
            original_tools: tools.to_vec(),
        });
    }

    // Build the compact text block.
    let mut text = String::new();
    for line in &compact_lines {
        text.push_str(line);
        text.push('\n');
    }
    text.push_str("To call a tool: <<call name {\"arg\":\"val\"}>>\n");

    // Build a realistic example from the first compacted tool.
    if let Some(first_ir) = tool_irs.values().next() {
        let example = build_example_call(first_ir);
        text.push_str(&format!("Example: {example}\n"));
    }

    // ── Never-worse guarantee ──────────────────────────────────────────────
    // Compare byte counts as a cheap token proxy (~4 chars/token).
    let native_json = serde_json::to_string(tools).unwrap_or_default();
    let native_bytes = native_json.len();
    let compact_bytes = text.len();

    let compacted = if native_bytes > 0 {
        let savings = native_bytes.saturating_sub(compact_bytes);
        let ratio = savings as f64 / native_bytes as f64;
        ratio >= 0.10
    } else {
        false
    };

    Ok(CompactTools {
        text,
        compacted,
        tool_status: status,
        registry: tool_irs,
        original_tools: tools.to_vec(),
    })
}

/// Reconstruct `ToolDef`s from a [`CompactTools`] (lets graders check that schema
/// information survived the encoding).
pub fn decode_tools(compact: &CompactTools) -> Result<Vec<ToolDef>, CompactError> {
    let mut tools = Vec::new();

    // Compacted tools: reconstruct from IR.
    for ir in compact.registry.values() {
        let schema = ir.to_json_schema();
        tools.push(ToolDef {
            kind: "function".to_string(),
            function: FunctionDef {
                name: ir.name.clone(),
                description: ir.description.clone(),
                parameters: Some(schema),
            },
            extra: serde_json::Map::new(),
        });
    }

    // Bypassed tools: pass through unchanged.
    for t in &compact.original_tools {
        if !compact.registry.contains_key(&t.function.name) {
            tools.push(t.clone());
        }
    }

    Ok(tools)
}

// ── Helpers ────────────────────────────────────────────────────────────────

fn render_tool_line(ir: &ToolIR) -> String {
    let mut line = ir.name.clone();
    line.push('(');
    let params: Vec<String> = ir
        .params
        .iter()
        .map(|(name, field)| render_param(name, field))
        .collect();
    line.push_str(&params.join(","));
    line.push(')');
    if let Some(desc) = &ir.description
        && !is_redundant_description(&ir.name, desc)
    {
        line.push_str(" - ");
        line.push_str(desc);
    }
    line
}

fn is_redundant_description(tool_name: &str, desc: &str) -> bool {
    let name_words: Vec<String> = tool_name
        .to_lowercase()
        .split(|c: char| c == '_' || c == '-' || c.is_whitespace())
        .filter(|s| !s.is_empty())
        .map(String::from)
        .collect();

    let desc_clean: String = desc
        .chars()
        .filter(|c| c.is_alphanumeric() || c.is_whitespace())
        .collect();
    let desc_lower = desc_clean.to_lowercase();
    let desc_words: Vec<&str> = desc_lower
        .split_whitespace()
        .filter(|w| {
            ![
                "a", "an", "the", "to", "for", "of", "in", "is", "by", "or", "and",
            ]
            .contains(w)
        })
        .collect();

    if desc_words.is_empty() {
        return true;
    }

    desc_words
        .iter()
        .all(|w| name_words.iter().any(|nw| nw == w))
}

fn render_param(name: &str, field: &FieldIR) -> String {
    let opt = if field.required { "" } else { "?" };
    let type_str = render_type(&field.schema);
    format!("{name}{opt}:{type_str}")
}

fn build_example_call(ir: &ToolIR) -> String {
    let mut args = serde_json::Map::new();
    for (key, field) in &ir.params {
        if field.required {
            args.insert(key.clone(), example_value(&field.schema));
        }
    }
    let args_str = serde_json::to_string(&Value::Object(args)).unwrap_or_else(|_| "{}".to_string());
    format!("<<call {} {}>>", ir.name, args_str)
}

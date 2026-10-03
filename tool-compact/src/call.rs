//! Rendering one call in the `<<call …>>` grammar, and validating a call that arrived in some
//! other representation (for example a provider's native tool call) against the same schemas.

use serde_json::Value;

use crate::catalog::Catalog;
use crate::error::{Result, ToolCompactError};
use crate::json::{canonical_json, parse_object_unique};
use crate::limits::{MAX_ARGS_BYTES, MAX_CALLS, MAX_TOTAL_ARGS_BYTES};
use crate::types::{ToolCall, ToolDef};
use crate::validate::validate;

/// Render `<<call name {args}>>` with canonical (sorted-key) JSON. This is the only renderer of
/// the call grammar, so anything that must produce it (evaluation fixtures, documentation
/// examples) cannot drift from what the decoder accepts.
pub fn encode_call(name: &str, arguments: &Value) -> String {
    format!("<<call {name} {}>>", canonical_json(arguments))
}

/// Validate a call given as a tool name and a JSON-text argument object.
///
/// Same rules and limits as the decoder: arguments above `MAX_ARGS_BYTES` → `limit_exceeded`
/// before anything is parsed; unknown name → `unknown_tool`; malformed JSON or duplicate keys →
/// `malformed_call`; schema violation → `invalid_arguments`. An unsupported catalog is an error
/// here too, never a reason to skip validation.
pub fn validate_call(name: &str, arguments_json: &str, tools: &[ToolDef]) -> Result<ToolCall> {
    let catalog = Catalog::compile(tools)?;
    validate_in(&catalog, name, arguments_json)
}

/// Validate a whole batch of calls that arrived together (a provider's native `tool_calls`).
///
/// The batch limits the stream decoder enforces apply here too, and before any call is parsed:
/// more than `MAX_CALLS` calls or more than `MAX_TOTAL_ARGS_BYTES` of arguments in total →
/// `limit_exceeded`. Atomic: any failing call fails the batch and nothing is returned.
pub fn validate_calls(calls: &[(&str, &str)], tools: &[ToolDef]) -> Result<Vec<ToolCall>> {
    if calls.len() > MAX_CALLS {
        return Err(ToolCompactError::limit("MAX_CALLS", MAX_CALLS));
    }
    let total: usize = calls
        .iter()
        .fold(0usize, |acc, (_, args)| acc.saturating_add(args.len()));
    if total > MAX_TOTAL_ARGS_BYTES {
        return Err(ToolCompactError::limit(
            "MAX_TOTAL_ARGS_BYTES",
            MAX_TOTAL_ARGS_BYTES,
        ));
    }
    let catalog = Catalog::compile(tools)?;
    calls
        .iter()
        .map(|(name, args)| validate_in(&catalog, name, args))
        .collect()
}

pub(crate) fn validate_in(catalog: &Catalog, name: &str, arguments_json: &str) -> Result<ToolCall> {
    if arguments_json.len() > MAX_ARGS_BYTES {
        return Err(ToolCompactError::limit("MAX_ARGS_BYTES", MAX_ARGS_BYTES));
    }
    let Some(tool) = catalog.get(name) else {
        return Err(ToolCompactError::UnknownTool {
            name: name.to_owned(),
        });
    };
    let map = parse_object_unique(arguments_json)
        .map_err(|e| ToolCompactError::malformed(e.to_string()))?;
    let value = Value::Object(map);
    if let Some(schema) = &tool.schema {
        validate(schema, &value, "").map_err(|v| ToolCompactError::InvalidArguments {
            tool: name.to_owned(),
            path: v.path,
            reason: v.reason,
        })?;
    }
    Ok(ToolCall {
        name: name.to_owned(),
        arguments: value,
    })
}

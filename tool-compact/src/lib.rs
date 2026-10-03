//! Compact tool schemas without breaking tool calls.
//!
//! A pure, deterministic, fail-closed library that shrinks the prompt-token cost of
//! OpenAI-style function-tool definitions while preserving the exact tool names and
//! validated arguments returned to clients.
//!
//! # Grammar
//!
//! ## Compact tool block (`encode_tools` output)
//!
//! ```text
//! Tools:
//! name(param[, param...]) [- description]
//! ...
//! Call tools as <<call name {json args}>> ...
//! ```
//!
//! One line per tool, in input order. A parameter is:
//!
//! ```text
//! field_name[?] : type
//! ```
//!
//! `?` marks optional fields (absent from the schema's `required` list). No `?`
//! means required.
//!
//! Field descriptions are intentionally elided: names plus types
//! (`title:str`, `start:datetime`, `attendees?:[str]`) already carry the
//! meaning, matching the brief's illustrative format. Tool descriptions are
//! always kept verbatim after ` - `, since they disambiguate tools with
//! similar names. (The decoder still *accepts* a trailing `"description"`
//! suffix for compatibility, but the encoder never emits one.)
//!
//! Types:
//!
//! ```text
//! type    := "str" | "datetime" | "date" | "int" | "num" | "bool"
//!          | enum | array | object
//! enum    := value ("|" value)*        // string enums, simple values only
//! array   := "[" type "]"
//! object  := "{" [param (", " param)*] "}"
//! ```
//!
//! - `str` is a plain JSON string; `datetime` is a string with
//!   `format: date-time`; `date` is a string with `format: date`. The format
//!   is carried as a *type* only: the decoder checks the JSON type (string),
//!   not the value's shape.
//! - `int` is a JSON integer (no fractions, no strings); `num` is any JSON
//!   number; `bool` is a JSON boolean.
//! - `enum` lists the allowed string values joined by `|`
//!   (e.g. `public|private`).
//! - Arrays render their item type (`[str]`); nested objects render inline
//!   (`{street:str, zip?:str}`).
//!
//! The block ends with [`CALL_INSTRUCTIONS`], which states the call format, the
//! validation rules, and the fixed reference time (`2026-10-02`,
//! `Asia/Kolkata`) so relative dates resolve identically for every runner.
//!
//! ## Call grammar (model output)
//!
//! ```text
//! output := (text | call)*
//! call   := "<<call" SP tool_name SP json_object ">>"
//! tool_name := [A-Za-z0-9_.-]+
//! json_object := a balanced JSON object; strings/escapes are respected so a
//!                ">>" inside a string never terminates the call
//! ```
//!
//! Rules enforced by [`decode_calls`]:
//!
//! - Text before, between, or after calls is ignored; zero calls is a valid
//!   plain answer (`Ok(vec![])`).
//! - The tool name must exactly match one of the supplied tools, else
//!   [`CompactError::UnknownTool`].
//! - Arguments must be a JSON object satisfying the tool's schema (required
//!   fields present, types correct, enums exact), else
//!   [`CompactError::InvalidArguments`]. Nothing is guessed or coerced.
//! - A `<<call` marker that cannot be parsed (bad name, bad JSON, missing
//!   `>>`) is [`CompactError::Malformed`], never a skipped call.
//!
//! # Fail-closed scope
//!
//! Schemas using features outside the grammar bypass compaction instead of
//! being approximated. [`encode_tools`] returns [`CompactTools`] with
//! `compacted == false` and a `bypass_reason` when it meets:
//!
//! - `$ref`, `oneOf`, `anyOf` (beyond a nullable `["T","null"]`), `allOf`,
//!   `not`, `if`/`then`/`else`, `patternProperties`, `propertyNames`
//! - string `format` other than `date-time`/`date` (a format like `email` or
//!   `uuid` carries validity semantics the decoder does not check, so it
//!   bypasses rather than silently widening what counts as valid)
//! - `pattern` and numeric/length constraints (`multipleOf`, `minLength`,
//!   `minimum`, …, `uniqueItems`) are accepted but *not enforced* by the
//!   decoder — see rationale below
//! - non-string enums, enum values outside `[A-Za-z0-9_.-]+`
//! - `type` unions other than `["T","null"]`
//! - array schemas without `items`, or `items` as a list
//! - `prefixItems`, `contains`, `additionalProperties` as a schema object
//!   (a boolean `additionalProperties` is accepted and ignored for
//!   validation; the canonical schema round-trips without it)
//! - `const`, `required` entries without a matching property, property names
//!   outside `[A-Za-z0-9_.-]+`
//!
//! Rationale: length/range constraints (`minLength`, `minimum`, …) and
//! `pattern` are documented here as *not enforced* by the decoder. They are
//! accepted (not a bypass) because ignoring them cannot invent a call — the
//! decoder still requires correct names, presence, types, and enum values.
//! Anything that would change *which* calls are valid bypasses instead.
//!
//! # Determinism
//!
//! Pure string processing only: no clock, no RNG, no I/O, no environment
//! reads. Identical input yields byte-identical output.

#![forbid(unsafe_code)]
#![deny(
    clippy::string_slice,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic
)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Fixed reference time note appended to every compact block so relative dates
/// ("Monday 3pm") resolve identically for every runner.
pub const REFERENCE_TIME_NOTE: &str =
    "Today: 2026-10-02 Asia/Kolkata; resolve relative dates to it.";

/// Instructions appended after the compact tool lines.
pub const CALL_INSTRUCTIONS: &str = "Call tools as <<call name {json args}>> (multiple allowed; no marker = no call). Use exact names; include all required fields with correct types and enum values; never guess.";

/// A tool definition owned by this crate (independent of any router IR).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolDef {
    /// Function name, e.g. `create_calendar_event`.
    pub name: String,
    /// Tool description, if the schema provided one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// JSON Schema for the arguments (`type: object` with `properties`).
    /// `None` means "no parameters".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parameters: Option<Value>,
}

/// A decoded tool call: validated arguments as parsed JSON.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    /// Exact tool name from the schema.
    pub name: String,
    /// Validated argument object.
    pub arguments: Value,
}

impl ToolCall {
    /// Arguments serialized as a JSON string (the OpenAI `function.arguments`
    /// contract). Keys serialize in the decoded object's order.
    pub fn arguments_json(&self) -> String {
        serde_json::to_string(&self.arguments).unwrap_or_else(|_| "{}".to_string())
    }
}

/// The result of [`encode_tools`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompactTools {
    /// The compact block to inject (tool lines + instructions + reference
    /// time). Empty when `compacted == false`.
    pub text: String,
    /// Whether compaction applied. `false` means the caller must send the
    /// native schemas untouched.
    pub compacted: bool,
    /// Set when `compacted == false`; names the first unsupported feature met.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bypass_reason: Option<String>,
}

/// All failure modes. Validation failures never produce a guessed call.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CompactError {
    /// The call names a tool that was not supplied.
    #[error("unknown tool: {0}")]
    UnknownTool(String),
    /// Arguments are valid JSON but violate the schema (missing required
    /// field, wrong type, bad enum, …). The detail names the cause.
    #[error("invalid arguments for tool '{tool}': {reason}")]
    InvalidArguments {
        /// Tool whose schema was violated.
        tool: String,
        /// Machine-readable cause (`missing_required`, `wrong_type`,
        /// `invalid_enum`, `not_an_object`, `bad_json`, …).
        reason: String,
    },
    /// A `<<call` marker is present but unparsable (bad name, unbalanced
    /// JSON, missing `>>`, …).
    #[error("malformed tool call: {0}")]
    Malformed(String),
    /// A tool definition itself is invalid (empty name, duplicate name).
    #[error("invalid tool definition: {0}")]
    InvalidToolDef(String),
    /// The schema needs a feature outside the grammar; the caller should
    /// bypass compaction for the request (encode surfaces this as
    /// `compacted == false`, not as an error, except when called directly).
    #[error("unsupported schema for tool '{tool}': {feature}")]
    UnsupportedSchema {
        /// Tool carrying the unsupported feature.
        tool: String,
        /// Feature name (`$ref`, `oneOf`, …).
        feature: String,
    },
}

// ---------------------------------------------------------------------------
// Internal schema model
// ---------------------------------------------------------------------------

/// A field's type in the compact grammar.
#[derive(Debug, Clone, PartialEq, Eq)]
enum FieldType {
    Str,
    Datetime,
    Date,
    Int,
    Num,
    Bool,
    Enum(Vec<String>),
    Array(Box<FieldType>),
    Object(Vec<Field>),
}

/// One parameter.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Field {
    name: String,
    required: bool,
    ty: FieldType,
    description: Option<String>,
}

/// Parsed, validated view of one tool's parameters.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ParsedTool {
    name: String,
    description: Option<String>,
    fields: Vec<Field>,
}

/// True for names the grammars accept: `[A-Za-z0-9_.-]+`.
fn is_simple_name(s: &str) -> bool {
    !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-' || b == b'.')
}

/// True for enum values we render inline (`a|b`). Same charset as names so
/// the call grammar stays unambiguous.
fn is_simple_enum_value(s: &str) -> bool {
    is_simple_name(s)
}

// ---------------------------------------------------------------------------
// Encoding
// ---------------------------------------------------------------------------

/// Encode tool schemas into a compact prompt block.
///
/// Returns `compacted == false` (never an error) when any tool uses a schema
/// feature outside the grammar; the `bypass_reason` names it. Errors only on
/// invalid tool definitions (empty or duplicate names).
pub fn encode_tools(tools: &[ToolDef]) -> Result<CompactTools, CompactError> {
    if tools.is_empty() {
        return Ok(CompactTools {
            text: String::new(),
            compacted: true,
            bypass_reason: None,
        });
    }
    let mut seen = std::collections::HashSet::new();
    for t in tools {
        if t.name.trim().is_empty() {
            return Err(CompactError::InvalidToolDef("empty tool name".to_string()));
        }
        if !is_simple_name(&t.name) {
            return Err(CompactError::InvalidToolDef(format!(
                "tool name {:?} outside [A-Za-z0-9_.-]+",
                t.name
            )));
        }
        if !seen.insert(t.name.as_str()) {
            return Err(CompactError::InvalidToolDef(format!(
                "duplicate tool name {:?}",
                t.name
            )));
        }
    }

    let mut parsed: Vec<ParsedTool> = Vec::with_capacity(tools.len());
    for t in tools {
        match parse_tool_schema(t) {
            Ok(p) => parsed.push(p),
            Err(CompactError::UnsupportedSchema { tool, feature }) => {
                return Ok(CompactTools {
                    text: String::new(),
                    compacted: false,
                    bypass_reason: Some(format!("{tool}: {feature}")),
                });
            }
            Err(e) => return Err(e),
        }
    }

    let mut out = String::from("Tools:\n");
    for p in &parsed {
        out.push_str(&render_tool_line(p));
        out.push('\n');
    }
    out.push_str(CALL_INSTRUCTIONS);
    out.push('\n');
    out.push_str(REFERENCE_TIME_NOTE);
    Ok(CompactTools {
        text: out,
        compacted: true,
        bypass_reason: None,
    })
}

/// Render one tool line: `name(p1, p2) - description`.
fn render_tool_line(tool: &ParsedTool) -> String {
    let mut s = String::new();
    s.push_str(&tool.name);
    s.push('(');
    let parts: Vec<String> = tool.fields.iter().map(render_field).collect();
    s.push_str(&parts.join(", "));
    s.push(')');
    if let Some(d) = tool.description.as_deref() {
        let d = d.trim();
        if !d.is_empty() {
            s.push_str(" - ");
            s.push_str(d);
        }
    }
    s
}

fn render_field(f: &Field) -> String {
    let mut s = String::new();
    s.push_str(&f.name);
    if !f.required {
        s.push('?');
    }
    s.push(':');
    s.push_str(&render_type(&f.ty));
    // Field descriptions are deliberately NOT emitted (see the grammar docs);
    // `f.description` is retained in the model only so decoders that accept
    // the legacy suffix keep working.
    s
}

fn render_type(ty: &FieldType) -> String {
    match ty {
        FieldType::Str => "str".to_string(),
        FieldType::Datetime => "datetime".to_string(),
        FieldType::Date => "date".to_string(),
        FieldType::Int => "int".to_string(),
        FieldType::Num => "num".to_string(),
        FieldType::Bool => "bool".to_string(),
        FieldType::Enum(vals) => vals.join("|"),
        FieldType::Array(inner) => format!("[{}]", render_type(inner)),
        FieldType::Object(fields) => {
            let parts: Vec<String> = fields.iter().map(render_field).collect();
            format!("{{{}}}", parts.join(", "))
        }
    }
}

/// Parse one tool's JSON Schema into the internal model, rejecting
/// unsupported features fail-closed.
fn parse_tool_schema(tool: &ToolDef) -> Result<ParsedTool, CompactError> {
    let unsupported = |feature: &str| CompactError::UnsupportedSchema {
        tool: tool.name.clone(),
        feature: feature.to_string(),
    };
    let Some(params) = tool.parameters.as_ref() else {
        return Ok(ParsedTool {
            name: tool.name.clone(),
            description: tool.description.clone(),
            fields: Vec::new(),
        });
    };
    if params.is_null() {
        return Ok(ParsedTool {
            name: tool.name.clone(),
            description: tool.description.clone(),
            fields: Vec::new(),
        });
    }
    let obj = params
        .as_object()
        .ok_or_else(|| unsupported("parameters must be an object schema"))?;
    // Reject combinators at the top level.
    for key in [
        "$ref",
        "oneOf",
        "anyOf",
        "allOf",
        "not",
        "if",
        "then",
        "else",
        "patternProperties",
        "propertyNames",
    ] {
        if obj.contains_key(key) {
            return Err(unsupported(key));
        }
    }
    if let Some(const_val) = obj.get("const") {
        if !const_val.is_null() {
            return Err(unsupported("const"));
        }
    }
    if let Some(t) = obj.get("type") {
        let ok = match t {
            Value::String(s) => s == "object" || s == "null",
            Value::Array(arr) => {
                // Allow ["object","null"] only.
                let mut kinds: Vec<&str> = arr.iter().filter_map(|v| v.as_str()).collect();
                kinds.sort_unstable();
                kinds == ["null", "object"]
            }
            _ => false,
        };
        if !ok {
            return Err(unsupported("non-object parameters type"));
        }
    }
    // additionalProperties as a schema object changes validity; a boolean is
    // accepted (ignored for validation) so strict-mode schemas still compact.
    if let Some(ap) = obj.get("additionalProperties") {
        if !ap.is_boolean() {
            return Err(unsupported("additionalProperties schema"));
        }
    }
    let required: std::collections::HashSet<String> = match obj.get("required") {
        None | Some(Value::Null) => std::collections::HashSet::new(),
        Some(Value::Array(arr)) => {
            let mut set = std::collections::HashSet::new();
            for v in arr {
                let s = v
                    .as_str()
                    .ok_or_else(|| unsupported("non-string required entry"))?;
                set.insert(s.to_string());
            }
            set
        }
        Some(_) => return Err(unsupported("required must be an array")),
    };
    let props = match obj.get("properties") {
        None | Some(Value::Null) => serde_json::Map::new(),
        Some(Value::Object(m)) => m.clone(),
        Some(_) => return Err(unsupported("properties must be an object")),
    };
    // Every required entry must name a property; otherwise validity is unclear.
    for r in &required {
        if !props.contains_key(r) {
            return Err(unsupported("required without matching property"));
        }
    }
    let mut fields = Vec::with_capacity(props.len());
    for (name, schema) in &props {
        if !is_simple_name(name) {
            return Err(unsupported("property name charset"));
        }
        let ty = parse_field_type(&tool.name, name, schema)?;
        let desc = schema
            .get("description")
            .and_then(Value::as_str)
            .map(str::to_string);
        fields.push(Field {
            name: name.clone(),
            required: required.contains(name),
            ty,
            description: desc,
        });
    }
    Ok(ParsedTool {
        name: tool.name.clone(),
        description: tool.description.clone(),
        fields,
    })
}

/// Parse one property schema. `nullable` allows `["T","null"]`, which folds
/// into optionality handled by the caller.
fn parse_field_type(tool: &str, field: &str, schema: &Value) -> Result<FieldType, CompactError> {
    let unsupported = |feature: &str| CompactError::UnsupportedSchema {
        tool: tool.to_string(),
        feature: format!("{field}: {feature}"),
    };
    let Some(obj) = schema.as_object() else {
        return Err(unsupported("property schema must be an object"));
    };
    for key in [
        "$ref",
        "oneOf",
        "allOf",
        "not",
        "if",
        "then",
        "else",
        "patternProperties",
        "propertyNames",
        "prefixItems",
        "contains",
        "const",
    ] {
        if obj.contains_key(key) {
            return Err(unsupported(key));
        }
    }
    if obj.contains_key("anyOf") {
        return Err(unsupported("anyOf"));
    }
    if let Some(ap) = obj.get("additionalProperties") {
        if !ap.is_boolean() {
            return Err(unsupported("additionalProperties schema"));
        }
    }
    // Nullable `type: ["T","null"]` is allowed; nullability itself is carried
    // by required/optional, so the inner type is what matters.
    let type_str: Option<String> = match obj.get("type") {
        None => None,
        Some(Value::String(s)) => Some(s.clone()),
        Some(Value::Array(arr)) => {
            let mut kinds: Vec<String> = arr
                .iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect();
            if kinds.len() != arr.len() {
                return Err(unsupported("non-string type union member"));
            }
            kinds.sort();
            if kinds.len() == 2 && kinds.contains(&"null".to_string()) {
                let inner = kinds.into_iter().find(|k| k != "null").unwrap_or_default();
                Some(inner)
            } else {
                return Err(unsupported("type union"));
            }
        }
        Some(_) => return Err(unsupported("type must be a string")),
    };

    // Enum takes precedence: a string enum renders inline.
    if let Some(e) = obj.get("enum") {
        let arr = e
            .as_array()
            .ok_or_else(|| unsupported("enum must be an array"))?;
        if arr.is_empty() {
            return Err(unsupported("empty enum"));
        }
        let mut vals = Vec::with_capacity(arr.len());
        for v in arr {
            let s = v.as_str().ok_or_else(|| unsupported("non-string enum"))?;
            if !is_simple_enum_value(s) {
                return Err(unsupported("enum value charset"));
            }
            vals.push(s.to_string());
        }
        return Ok(FieldType::Enum(vals));
    }

    match type_str.as_deref() {
        None => {
            // No explicit type: infer from `properties` (object) or `items`
            // (array), else unsupported — never guess.
            if obj.contains_key("properties") {
                parse_object_inline(tool, field, obj)
            } else if obj.contains_key("items") {
                let items = obj.get("items").unwrap_or(&Value::Null);
                if items.is_array() {
                    return Err(unsupported("items list form"));
                }
                Ok(FieldType::Array(Box::new(parse_field_type(
                    tool,
                    &format!("{field}[]"),
                    items,
                )?)))
            } else {
                Err(unsupported("property without type"))
            }
        }
        Some("string") => match obj.get("format").and_then(Value::as_str) {
            None => Ok(FieldType::Str),
            Some("date-time") => Ok(FieldType::Datetime),
            Some("date") => Ok(FieldType::Date),
            Some(_) => Err(unsupported("string format")),
        },
        Some("integer") => Ok(FieldType::Int),
        Some("number") => Ok(FieldType::Num),
        Some("boolean") => Ok(FieldType::Bool),
        Some("array") => {
            let items = obj
                .get("items")
                .ok_or_else(|| unsupported("array without items"))?;
            if items.is_array() {
                return Err(unsupported("items list form"));
            }
            Ok(FieldType::Array(Box::new(parse_field_type(
                tool,
                &format!("{field}[]"),
                items,
            )?)))
        }
        Some("object") => parse_object_inline(tool, field, obj),
        Some("null") => Err(unsupported("null property type")),
        Some(other) => Err(unsupported(&format!("unknown type {other}"))),
    }
}

fn parse_object_inline(
    tool: &str,
    field: &str,
    obj: &serde_json::Map<String, Value>,
) -> Result<FieldType, CompactError> {
    let unsupported = |feature: &str| CompactError::UnsupportedSchema {
        tool: tool.to_string(),
        feature: format!("{field}: {feature}"),
    };
    let required: std::collections::HashSet<String> = match obj.get("required") {
        None | Some(Value::Null) => std::collections::HashSet::new(),
        Some(Value::Array(arr)) => {
            let mut set = std::collections::HashSet::new();
            for v in arr {
                set.insert(
                    v.as_str()
                        .ok_or_else(|| unsupported("non-string required entry"))?
                        .to_string(),
                );
            }
            set
        }
        Some(_) => return Err(unsupported("required must be an array")),
    };
    let props = match obj.get("properties") {
        None | Some(Value::Null) => serde_json::Map::new(),
        Some(Value::Object(m)) => m.clone(),
        Some(_) => return Err(unsupported("properties must be an object")),
    };
    for r in &required {
        if !props.contains_key(r) {
            return Err(unsupported("required without matching property"));
        }
    }
    let mut fields = Vec::with_capacity(props.len());
    for (name, schema) in &props {
        if !is_simple_name(name) {
            return Err(unsupported("property name charset"));
        }
        fields.push(Field {
            name: name.clone(),
            required: required.contains(name),
            ty: parse_field_type(tool, &format!("{field}.{name}"), schema)?,
            description: schema
                .get("description")
                .and_then(Value::as_str)
                .map(str::to_string),
        });
    }
    Ok(FieldType::Object(fields))
}

// ---------------------------------------------------------------------------
// Decoding
// ---------------------------------------------------------------------------

/// Render one expected call in this crate's wire grammar (used by evaluators
/// to build `rendered_calls` deterministically from expected calls).
pub fn render_call(name: &str, args: &Value) -> String {
    let body = serde_json::to_string(args).unwrap_or_else(|_| "{}".to_string());
    format!("<<call {name} {body}>>")
}

/// Decode every `<<call …>>` in `text`, validating each against `tools`.
///
/// - No marker → `Ok(vec![])` (plain answer).
/// - Unknown name → `Err(UnknownTool)`.
/// - Bad arguments → `Err(InvalidArguments)`.
/// - Unparsable marker → `Err(Malformed)`.
/// Never returns a guessed call.
pub fn decode_calls(text: &str, tools: &[ToolDef]) -> Result<Vec<ToolCall>, CompactError> {
    let parsed = parse_all_tools_for_decode(tools)?;
    decode_with_parsed(text, tools, &parsed)
}

fn parse_all_tools_for_decode(tools: &[ToolDef]) -> Result<Vec<ParsedTool>, CompactError> {
    let mut out = Vec::with_capacity(tools.len());
    for t in tools {
        match parse_tool_schema(t) {
            Ok(p) => out.push(p),
            Err(CompactError::UnsupportedSchema { tool, feature }) => {
                // Decoding against an un-compactable schema is a caller error;
                // surface it as invalid tooling, never as a guessed call.
                return Err(CompactError::InvalidArguments {
                    tool,
                    reason: format!("unsupported_schema:{feature}"),
                });
            }
            Err(e) => return Err(e),
        }
    }
    Ok(out)
}

fn decode_with_parsed(
    text: &str,
    _tools: &[ToolDef],
    parsed: &[ParsedTool],
) -> Result<Vec<ToolCall>, CompactError> {
    let spans = scan_calls(text)?;
    let mut out = Vec::with_capacity(spans.len());
    for span in spans {
        let tool = parsed
            .iter()
            .find(|p| p.name == span.name)
            .ok_or_else(|| CompactError::UnknownTool(span.name.clone()))?;
        let args: Value =
            serde_json::from_str(&span.json_body).map_err(|e| CompactError::InvalidArguments {
                tool: span.name.clone(),
                reason: format!("bad_json:{e}"),
            })?;
        validate_args(tool, &args)?;
        // The router assigns call IDs at its own seam; this crate returns the
        // validated name + argument object.
        out.push(ToolCall {
            name: span.name,
            arguments: args,
        });
    }
    Ok(out)
}

/// One raw marker span found by the scanner.
#[derive(Debug, Clone, PartialEq, Eq)]
struct RawCall {
    name: String,
    json_body: String,
}

/// Scan `text` for `<<call name {…}>>` spans with a JSON-aware brace matcher
/// so `>>` inside a string never terminates early.
///
/// Any `<<call` that cannot be completed is [`CompactError::Malformed`].
fn scan_calls(text: &str) -> Result<Vec<RawCall>, CompactError> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let Some(rel) = find_marker(&bytes[i..]) else {
            break;
        };
        let start = i + rel;
        let mut j = start + "<<call".len();
        // Require at least one ASCII space after `<<call`.
        if j >= bytes.len() || (bytes[j] != b' ' && bytes[j] != b'\t' && bytes[j] != b'\n') {
            return Err(CompactError::Malformed(
                "`<<call` must be followed by a space and a tool name".to_string(),
            ));
        }
        j = skip_ws(bytes, j);
        let name_start = j;
        while j < bytes.len()
            && (bytes[j].is_ascii_alphanumeric()
                || bytes[j] == b'_'
                || bytes[j] == b'-'
                || bytes[j] == b'.')
        {
            j += 1;
        }
        let name = text.get(name_start..j).unwrap_or_default().to_string();
        if name.is_empty() {
            return Err(CompactError::Malformed("missing tool name".to_string()));
        }
        j = skip_ws(bytes, j);
        if j >= bytes.len() || bytes[j] != b'{' {
            return Err(CompactError::Malformed(format!(
                "missing JSON object for tool '{name}'"
            )));
        }
        let json_start = j;
        let json_end = match_json_object(text, json_start)
            .ok_or_else(|| CompactError::Malformed(format!("unbalanced JSON for tool '{name}'")))?;
        j = skip_ws(bytes, json_end);
        if j + 1 >= bytes.len() || bytes[j] != b'>' || bytes[j + 1] != b'>' {
            return Err(CompactError::Malformed(format!(
                "missing closing '>>' for tool '{name}'"
            )));
        }
        let json_body = text
            .get(json_start..json_end)
            .unwrap_or_default()
            .to_string();
        out.push(RawCall { name, json_body });
        i = j + 2;
    }
    Ok(out)
}

fn find_marker(haystack: &[u8]) -> Option<usize> {
    if haystack.len() < 6 {
        return None;
    }
    haystack.windows(6).position(|w| w == b"<<call")
}

fn skip_ws(bytes: &[u8], mut j: usize) -> usize {
    while j < bytes.len()
        && (bytes[j] == b' ' || bytes[j] == b'\t' || bytes[j] == b'\n' || bytes[j] == b'\r')
    {
        j += 1;
    }
    j
}

/// Given the byte index of a `{`, return the one-past-end index of the
/// balanced object, respecting strings and `\\` escapes. `None` when
/// unbalanced. Operates on bytes but only slices at the (ASCII) braces, so
/// no UTF-8 boundary is ever split: the returned index is either the input
/// length or just past an ASCII `}`, both valid boundaries.
fn match_json_object(text: &str, open: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    if bytes.get(open) != Some(&b'{') {
        return None;
    }
    let mut depth: i32 = 0;
    let mut in_str = false;
    let mut escaped = false;
    let mut j = open;
    while j < bytes.len() {
        let b = bytes[j];
        if in_str {
            if escaped {
                escaped = false;
            } else if b == b'\\' {
                escaped = true;
            } else if b == b'"' {
                in_str = false;
            }
        } else if b == b'"' {
            in_str = true;
        } else if b == b'{' {
            depth += 1;
        } else if b == b'}' {
            depth -= 1;
            if depth == 0 {
                return Some(j + 1);
            }
            if depth < 0 {
                return None;
            }
        }
        j += 1;
    }
    None
}

/// Validate a parsed JSON argument object against the tool's schema.
fn validate_args(tool: &ParsedTool, args: &Value) -> Result<(), CompactError> {
    let invalid = |reason: String| CompactError::InvalidArguments {
        tool: tool.name.clone(),
        reason,
    };
    let obj = args
        .as_object()
        .ok_or_else(|| invalid("not_an_object".to_string()))?;
    for f in &tool.fields {
        match obj.get(&f.name) {
            None => {
                if f.required {
                    return Err(invalid(format!("missing_required:{}", f.name)));
                }
            }
            Some(Value::Null) => {
                if f.required {
                    return Err(invalid(format!("missing_required:{}", f.name)));
                }
                // Explicit null for an optional field: accept only if the
                // schema type itself is nullable — but nullability folds into
                // optionality here, so plain null is accepted as "absent".
            }
            Some(v) => validate_value(&tool.name, &f.name, &f.ty, v)?,
        }
    }
    Ok(())
}

fn validate_value(tool: &str, path: &str, ty: &FieldType, v: &Value) -> Result<(), CompactError> {
    let invalid = |reason: &str| CompactError::InvalidArguments {
        tool: tool.to_string(),
        reason: format!("{reason}:{path}"),
    };
    match ty {
        FieldType::Str | FieldType::Datetime | FieldType::Date => {
            if v.as_str().is_none() {
                return Err(invalid("wrong_type"));
            }
            Ok(())
        }
        FieldType::Int => {
            if v.as_i64().is_none() {
                return Err(invalid("wrong_type"));
            }
            Ok(())
        }
        FieldType::Num => {
            if v.as_f64().is_none() {
                return Err(invalid("wrong_type"));
            }
            Ok(())
        }
        FieldType::Bool => {
            if v.as_bool().is_none() {
                return Err(invalid("wrong_type"));
            }
            Ok(())
        }
        FieldType::Enum(vals) => {
            let s = v.as_str().ok_or_else(|| CompactError::InvalidArguments {
                tool: tool.to_string(),
                reason: format!("wrong_type:{path}"),
            })?;
            if !vals.iter().any(|e| e == s) {
                return Err(invalid("invalid_enum"));
            }
            Ok(())
        }
        FieldType::Array(inner) => {
            let arr = v.as_array().ok_or_else(|| CompactError::InvalidArguments {
                tool: tool.to_string(),
                reason: format!("wrong_type:{path}"),
            })?;
            for (idx, item) in arr.iter().enumerate() {
                validate_value(tool, &format!("{path}[{idx}]"), inner, item)?;
            }
            Ok(())
        }
        FieldType::Object(fields) => {
            let obj = v
                .as_object()
                .ok_or_else(|| CompactError::InvalidArguments {
                    tool: tool.to_string(),
                    reason: format!("wrong_type:{path}"),
                })?;
            for f in fields {
                match obj.get(&f.name) {
                    None => {
                        if f.required {
                            return Err(CompactError::InvalidArguments {
                                tool: tool.to_string(),
                                reason: format!("missing_required:{path}.{}", f.name),
                            });
                        }
                    }
                    Some(Value::Null) => {
                        if f.required {
                            return Err(CompactError::InvalidArguments {
                                tool: tool.to_string(),
                                reason: format!("missing_required:{path}.{}", f.name),
                            });
                        }
                    }
                    Some(inner) => {
                        validate_value(tool, &format!("{path}.{}", f.name), &f.ty, inner)?;
                    }
                }
            }
            Ok(())
        }
    }
}

// ---------------------------------------------------------------------------
// Reverse decoding: compact block back to tool schemas
// ---------------------------------------------------------------------------

/// Reconstruct tool schemas from a [`CompactTools`] block produced by
/// [`encode_tools`].
///
/// Round-trips everything the wire format carries: names, tool descriptions,
/// required flags, primitive types, enums, arrays, nested objects, and
/// `date-time`/`date` formats. Field descriptions are intentionally elided on
/// the wire (see the grammar docs), so restored schemas carry `None` for
/// field descriptions unless the block used the legacy `"description"` suffix
/// (still accepted on decode). Property order in restored schemas is
/// canonical (alphabetical by property name, following `serde_json::Map`
/// ordering), not file order — the property *set*, required *set*, types,
/// enums, and nesting are what validity depends on. Blocks that were bypassed
/// (`compacted == false`) return an error — there is nothing faithful to
/// restore.
pub fn decode_tools(compact: &CompactTools) -> Result<Vec<ToolDef>, CompactError> {
    if !compact.compacted {
        return Err(CompactError::Malformed(
            "no compact block: compaction was bypassed".to_string(),
        ));
    }
    let mut tools = Vec::new();
    let mut in_tools = false;
    for line in compact.text.lines() {
        let line = line.trim();
        if line == "Tools:" {
            in_tools = true;
            continue;
        }
        if !in_tools {
            continue;
        }
        if line.is_empty()
            || line.starts_with("Call tools as ")
            || line.starts_with("Today:")
            // Legacy block wordings (accepted on decode for compatibility).
            || line.starts_with("To call a tool")
            || line.starts_with("Rules:")
            || line.starts_with("Today is")
        {
            continue;
        }
        tools.push(parse_tool_line(line)?);
    }
    Ok(tools)
}

fn parse_tool_line(line: &str) -> Result<ToolDef, CompactError> {
    let malformed = |why: &str| CompactError::Malformed(format!("bad tool line: {why}: {line}"));
    let open = line.find('(').ok_or_else(|| malformed("missing '('"))?;
    let name = line.get(..open).unwrap_or_default().trim().to_string();
    if !is_simple_name(&name) {
        return Err(malformed("bad tool name"));
    }
    // Find the matching close paren for the outer parameter list, respecting
    // nested braces/brackets and quoted descriptions.
    let bytes = line.as_bytes();
    let mut depth = 0;
    let mut in_str = false;
    let mut escaped = false;
    let mut close: Option<usize> = None;
    let mut j = open;
    while j < bytes.len() {
        let b = bytes[j];
        if in_str {
            if escaped {
                escaped = false;
            } else if b == b'\\' {
                escaped = true;
            } else if b == b'"' {
                in_str = false;
            }
        } else if b == b'"' {
            in_str = true;
        } else if b == b'(' || b == b'{' || b == b'[' {
            depth += 1;
        } else if b == b')' || b == b'}' || b == b']' {
            depth -= 1;
            if depth == 0 && b == b')' {
                close = Some(j);
                break;
            }
            if depth < 0 {
                return Err(malformed("unbalanced brackets"));
            }
        }
        j += 1;
    }
    let close = close.ok_or_else(|| malformed("missing ')'"))?;
    let params_src = line.get(open + 1..close).unwrap_or_default();
    let rest = line.get(close + 1..).unwrap_or_default().trim();
    let description = if rest.is_empty() {
        None
    } else if let Some(stripped) = rest.strip_prefix("-") {
        let d = stripped.trim();
        if d.is_empty() {
            None
        } else {
            Some(d.to_string())
        }
    } else {
        return Err(malformed("expected ' - description'"));
    };

    let fields = if params_src.trim().is_empty() {
        Vec::new()
    } else {
        split_top_level(params_src, ',')
            .into_iter()
            .map(|p| parse_param(p.trim()))
            .collect::<Result<Vec<Field>, _>>()?
    };
    let required: Vec<Value> = fields
        .iter()
        .filter(|f| f.required)
        .map(|f| Value::String(f.name.clone()))
        .collect();
    let mut properties = serde_json::Map::new();
    for f in &fields {
        properties.insert(f.name.clone(), field_to_schema(f));
    }
    let parameters = Value::Object({
        let mut m = serde_json::Map::new();
        m.insert("type".to_string(), Value::String("object".to_string()));
        m.insert("properties".to_string(), Value::Object(properties));
        if !required.is_empty() {
            m.insert("required".to_string(), Value::Array(required));
        }
        m
    });
    Ok(ToolDef {
        name,
        description,
        parameters: Some(parameters),
    })
}

fn parse_param(src: &str) -> Result<Field, CompactError> {
    let malformed = || CompactError::Malformed(format!("bad param: {src}"));
    // name[?] : type [ "description" ]
    // Split name from type at the first top-level ':'.
    let colon = find_top_level_char(src, ':').ok_or_else(malformed)?;
    let name_part = src.get(..colon).unwrap_or_default().trim();
    let mut type_part = src.get(colon + 1..).unwrap_or_default().trim().to_string();
    let (name, required) = if let Some(stripped) = name_part.strip_suffix('?') {
        (stripped.trim().to_string(), false)
    } else {
        (name_part.to_string(), true)
    };
    if !is_simple_name(&name) {
        return Err(malformed());
    }
    // Optional trailing quoted description at the top level.
    let mut description = None;
    if type_part.ends_with('"') {
        let q = find_trailing_quote_start(&type_part).ok_or_else(malformed)?;
        let raw = type_part
            .get(q + 1..type_part.len() - 1)
            .unwrap_or_default();
        description = Some(unescape_desc(raw)?);
        type_part = type_part.get(..q).unwrap_or_default().trim().to_string();
    }
    let ty = parse_type_expr(&type_part)?;
    Ok(Field {
        name,
        required,
        ty,
        description,
    })
}

fn find_top_level_char(src: &str, target: char) -> Option<usize> {
    let mut depth_paren = 0;
    let mut depth_brace = 0;
    let mut depth_bracket = 0;
    let mut in_str = false;
    let mut escaped = false;
    for (idx, c) in src.char_indices() {
        if in_str {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_str = false;
            }
            continue;
        }
        match c {
            '"' => in_str = true,
            '(' => depth_paren += 1,
            ')' => depth_paren -= 1,
            '{' => depth_brace += 1,
            '}' => depth_brace -= 1,
            '[' => depth_bracket += 1,
            ']' => depth_bracket -= 1,
            c if c == target && depth_paren == 0 && depth_brace == 0 && depth_bracket == 0 => {
                return Some(idx);
            }
            _ => {}
        }
    }
    None
}

fn find_trailing_quote_start(s: &str) -> Option<usize> {
    // Last '"' is the closing quote; walk back to its opening quote,
    // respecting escapes. The opening quote must be preceded by whitespace.
    let bytes = s.as_bytes();
    if bytes.len() < 2 || bytes[bytes.len() - 1] != b'"' {
        return None;
    }
    // Find candidate opening quotes from the end; the first one whose
    // preceding char is whitespace (or start) and which is not escaped wins.
    // Because descriptions cannot contain unescaped '"' by construction
    // (encode escapes them), scanning for an unescaped '"' works.
    let mut i = bytes.len() - 1;
    // skip closing quote
    i = i.saturating_sub(1);
    loop {
        // find previous unescaped '"'
        let mut k: Option<usize> = None;
        let mut idx = i + 1;
        while idx > 0 {
            idx -= 1;
            if bytes[idx] == b'"' && !is_escaped(bytes, idx) {
                k = Some(idx);
                break;
            }
            if idx == 0 {
                break;
            }
        }
        let k = k?;
        let ok = k == 0 || bytes[k - 1].is_ascii_whitespace();
        if ok {
            return Some(k);
        }
        if k == 0 {
            return None;
        }
        i = k.saturating_sub(1);
        if i == 0 && k == 0 {
            return None;
        }
    }
}

fn is_escaped(bytes: &[u8], pos: usize) -> bool {
    let mut backslashes = 0;
    let mut i = pos;
    while i > 0 {
        i -= 1;
        if bytes[i] == b'\\' {
            backslashes += 1;
        } else {
            break;
        }
    }
    backslashes % 2 == 1
}

fn unescape_desc(raw: &str) -> Result<String, CompactError> {
    let mut s = String::with_capacity(raw.len());
    let mut chars = raw.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('"') => s.push('"'),
                Some('\\') => s.push('\\'),
                Some(other) => {
                    return Err(CompactError::Malformed(format!(
                        "bad escape in description: \\{other}"
                    )));
                }
                None => return Err(CompactError::Malformed("trailing backslash".to_string())),
            }
        } else {
            s.push(c);
        }
    }
    Ok(s)
}

fn parse_type_expr(src: &str) -> Result<FieldType, CompactError> {
    let malformed = || CompactError::Malformed(format!("bad type: {src}"));
    let src = src.trim();
    match src {
        "str" => return Ok(FieldType::Str),
        "datetime" => return Ok(FieldType::Datetime),
        "date" => return Ok(FieldType::Date),
        "int" => return Ok(FieldType::Int),
        "num" => return Ok(FieldType::Num),
        "bool" => return Ok(FieldType::Bool),
        _ => {}
    }
    if src.starts_with('[') && src.ends_with(']') {
        let inner = src.get(1..src.len() - 1).unwrap_or_default();
        return Ok(FieldType::Array(Box::new(parse_type_expr(inner)?)));
    }
    if src.starts_with('{') && src.ends_with('}') {
        let inner = src.get(1..src.len() - 1).unwrap_or_default().trim();
        if inner.is_empty() {
            return Ok(FieldType::Object(Vec::new()));
        }
        let fields = split_top_level(inner, ',')
            .into_iter()
            .map(|p| parse_param(p.trim()))
            .collect::<Result<Vec<Field>, _>>()?;
        return Ok(FieldType::Object(fields));
    }
    if src.contains('|') {
        let vals: Vec<String> = src.split('|').map(str::to_string).collect();
        if vals.iter().any(|v| !is_simple_enum_value(v)) {
            return Err(malformed());
        }
        return Ok(FieldType::Enum(vals));
    }
    Err(malformed())
}

fn split_top_level(src: &str, sep: char) -> Vec<String> {
    let mut parts = Vec::new();
    let mut depth_paren = 0;
    let mut depth_brace = 0;
    let mut depth_bracket = 0;
    let mut in_str = false;
    let mut escaped = false;
    let mut start = 0;
    for (idx, c) in src.char_indices() {
        if in_str {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_str = false;
            }
            continue;
        }
        match c {
            '"' => in_str = true,
            '(' => depth_paren += 1,
            ')' => depth_paren -= 1,
            '{' => depth_brace += 1,
            '}' => depth_brace -= 1,
            '[' => depth_bracket += 1,
            ']' => depth_bracket -= 1,
            c if c == sep && depth_paren == 0 && depth_brace == 0 && depth_bracket == 0 => {
                parts.push(src.get(start..idx).unwrap_or_default().to_string());
                start = idx + c.len_utf8();
            }
            _ => {}
        }
    }
    parts.push(src.get(start..).unwrap_or_default().to_string());
    parts
}

fn field_to_schema(f: &Field) -> Value {
    let mut m = serde_json::Map::new();
    match &f.ty {
        FieldType::Str => {
            m.insert("type".to_string(), Value::String("string".to_string()));
        }
        FieldType::Datetime => {
            m.insert("type".to_string(), Value::String("string".to_string()));
            m.insert("format".to_string(), Value::String("date-time".to_string()));
        }
        FieldType::Date => {
            m.insert("type".to_string(), Value::String("string".to_string()));
            m.insert("format".to_string(), Value::String("date".to_string()));
        }
        FieldType::Int => {
            m.insert("type".to_string(), Value::String("integer".to_string()));
        }
        FieldType::Num => {
            m.insert("type".to_string(), Value::String("number".to_string()));
        }
        FieldType::Bool => {
            m.insert("type".to_string(), Value::String("boolean".to_string()));
        }
        FieldType::Enum(vals) => {
            m.insert("type".to_string(), Value::String("string".to_string()));
            m.insert(
                "enum".to_string(),
                Value::Array(vals.iter().map(|v| Value::String(v.clone())).collect()),
            );
        }
        FieldType::Array(inner) => {
            m.insert("type".to_string(), Value::String("array".to_string()));
            m.insert("items".to_string(), type_to_schema(inner));
        }
        FieldType::Object(fields) => {
            m.insert("type".to_string(), Value::String("object".to_string()));
            let mut props = serde_json::Map::new();
            let mut required = Vec::new();
            for inner in fields {
                if inner.required {
                    required.push(Value::String(inner.name.clone()));
                }
                props.insert(inner.name.clone(), field_to_schema(inner));
            }
            m.insert("properties".to_string(), Value::Object(props));
            if !required.is_empty() {
                m.insert("required".to_string(), Value::Array(required));
            }
        }
    }
    if let Some(d) = f.description.as_deref() {
        m.insert("description".to_string(), Value::String(d.to_string()));
    }
    Value::Object(m)
}

fn type_to_schema(ty: &FieldType) -> Value {
    match ty {
        FieldType::Object(fields) => {
            let mut m = serde_json::Map::new();
            m.insert("type".to_string(), Value::String("object".to_string()));
            let mut props = serde_json::Map::new();
            let mut required = Vec::new();
            for f in fields {
                if f.required {
                    required.push(Value::String(f.name.clone()));
                }
                props.insert(f.name.clone(), field_to_schema(f));
            }
            m.insert("properties".to_string(), Value::Object(props));
            if !required.is_empty() {
                m.insert("required".to_string(), Value::Array(required));
            }
            Value::Object(m)
        }
        FieldType::Array(inner) => Value::Object({
            let mut m = serde_json::Map::new();
            m.insert("type".to_string(), Value::String("array".to_string()));
            m.insert("items".to_string(), type_to_schema(inner));
            m
        }),
        FieldType::Enum(vals) => Value::Object({
            let mut m = serde_json::Map::new();
            m.insert("type".to_string(), Value::String("string".to_string()));
            m.insert(
                "enum".to_string(),
                Value::Array(vals.iter().map(|v| Value::String(v.clone())).collect()),
            );
            m
        }),
        FieldType::Str => serde_json::json!({"type": "string"}),
        FieldType::Datetime => serde_json::json!({"type": "string", "format": "date-time"}),
        FieldType::Date => serde_json::json!({"type": "string", "format": "date"}),
        FieldType::Int => serde_json::json!({"type": "integer"}),
        FieldType::Num => serde_json::json!({"type": "number"}),
        FieldType::Bool => serde_json::json!({"type": "boolean"}),
    }
}

// ---------------------------------------------------------------------------
// Incremental decoding
// ---------------------------------------------------------------------------

/// Incremental decoder for streaming model output.
///
/// Chunks are concatenated verbatim; markers split across chunk boundaries
/// reconstruct correctly because parsing runs over the full buffer at
/// [`StreamDecoder::finish`]. Feed every chunk via [`StreamDecoder::push`],
/// then call `finish` once the stream ends.
#[derive(Debug, Default, Clone)]
pub struct StreamDecoder {
    buffer: String,
}

impl StreamDecoder {
    /// Create an empty decoder.
    pub fn new() -> Self {
        Self::default()
    }

    /// Append one stream chunk verbatim.
    pub fn push(&mut self, chunk: &str) {
        self.buffer.push_str(chunk);
    }

    /// Decode the buffered output against `tools`. See [`decode_calls`] for
    /// the success/error contract.
    pub fn finish(&self, tools: &[ToolDef]) -> Result<Vec<ToolCall>, CompactError> {
        decode_calls(&self.buffer, tools)
    }

    /// Discard buffered output.
    pub fn reset(&mut self) {
        self.buffer.clear();
    }

    /// Buffered text so far (for debugging only; never parsed incrementally).
    pub fn buffered(&self) -> &str {
        &self.buffer
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn calendar_tool() -> ToolDef {
        ToolDef {
            name: "create_calendar_event".to_string(),
            description: Some("Create an event in the user's calendar.".to_string()),
            parameters: Some(json!({
                "type": "object",
                "properties": {
                    "title": {"type": "string", "description": "Event title"},
                    "start": {"type": "string", "format": "date-time", "description": "Start time, ISO 8601"},
                    "duration_min": {"type": "integer", "description": "Duration in minutes"},
                    "attendees": {"type": "array", "items": {"type": "string"}, "description": "Attendee emails"},
                    "visibility": {"type": "string", "enum": ["public", "private"]}
                },
                "required": ["title", "start"]
            })),
        }
    }

    fn email_tool() -> ToolDef {
        ToolDef {
            name: "send_email".to_string(),
            description: Some("Send an email from the user's account.".to_string()),
            parameters: Some(json!({
                "type": "object",
                "properties": {
                    "to": {"type": "array", "items": {"type": "string"}, "description": "Recipient emails"},
                    "subject": {"type": "string", "description": "Subject line"},
                    "body": {"type": "string", "description": "Plain-text body"},
                    "cc": {"type": "array", "items": {"type": "string"}, "description": "CC emails"}
                },
                "required": ["to", "subject", "body"]
            })),
        }
    }

    #[test]
    fn encode_is_compact_and_mentions_every_tool() {
        let c = encode_tools(&[calendar_tool(), email_tool()]).unwrap();
        assert!(c.compacted);
        assert!(c.text.contains("create_calendar_event("));
        assert!(c.text.contains("send_email("));
        assert!(c.text.contains("<<call name {json args}>>"));
        assert!(c.text.contains("2026-10-02"));
        // Required vs optional markers.
        assert!(c.text.contains("title:str"));
        assert!(c.text.contains("duration_min?:int"));
        assert!(c.text.contains("attendees?:[str]"));
        assert!(c.text.contains("visibility?:public|private"));
    }

    #[test]
    fn encode_is_deterministic() {
        let tools = [calendar_tool(), email_tool()];
        let a = encode_tools(&tools).unwrap();
        let b = encode_tools(&tools).unwrap();
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn encode_rejects_duplicate_names() {
        let t = calendar_tool();
        let err = encode_tools(&[t.clone(), t]).unwrap_err();
        assert!(matches!(err, CompactError::InvalidToolDef(_)));
    }

    #[test]
    fn decode_single_call() {
        let tools = [calendar_tool()];
        let calls = decode_calls(
            "<<call create_calendar_event {\"title\":\"Design review\",\"start\":\"2026-10-05T15:00:00+05:30\"}>>",
            &tools,
        )
        .unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "create_calendar_event");
        assert_eq!(calls[0].arguments["title"], json!("Design review"));
    }

    #[test]
    fn decode_multiple_calls_and_surrounding_text() {
        let tools = [calendar_tool(), email_tool()];
        let text = "Sure, doing both:\n<<call send_email {\"to\":[\"sam@example.com\"],\"subject\":\"Hi\",\"body\":\"x\"}>>\nand\n<<call create_calendar_event {\"title\":\"Retro\",\"start\":\"2026-10-04T10:00:00+05:30\"}>>\ndone.";
        let calls = decode_calls(text, &tools).unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].name, "send_email");
        assert_eq!(calls[1].name, "create_calendar_event");
    }

    #[test]
    fn decode_plain_answer_is_empty() {
        let tools = [calendar_tool()];
        assert_eq!(
            decode_calls("The weather is sunny.", &tools).unwrap(),
            vec![]
        );
        assert_eq!(decode_calls("", &tools).unwrap(), vec![]);
    }

    #[test]
    fn decode_unknown_tool_errors() {
        let tools = [calendar_tool()];
        let err = decode_calls("<<call delete_everything {}>>", &tools).unwrap_err();
        assert_eq!(
            err,
            CompactError::UnknownTool("delete_everything".to_string())
        );
    }

    #[test]
    fn decode_missing_required_errors() {
        let tools = [calendar_tool()];
        let err = decode_calls(
            "<<call create_calendar_event {\"start\":\"2026-10-05T15:00:00+05:30\"}>>",
            &tools,
        )
        .unwrap_err();
        assert!(matches!(err, CompactError::InvalidArguments { .. }));
    }

    #[test]
    fn decode_wrong_type_errors() {
        let tools = [calendar_tool()];
        let err = decode_calls(
            "<<call create_calendar_event {\"title\":\"x\",\"start\":\"y\",\"duration_min\":\"thirty\"}>>",
            &tools,
        )
        .unwrap_err();
        assert!(matches!(err, CompactError::InvalidArguments { .. }));
    }

    #[test]
    fn decode_bad_enum_errors() {
        let tools = [calendar_tool()];
        let err = decode_calls(
            "<<call create_calendar_event {\"title\":\"x\",\"start\":\"y\",\"visibility\":\"secret\"}>>",
            &tools,
        )
        .unwrap_err();
        assert!(matches!(err, CompactError::InvalidArguments { .. }));
    }

    #[test]
    fn decode_malformed_errors() {
        let tools = [calendar_tool()];
        assert!(matches!(
            decode_calls("<<call create_calendar_event {\"title\":}>>", &tools).unwrap_err(),
            CompactError::InvalidArguments { .. } | CompactError::Malformed(_)
        ));
        assert!(matches!(
            decode_calls("<<call create_calendar_event", &tools).unwrap_err(),
            CompactError::Malformed(_)
        ));
        assert!(matches!(
            decode_calls("<<call  {\"title\":\"x\"}>>", &tools).unwrap_err(),
            CompactError::Malformed(_)
        ));
    }

    #[test]
    fn decode_escaped_content_and_gt_inside_string() {
        let tools = [email_tool()];
        let calls = decode_calls(
            "<<call send_email {\"to\":[\"sam@example.com\"],\"subject\":\"a >> b\",\"body\":\"x\"}>>",
            &tools,
        )
        .unwrap();
        assert_eq!(calls[0].arguments["subject"], json!("a >> b"));
        // Escaped quotes inside a string.
        let calls = decode_calls(
            "<<call send_email {\"to\":[\"s@e.com\"],\"subject\":\"say \\\"hi\\\"\",\"body\":\"x\"}>>",
            &tools,
        )
        .unwrap();
        assert_eq!(calls[0].arguments["subject"], json!("say \"hi\""));
    }

    #[test]
    fn stream_decoder_handles_split_markers() {
        let tools = [calendar_tool()];
        let chunks = [
            "<<ca",
            "ll create_calendar_event {\"title\":\"Ret",
            "ro\",\"start\":\"2026-10-04T10:00:00+05:30\"}>",
            ">",
        ];
        let mut dec = StreamDecoder::new();
        for c in chunks {
            dec.push(c);
        }
        let calls = dec.finish(&tools).unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].arguments["title"], json!("Retro"));
    }

    #[test]
    fn stream_decoder_split_inside_json_string() {
        let tools = [email_tool()];
        let full = "<<call send_email {\"to\":[\"sam@example.com\"],\"subject\":\"a >> b\",\"body\":\"x\"}>>";
        for split in [1, 5, 10, 30, 60] {
            let split = split.min(full.len());
            let mut dec = StreamDecoder::new();
            dec.push(&full[..split]);
            dec.push(&full[split..]);
            let calls = dec.finish(&tools).unwrap();
            assert_eq!(calls.len(), 1, "split at {split}");
        }
    }

    #[test]
    fn round_trip_render_then_decode() {
        let tools = [calendar_tool(), email_tool()];
        let args = json!({"title": "Retro", "start": "2026-10-04T10:00:00+05:30"});
        let rendered = render_call("create_calendar_event", &args);
        let back = decode_calls(&rendered, &tools).unwrap();
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].arguments, args);
    }

    #[test]
    fn decode_tools_round_trips_supported_schemas() {
        let tools = vec![calendar_tool(), email_tool()];
        let compact = encode_tools(&tools).unwrap();
        let back = decode_tools(&compact).unwrap();
        assert_eq!(back.len(), 2);
        for (a, b) in tools.iter().zip(back.iter()) {
            assert_eq!(a.name, b.name);
            assert_eq!(a.description, b.description);
            // Field descriptions are elided on the wire by design; everything
            // else (required, types, enums, arrays, nesting, formats) must be
            // identical.
            assert_eq!(
                without_field_descriptions(a.parameters.as_ref()),
                without_field_descriptions(b.parameters.as_ref())
            );
        }
    }

    /// Strip per-field `description` entries (recursively) for comparison:
    /// the wire format elides them, so restored schemas differ only there.
    /// `required` arrays are sorted: the original lists them in file order
    /// while restored schemas follow canonical property order — the set is
    /// what validity depends on.
    fn without_field_descriptions(params: Option<&Value>) -> Value {
        let Some(v) = params else {
            return Value::Null;
        };
        normalize_schema(v)
    }

    fn normalize_schema(v: &Value) -> Value {
        match v {
            Value::Object(m) => {
                let mut out = serde_json::Map::new();
                for (k, val) in m {
                    if k == "description" {
                        continue;
                    }
                    if k == "required" {
                        if let Value::Array(arr) = val {
                            let mut sorted = arr.clone();
                            sorted.sort_by(|a, b| {
                                a.as_str()
                                    .unwrap_or_default()
                                    .cmp(b.as_str().unwrap_or_default())
                            });
                            out.insert(k.clone(), Value::Array(sorted));
                            continue;
                        }
                    }
                    out.insert(k.clone(), normalize_schema(val));
                }
                Value::Object(out)
            }
            Value::Array(arr) => Value::Array(arr.iter().map(normalize_schema).collect()),
            other => other.clone(),
        }
    }

    #[test]
    fn nested_objects_validate() {
        let tool = ToolDef {
            name: "book".to_string(),
            description: None,
            parameters: Some(json!({
                "type": "object",
                "properties": {
                    "where": {
                        "type": "object",
                        "properties": {
                            "city": {"type": "string"},
                            "zip": {"type": "string"}
                        },
                        "required": ["city"]
                    }
                },
                "required": ["where"]
            })),
        };
        let tools = [tool];
        decode_calls("<<call book {\"where\":{\"city\":\"Hyd\"}}>>", &tools).unwrap();
        assert!(decode_calls("<<call book {\"where\":{}}>>", &tools).is_err());
        assert!(decode_calls("<<call book {\"where\":{\"city\":5}}>>", &tools).is_err());
        // Compact + reverse round-trip preserves nesting.
        let c = encode_tools(&tools).unwrap();
        assert!(c.compacted);
        let back = decode_tools(&c).unwrap();
        assert_eq!(back[0].parameters, tools[0].parameters);
    }

    #[test]
    fn arrays_of_objects_validate() {
        let tool = ToolDef {
            name: "invite".to_string(),
            description: None,
            parameters: Some(json!({
                "type": "object",
                "properties": {
                    "guests": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {"email": {"type": "string"}},
                            "required": ["email"]
                        }
                    }
                },
                "required": ["guests"]
            })),
        };
        let tools = [tool];
        decode_calls(
            "<<call invite {\"guests\":[{\"email\":\"a@e.com\"}]}>>",
            &tools,
        )
        .unwrap();
        assert!(decode_calls("<<call invite {\"guests\":[{}]}>>", &tools).is_err());
        assert!(decode_calls("<<call invite {\"guests\":{}}>>", &tools).is_err());
    }

    #[test]
    fn unsupported_features_bypass_fail_closed() {
        for (feature, params) in [
            ("$ref", json!({"$ref": "#/defs/X"})),
            ("oneOf", json!({"oneOf": [{"type": "string"}]})),
            ("allOf", json!({"allOf": [{"type": "object"}]})),
            (
                "bad-enum-charset",
                json!({
                    "type": "object",
                    "properties": {"v": {"type": "string", "enum": ["a b"]}},
                }),
            ),
            (
                "array-without-items",
                json!({
                    "type": "object",
                    "properties": {"v": {"type": "array"}},
                    "required": ["v"],
                }),
            ),
        ] {
            let tool = ToolDef {
                name: "t".to_string(),
                description: None,
                parameters: Some(params),
            };
            let c = encode_tools(&[tool]).unwrap();
            assert!(!c.compacted, "feature {feature} should bypass");
            assert!(c.bypass_reason.is_some());
        }
    }

    #[test]
    fn empty_tools_encode_to_empty_block() {
        let c = encode_tools(&[]).unwrap();
        assert!(c.compacted);
        assert_eq!(decode_calls("hello", &[]).unwrap(), vec![]);
    }

    #[test]
    fn integer_rejects_floats_and_strings() {
        let tool = ToolDef {
            name: "t".to_string(),
            description: None,
            parameters: Some(json!({
                "type": "object",
                "properties": {"n": {"type": "integer"}},
                "required": ["n"]
            })),
        };
        let tools = [tool];
        decode_calls("<<call t {\"n\":3}>>", &tools).unwrap();
        assert!(decode_calls("<<call t {\"n\":3.5}>>", &tools).is_err());
        assert!(decode_calls("<<call t {\"n\":\"3\"}>>", &tools).is_err());
    }

    #[test]
    fn missing_closing_marker_is_malformed() {
        let tools = [calendar_tool()];
        let err = decode_calls(
            "<<call create_calendar_event {\"title\":\"x\",\"start\":\"y\"}",
            &tools,
        )
        .unwrap_err();
        assert!(matches!(err, CompactError::Malformed(_)));
    }

    #[test]
    fn whitespace_before_close_is_allowed() {
        let tools = [calendar_tool()];
        let calls = decode_calls(
            "<<call create_calendar_event {\"title\":\"x\",\"start\":\"y\"} >>",
            &tools,
        )
        .unwrap();
        assert_eq!(calls.len(), 1);
    }

    #[test]
    fn fuzz_scanner_never_panics_or_guesses() {
        // Adversarial fragments: the decoder must either return calls that
        // validate or return an error — never panic, never invent a call.
        let tools = [calendar_tool(), email_tool()];
        let fragments = [
            "<<call",
            "<<call ",
            "<<call x",
            "<<<call x {}>>",
            "<<call x {}>",
            ">>",
            "<<call create_calendar_event",
            "{",
            "}",
            "\"",
            "\\",
            "<<call create_calendar_event {\"title\":",
            "null",
            "[]",
            "{}",
        ];
        for a in &fragments {
            for b in &fragments {
                let text = format!("{a}{b}");
                let _ = decode_calls(&text, &tools);
            }
        }
    }

    #[test]
    fn field_descriptions_are_elided_but_structure_survives() {
        // Field descriptions stay off the wire (token budget); tool
        // descriptions and formats survive; structure round-trips.
        let tools = [calendar_tool()];
        let c = encode_tools(&tools).unwrap();
        assert!(
            !c.text.contains("Event title"),
            "field descriptions must be elided"
        );
        assert!(c.text.contains("datetime"));
        assert!(
            c.text.contains("Create an event in the user's calendar."),
            "tool descriptions must be kept"
        );
        let back = decode_tools(&c).unwrap();
        assert_eq!(
            without_field_descriptions(back[0].parameters.as_ref()),
            without_field_descriptions(tools[0].parameters.as_ref())
        );
    }
}

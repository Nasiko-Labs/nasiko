//! Compact tool schemas: a token-cheap rendering of OpenAI function tools, and a decoder that
//! turns the model's compact calls back into standard tool calls.
//!
//! Instead of sending `tools: [...]` as JSON Schema, the caller injects
//! [`CompactTools::prompt`] into a system message. The model answers with `<<call ...>>`
//! markers, which [`decode`] / [`StreamDecoder`] validate against the **original** schemas and
//! return as [`ToolCall`]s. A call that does not validate is an error, never a guess.
//!
//! # Invariants
//!
//! * **Pure** — no IO, no env, no clock, no RNG. Same input, same output.
//! * **Lossless or bypass** — [`encode_tools`] accepts only schema features it can express
//!   exactly (see *Supported schemas*); anything else is [`Error::Unsupported`] and the caller
//!   sends native tools. [`decode_tools`] recovers the canonical schema from the compact text.
//! * **Fail closed** — unknown tool, malformed call, invalid JSON, duplicate key, missing
//!   required field, unknown field, wrong type, enum or format violation: all errors.
//! * **Byte-faithful arguments** — [`ToolCall::arguments`] is the model's JSON object exactly as
//!   written (validated, never re-serialized or reordered).
//!
//! # Signature grammar (what the model reads)
//!
//! One line per tool:
//!
//! ```text
//! line   := NAME "(" fields? ")" [" - " desc]
//! fields := field (", " field)*
//! field  := fname ["?"] ":" type [" " QUOTED]   ; "?" = optional; QUOTED = description
//! fname  := [A-Za-z_][A-Za-z0-9_]* | QUOTED
//! type   := "str" | "int" | "num" | "bool" | "obj"
//!         | "datetime" | "date" | "time" | "email" | "uri"   ; string with that format
//!         | "[" type "]"                                     ; array
//!         | "{" fields? "}"                                  ; nested object
//!         | lit ("|" lit)*                                   ; enum (a bare lit needs 2+ values)
//! lit    := BARE_WORD | QUOTED | INTEGER
//! desc   := rest of line, or QUOTED if it is multi-line or starts with a quote
//! QUOTED := "'" (char | "\\" ( "\\" | "'" | "n" | "r" | "t" ))* "'"
//! ```
//!
//! # Call grammar (what the model writes)
//!
//! ```text
//! output := (TEXT | call)*
//! call   := "<<call" WS+ NAME WS* [JSON_OBJECT] WS* ">>"    ; omitted object = {}
//! NAME   := [A-Za-z0-9_.-]+
//! ```
//!
//! `<<call` must be followed by whitespace to open a call (`<<callback` is text). The JSON
//! object is scanned structurally, so `>>`, `}` or `<<call` inside a string argument needs no
//! escaping beyond ordinary JSON. Text before, between and after calls is returned as
//! [`Decoded::text`].
//!
//! # Supported schemas
//!
//! Root `parameters` must be `{"type":"object"}` with optional `properties` / `required`.
//! Properties may use `type` (`string`, `integer`, `number`, `boolean`, `array` with an `items`
//! schema, `object` with or without `properties`), `format` (`date-time`, `date`, `time`,
//! `email`, `uri`), `enum` (all strings or all integers) and `description`.
//!
//! Unsupported (tool is bypassed): `$ref`/`$defs`, `anyOf`/`oneOf`/`allOf`/`not`, type unions
//! and nullable types, `additionalProperties`, `default`, `examples`, `const`, `title`, numeric
//! and length constraints (`minimum`, `maxLength`, `pattern`, …), other formats, descriptions on
//! `items` or the root, and non-`function` tool kinds. Decoding rejects fields the schema does
//! not declare, i.e. it treats every object as closed.

#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

mod decode;
mod error;
mod schema;

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub use decode::{Decoded, StreamDecoder, StreamEvent};
pub use error::{Error, Result};

/// A function tool. The router converts its own `ToolDef` to this at the seam.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolDef {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// JSON Schema for the arguments.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parameters: Option<Value>,
}

/// A decoded call. The router assigns the `id`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCall {
    pub name: String,
    /// The JSON object as the model wrote it (OpenAI's `function.arguments` string).
    pub arguments: String,
}

impl ToolCall {
    /// Parsed arguments. Infallible in practice: decoded calls were validated as JSON objects.
    pub fn arguments_value(&self) -> Value {
        serde_json::from_str(&self.arguments).unwrap_or(Value::Null)
    }
}

/// The compact rendering of a tool set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactTools {
    /// One signature line per tool (the part [`decode_tools`] parses).
    pub definitions: String,
}

/// Fixed call-format instructions. Every token here is paid on every request, so it carries only
/// what guards a known failure mode: the marker syntax, JSON (not prose) arguments, omitting
/// unused optional fields (a `null` for one fails validation), and permission to answer plainly.
/// The signature notation itself (`?`, `[T]`, `a|b`) is TypeScript-like and needs no legend.
const HEADER: &str = "Tools (?=optional):";
const INSTRUCTIONS: &str = "To call a tool write <<call NAME {JSON args}>>, one per call; omit unused optional args. Otherwise reply normally.";

impl CompactTools {
    /// Definitions plus call-format instructions, ready for a system message.
    pub fn prompt(&self) -> String {
        format!("{HEADER}\n{}\n{INSTRUCTIONS}", self.definitions)
    }
}

/// Render tools as compact signatures. Fails with [`Error::Unsupported`] — meaning "bypass
/// compaction" — if any tool uses a schema feature the format cannot express exactly.
pub fn encode_tools(tools: &[ToolDef]) -> Result<CompactTools> {
    let mut out = String::new();
    for (i, t) in tools.iter().enumerate() {
        let unsupported = |reason: &str| Error::Unsupported {
            tool: t.name.clone(),
            reason: reason.to_owned(),
        };
        if !is_tool_name(&t.name) {
            return Err(unsupported("name must match [A-Za-z0-9_.-]{1,64}"));
        }
        if tools
            .get(..i)
            .is_some_and(|prev| prev.iter().any(|p| p.name == t.name))
        {
            return Err(unsupported("duplicate tool name"));
        }
        let fields = schema::lower_root(t.parameters.as_ref()).map_err(|r| unsupported(&r))?;
        if i > 0 {
            out.push('\n');
        }
        out.push_str(&t.name);
        out.push('(');
        schema::render_fields(&fields, &mut out);
        out.push(')');
        if let Some(d) = &t.description {
            out.push_str(" - ");
            let needs_quote =
                d.contains(['\n', '\r']) || d.starts_with('\'') || d.trim() != d || d.is_empty();
            if needs_quote {
                out.push_str(&schema::quote(d));
            } else {
                out.push_str(d);
            }
        }
    }
    Ok(CompactTools { definitions: out })
}

/// Parse compact definitions back into tools with canonical JSON Schemas: `required` is omitted
/// when empty, and a tool with no parameters comes back as an empty object schema.
pub fn decode_tools(compact: &CompactTools) -> Result<Vec<ToolDef>> {
    let mut tools = Vec::new();
    for line in compact.definitions.lines() {
        let mut p = schema::Parser::new(line);
        let name = p.word()?.to_owned();
        p.expect(b'(')?;
        let fields = p.fields(b')')?;
        let description = match p.rest() {
            "" => None,
            rest => {
                let d = rest
                    .strip_prefix(" - ")
                    .ok_or_else(|| p.err("expected ' - ' before the description"))?;
                if d.starts_with('\'') {
                    let mut q = schema::Parser::new(d);
                    let text = q.quoted()?;
                    if !q.rest().is_empty() {
                        return Err(q.err("trailing text after the description"));
                    }
                    Some(text)
                } else {
                    Some(d.to_owned())
                }
            }
        };
        tools.push(ToolDef {
            name,
            description,
            parameters: Some(schema::raise_root(&fields)),
        });
    }
    Ok(tools)
}

/// Decode a complete model response into text and validated calls.
pub fn decode(text: &str, tools: &[ToolDef]) -> Result<Decoded> {
    let mut d = StreamDecoder::new(tools)?;
    d.push(text)?;
    d.finish()
}

/// Decode a complete model response into validated calls (text is discarded).
pub fn decode_calls(text: &str, tools: &[ToolDef]) -> Result<Vec<ToolCall>> {
    decode(text, tools).map(|d| d.calls)
}

/// Render calls in the compact call grammar, one per line — what a model should write.
pub fn render_calls(calls: &[ToolCall]) -> String {
    calls
        .iter()
        .map(|c| format!("<<call {} {}>>", c.name, c.arguments))
        .collect::<Vec<_>>()
        .join("\n")
}

fn is_tool_name(s: &str) -> bool {
    (1..=64).contains(&s.len())
        && s.bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'.' | b'-'))
}

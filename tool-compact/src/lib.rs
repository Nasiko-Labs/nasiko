//! Compact tool definitions and a validating decoder for compact tool calls.
//!
//! Native tool calling sends every tool's JSON Schema on every request. This crate replaces
//! that with a short, signature-style text the model reads in a system message, and parses
//! the model's text replies back into standard OpenAI-shaped tool calls.
//!
//! ```text
//! create_calendar_event: Create an event in the user's calendar.
//!  title: str # Event title
//!  start: str(date-time) # Start time, ISO 8601
//!  duration_min?: int # Duration in minutes
//!  visibility?: 'public'|'private'
//! ```
//!
//! and the model answers with `<<call create_calendar_event {"title":"Retro","start":"…"}>>`.
//!
//! # API
//!
//! * [`encode_tools`] — tools → [`CompactTools`] (definitions + call-format instructions), or an
//!   error meaning "bypass compaction for this request".
//! * [`decode_calls`] / [`decode`] — model text → validated [`ToolCall`]s.
//! * [`StreamDecoder`] — the same, incrementally, for streamed output.
//! * [`decode_tools`] — definitions text → JSON Schema, proving schema meaning survived.
//! * [`render_calls`] — calls → call text (for history replay and round-trip checks).
//!
//! # Grammar
//!
//! ## Definitions
//!
//! ```text
//! definitions := tool ( "\n" tool )*
//! tool        := header ( "\n" field )*
//! header      := TOOL_NAME [ "(closed)" ] ":" [ " " TEXT ]
//! field       := INDENT FIELD_NAME [ "?" ] ": " type [ " {" ] [ " # " TEXT ]
//!                 [ ( "\n" field )* "\n" INDENT "}" ]
//!                 -- INDENT is one space per nesting level (top-level fields: one space).
//!                 -- " {" appears exactly when the type is `obj` or an array of `obj`; the
//!                 -- object's fields follow at INDENT + 1, closed by "}" at INDENT.
//! type        := union postfix*
//! postfix     := "[]" [ ann ]                       -- array of the preceding type
//! union       := "(" type ")"                       -- grouping, used for arrays of unions
//!              | member ( "|" member )* [ ann ]
//! member      := "str" | "int" | "num" | "bool" | "null" | "any" | "obj" | literal
//!                 -- all names: a type union; any literal: an enum (where `null` is the literal)
//! literal     := "'" CHARS "'" | INTEGER | "true" | "false" | "null"
//!                 -- CHARS escapes: \\ \' \n \r \t
//! ann         := "(" item ( "," item )* ")"
//! item        := "closed"                           -- additionalProperties: false
//!              | FORMAT_WORD                        -- format, e.g. date-time, email
//!              | ("min" | "max") "=" NUMBER         -- minimum / maximum
//!              | ("minLen" | "maxLen" | "minItems" | "maxItems") "=" INTEGER
//!              | "pattern=" "'" CHARS "'"
//!              | "default=" literal
//! TOOL_NAME   := [A-Za-z0-9_-]{1,64}
//! FIELD_NAME  := [A-Za-z0-9_.$@-]+
//! TEXT        := description, whitespace collapsed to single spaces, to end of line
//! ```
//!
//! `?` marks an optional field; every field without it is required.
//!
//! ## Calls
//!
//! ```text
//! output := ( TEXT | call )*
//! call   := "<<call" WS+ NAME WS* [ JSON_OBJECT WS* ] ">>"
//! ```
//!
//! `NAME` runs to the first whitespace, `{`, `(` or `>`. `JSON_OBJECT` is scanned with string and
//! escape awareness, so `>>` or `}` inside a string value never ends a call. `<<call` not
//! followed by whitespace is ordinary text. A call without arguments means `{}`.
//!
//! # Invariants
//!
//! * **Fail closed** — encoding refuses (→ bypass) any schema it cannot carry losslessly;
//!   decoding returns an error for an unknown tool, malformed call, duplicate key, missing
//!   required field, undeclared field or invalid value. Never a guessed or repaired call.
//! * **Unaltered** — a decoded call's `arguments` is the exact JSON text the model wrote.
//! * **Deterministic** — no clock, RNG, IO or env. Same input, same bytes out.
//! * **Bounded** — schemas deeper than 16 levels bypass; a call longer than 1 MiB is an error.

#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

mod decode;
mod defs;
mod encode;
mod error;
mod schema;
mod types;
mod validate;

pub use decode::{Decoded, MAX_CALL_BYTES, StreamDecoder, StreamEvent, decode, decode_calls};
pub use encode::{
    CALL_CLOSE, CALL_OPEN, INSTRUCTIONS, is_redundant_description, render_call, render_calls,
};
pub use error::{Error, Result};
pub use types::{FunctionDef, ToolCall, ToolDef};

use schema::{Kind, Node, Obj};

/// Compact definitions for a tool set, ready to put in a system message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactTools {
    /// Call-format instructions ([`INSTRUCTIONS`]).
    pub instructions: String,
    /// The tool definitions in the definitions grammar.
    pub definitions: String,
}

impl CompactTools {
    /// Instructions and definitions as one system-message body.
    pub fn system_prompt(&self) -> String {
        format!("{}\n{}", self.instructions, self.definitions)
    }
}

/// OpenAI's tool-name rule: `^[a-zA-Z0-9_-]{1,64}$`.
pub fn is_valid_tool_name(name: &str) -> bool {
    (1..=64).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// A tool after schema checking.
pub(crate) struct Compiled {
    pub name: String,
    pub description: Option<String>,
    pub params: Obj,
}

/// Check every tool and convert its schema. The single gate shared by encoding and decoding.
pub(crate) fn compile(tools: &[ToolDef]) -> Result<Vec<Compiled>> {
    let mut out: Vec<Compiled> = Vec::with_capacity(tools.len());
    for tool in tools {
        let name = &tool.function.name;
        let invalid = |reason: &str| Error::InvalidTool {
            tool: name.clone(),
            reason: reason.to_string(),
        };
        if !is_valid_tool_name(name) {
            return Err(invalid("name must match [A-Za-z0-9_-]{1,64}"));
        }
        if out.iter().any(|c| &c.name == name) {
            return Err(invalid("duplicate tool name"));
        }
        if tool.kind != "function" {
            return Err(Error::Unsupported {
                tool: name.clone(),
                path: "type".into(),
                feature: format!("tool type '{}'", tool.kind),
            });
        }
        if let Some(key) = tool.extra.keys().next() {
            return Err(Error::Unsupported {
                tool: name.clone(),
                path: key.clone(),
                feature: format!("tool field '{key}'"),
            });
        }
        let params = match &tool.function.parameters {
            None => Obj::default(),
            Some(p) => {
                let node = Node::from_json(p, "#", 0).map_err(|u| Error::Unsupported {
                    tool: name.clone(),
                    path: u.path,
                    feature: u.feature,
                })?;
                let Kind::Object(obj) = node.kind else {
                    return Err(invalid("parameters must be an object schema"));
                };
                // The top-level object has no field line to carry these on.
                if node.description.is_some() || !node.ann.is_empty() {
                    return Err(Error::Unsupported {
                        tool: name.clone(),
                        path: "#".into(),
                        feature: "description or annotations on the parameters object".into(),
                    });
                }
                obj
            }
        };
        out.push(Compiled {
            name: name.clone(),
            description: tool
                .function
                .description
                .as_deref()
                .and_then(schema::normalize_description),
            params,
        });
    }
    Ok(out)
}

/// Encode tools compactly. An `Err` means: send the native tools for this request.
pub fn encode_tools(tools: &[ToolDef]) -> Result<CompactTools> {
    let compiled = compile(tools)?;
    let mut definitions = String::new();
    for (i, tool) in compiled.iter().enumerate() {
        if i > 0 {
            definitions.push('\n');
        }
        encode::render_tool(
            &tool.name,
            tool.description.as_deref(),
            &tool.params,
            &mut definitions,
        );
    }
    Ok(CompactTools {
        instructions: INSTRUCTIONS.to_string(),
        definitions,
    })
}

/// Rebuild JSON Schema tool definitions from compact definitions text.
///
/// Reads only `compact.definitions` — the text the model sees — so a match against the
/// original schemas shows the meaning is in that text. Documented normalizations: whitespace
/// in descriptions is collapsed; a field description made only of words from the field and
/// tool names is dropped (it cannot disambiguate); `title` / `$schema` are dropped; objects always carry
/// `properties`; `items: {}` is omitted; an untyped enum or untyped object gains its implied
/// `type`; absent `parameters` becomes an empty object schema.
pub fn decode_tools(compact: &CompactTools) -> Result<Vec<ToolDef>> {
    defs::parse_definitions(&compact.definitions).map(|tools| {
        tools
            .into_iter()
            .map(|t| {
                let params = Node {
                    kind: Kind::Object(t.params),
                    description: None,
                    ann: Default::default(),
                };
                ToolDef::function(&t.name, t.description.as_deref(), Some(params.to_json()))
            })
            .collect()
    })
}

#[cfg(test)]
mod tests;

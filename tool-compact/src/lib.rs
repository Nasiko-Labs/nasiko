//! Compact tool schemas for LLM prompts.
//!
//! Tool definitions are a large, repeated share of a tool-using request. This crate renders
//! them as one-line signatures plus a short call-format instruction, and decodes the calls a
//! model writes back (`<<call NAME {JSON}>>`) into standard tool calls, validated against the
//! original schema. A pure library: no I/O, no environment reads, no provider code, and no
//! dependency on the router (it owns [`ToolDef`] / [`ToolCall`]; callers convert at their seam).
//!
//! ```
//! use nasiko_tool_compact::{ToolDef, decode_calls, encode_tools};
//! use serde_json::json;
//!
//! let tools = vec![ToolDef {
//!     name: "get_weather".into(),
//!     description: Some("Current weather for a city.".into()),
//!     parameters: Some(json!({
//!         "type": "object",
//!         "properties": {"city": {"type": "string"}, "unit": {"enum": ["c", "f"]}},
//!         "required": ["city"]
//!     })),
//! }];
//! let compact = encode_tools(&tools).unwrap();
//! assert_eq!(
//!     compact.definitions(),
//!     "get_weather(city:str, unit?:c|f) - Current weather for a city."
//! );
//! let calls = decode_calls(r#"Sure. <<call get_weather {"city":"Pune"}>>"#, &tools).unwrap();
//! assert_eq!(calls[0].arguments_json(), r#"{"city":"Pune"}"#);
//! ```
//!
//! # Definitions grammar (caller → model)
//!
//! [`CompactTools::system_prompt`] is one tool per line followed by [`CALL_INSTRUCTIONS`]:
//!
//! ```text
//! definitions = tool_line { "\n" tool_line } ;
//! tool_line   = tool_name "(" [ fields ] ")" [ "!" ] [ " - " text ] ;
//! fields      = field { ", " field } ;
//! field       = key [ "?" ] ":" type ;                          (* "?" = optional *)
//! key         = ident | quoted ;
//! type        = alt { "|" alt } [ "=" json ] [ " " quoted ] ;  (* default, description *)
//! alt         = "[" type "]"                                    (* array *)
//!             | "{" [ fields ] "}" [ "!" ]                      (* object; "!" = no extra keys *)
//!             | prim [ "(" [ number ] ".." [ number ] ")" ]     (* inclusive range, int/num *)
//!             | literal ;
//! prim        = "str" | "int" | "num" | "bool" | "any" | "obj"
//!             | "datetime" | "date" | "time" | "email" | "uri" | "uuid" ;
//! literal     = bare_word | quoted | number | "true" | "false" | "null" ;
//! quoted      = "'" { char | "\\" | "\'" | "\n" | "\r" | "\t" } "'" ;
//! ident       = ( letter | "_" ) { letter | digit | "_" } ;
//! bare_word   = ( letter | "_" ) { letter | digit | "_" | "-" | "." | "/" } ;  (* not a prim *)
//! tool_name   = 1-64 of letter | digit | "_" | "-" ;
//! text        = tool description, "\" written "\\", line breaks written "\n" ;
//! ```
//!
//! A union is either all literals (an `enum`) or one type plus `null` (nullable). Fields are
//! listed required first, then optional, each group by name, so the text does not depend on the
//! key order of the client's JSON.
//!
//! | JSON Schema | Compact |
//! |---|---|
//! | `string` / `integer` / `number` / `boolean` | `str` / `int` / `num` / `bool` |
//! | `format`: `date-time`, `date`, `time`, `email`, `uri`, `uuid` | `datetime`, `date`, `time`, `email`, `uri`, `uuid` |
//! | `minimum: 1, maximum: 99` | `int(1..99)` |
//! | `{}` / `{"type":"object"}` without properties | `any` / `obj` |
//! | `enum` or `const` of scalars | `a\|'b c'\|1\|null` |
//! | `items` / `properties` + `required` | `[T]` / `{a:T, b?:T}` |
//! | `additionalProperties: false` | `{…}!` (or `name(…)!` at top level) |
//! | `type: [T, "null"]`, or `anyOf: [T, {"type":"null"}]` (Pydantic `Optional`) | `T\|null` |
//! | `default` / property `description` | `T=json` / `T 'text'` |
//! | `title`, `$schema`, `$comment`, `examples` | dropped (no effect on calls) |
//!
//! An enum is shown by its members only: `{"type":["string","null"],"enum":["a"]}` renders `a`
//! and rejects null, because JSON Schema applies `type` and `enum` independently.
//!
//! **Descriptions.** Tool descriptions are always kept. Under [`DescriptionPolicy::Auto`] a
//! property description is dropped only when every word in it is already said by the property
//! name (prefix match: `duration_min` covers "minutes"), its type keyword (`datetime` covers
//! "ISO 8601"), its enum values, or an exact word of the tool name. "Event title" on `title` is
//! dropped; "Attendee emails" on `attendees` is kept. [`DescriptionPolicy::Keep`] keeps all of
//! them, and [`decode_tools`] then returns the original schema exactly.
//!
//! **Unsupported (the caller bypasses compaction for the request):** `$ref`/`$defs`, other
//! `anyOf`/`oneOf`/`allOf`/`not`, `if`/`then`/`else`, `pattern`, `minLength`/`maxLength`,
//! `minItems`/`maxItems`/`uniqueItems`, `exclusiveMinimum`/`exclusiveMaximum`, `multipleOf`,
//! `patternProperties`, `propertyNames`, schema-valued `additionalProperties`, tuple `items`,
//! other `type` unions, non-scalar enum values, other `format`s, any other keyword, and
//! non-object top-level parameters.
//!
//! # Call grammar (model → caller)
//!
//! ```text
//! output = { text | call } ;
//! call   = "<<call" ws+ tool_name ws* ( json_object ws* ">>" | ">>" ) ;
//! ```
//!
//! The scanner tracks JSON strings and escapes, so `>>`, `}` or `<<call` inside a string
//! argument never end a call; no extra escaping exists. `<<call name>>` means `{}` arguments.
//! Text outside calls is returned as text; several calls keep their order.
//!
//! | Error ([`CompactError::kind`]) | When |
//! |---|---|
//! | `unknown_tool` | the name is not in the tool list (reported as soon as the name is complete) |
//! | `invalid_arguments` | not a JSON object, a repeated key, or a schema violation: missing required field, wrong type, value outside the enum or range, extra key in a `!` object, bad `date-time`/`date`/`time`/`uuid` |
//! | `malformed_call` | no space and name after `<<call`, no `{` or `>>` after the name, missing `>>`, output ending inside a call, a call over [`MAX_CALL_BYTES`] |
//!
//! Following JSON Schema, `30.0` is a valid integer and undeclared keys pass in an open object.
//! `email` and `uri` are hints to the model only; strict checks of either reject real values.
//! A literal `<<call` in prose cannot be escaped and is read as a call.
//!
//! # Invariants
//!
//! * **Fail-closed** — the first problem fails the whole output. No call is guessed, repaired or
//!   coerced; a call that passes carries the model's arguments unchanged.
//! * **Bypass, not loss** — a schema feature the grammar cannot express makes
//!   [`encode_tools`] fail with [`CompactError::Unsupported`]; the caller sends native tools.
//! * **Chunking-invariant** — [`StreamDecoder`] gives the same result however the output is
//!   split (property-tested); [`decode_calls`] is the same decoder fed one chunk.
//! * **Deterministic** — pure string processing: no clock, RNG, I/O, environment or model call.
//! * **Bounded** — schema nesting and buffered call size are capped.

#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

mod decode;
mod encode;
mod error;
mod json;
mod parse;
mod render;
mod schema;
mod types;
mod validate;

pub use decode::{
    CALL_CLOSE, CALL_OPEN, Decoded, MAX_CALL_BYTES, StreamDecoder, StreamEvent, decode,
    decode_calls, validate_call,
};
pub use encode::{
    CALL_INSTRUCTIONS, CompactTools, EncodeOptions, encode_tools, encode_tools_with, render_calls,
};
pub use error::{CompactError, Result};
pub use render::DescriptionPolicy;
pub use types::{ToolCall, ToolDef};

/// Rebuilds tool definitions from compact definitions, so a caller can check what schema
/// information survived. Exact for [`DescriptionPolicy::Keep`] up to key order; with
/// [`DescriptionPolicy::Auto`] only redundant property descriptions are missing.
pub fn decode_tools(compact: &CompactTools) -> Result<Vec<ToolDef>> {
    parse::definitions(compact.definitions())
}

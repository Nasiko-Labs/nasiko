//! Compact tool definitions for LLM prompts, and a decoder for the calls a model writes back.
//!
//! Native tool definitions are JSON Schema, and most of their tokens are JSON punctuation and
//! repeated keywords. [`encode_tools`] writes the same information as one line per tool;
//! [`decode_calls`] and [`StreamDecoder`] turn the model's text reply back into standard tool
//! calls, checked against the schema.
//!
//! ```text
//! create_calendar_event(title:str 'Event title', start:datetime, duration_min?:int,
//!   attendees?:[str], visibility?:public|private) - Create an event in the user's calendar.
//!
//! <<call create_calendar_event {"title":"Design review","start":"2026-10-05T15:00:00+05:30"}>>
//! ```
//!
//! (The definition is a single line; it is wrapped here for width.)
//!
//! # Invariants
//!
//! * **Fail-closed** — an unknown tool, a missing required argument, a wrong type, an enum
//!   violation, an undeclared argument or a marker that does not parse is an error. No call is
//!   ever guessed, repaired or partly returned.
//! * **Never altered** — a decoded call's `arguments` is the exact text the model wrote.
//! * **Lossless or refused** — [`encode_tools`] either carries the whole schema or returns
//!   [`CompactError::Unsupported`]; it never drops a constraint. [`decode_tools`] reads the
//!   compact form back so this can be checked mechanically.
//! * **Deterministic** — pure string processing. No clock, no RNG, no I/O, no environment.
//! * **One decoding path** — [`decode_calls`] is [`StreamDecoder`] fed a single chunk, so a result
//!   cannot depend on how the output was split.
//! * **Bounded** — nesting, tool-name length and argument size all have ceilings.
//! * **UTF-8 safe** — nothing slices a `str`; all scanning is by `char`.
//!
//! # Grammar: definitions
//!
//! One tool per line.
//!
//! ```text
//! tool    = name [ "(" [ fields ] ")" [ "!" ] ] [ " - " ( quoted | rest-of-line ) ]
//! fields  = field { "," " " field }
//! field   = key [ "?" ] ":" typed
//! typed   = type [ range ] [ "|null" ] [ "=" default ] [ " " quoted ]
//! type    = "str" | "str<" format ">" | "datetime" | "int" | "num" | "bool" | "obj"
//!         | "[" typed "]"                          ; array
//!         | "{" [ fields ] "}" [ "!" ]             ; object
//!         | value "|" value { "|" value }          ; string or integer enum
//!         | quoted | integer                       ; enum with one value
//!         | "num(" number { "|" number } ")"       ; number enum
//!         | "bool(" boolean { "|" boolean } ")"    ; boolean enum
//! range   = "(" [ number ] ".." [ number ] ")"     ; inclusive, at least one bound
//! default = quoted | number | boolean | "null"
//! value   = ident | quoted | integer
//! key     = ident | quoted
//! name    = 1*( ALPHA | DIGIT | "_" | "-" | "." )
//! ident   = ( ALPHA | "_" ) *( ALPHA | DIGIT | "_" | "-" )
//! format  = 1*( ALPHA | DIGIT | "_" | "-" )
//! integer = [ "-" ] 1*DIGIT
//! number  = a JSON number
//! boolean = "true" | "false"
//! quoted  = "'" *( char | "\\" | "\'" | "\n" | "\r" | "\t" | "\u{" 1*HEX "}" ) "'"
//! ```
//!
//! * `?` marks an optional field; every other field is required.
//! * `!` is `additionalProperties: false`.
//! * `datetime` is `str<date-time>`; `obj` is an object with no declared properties.
//! * A range bounds a number's value (`minimum`/`maximum`), a string's length
//!   (`minLength`/`maxLength`) or an array's item count (`minItems`/`maxItems`):
//!   `int(1..10)`, `str(..80)`, `[str](1..)`.
//! * `|null` is `type: [T, "null"]`; on an enum, `null` is also its last listed value.
//! * `=` gives the schema's `default`, which must be a scalar: `limit?:int(1..100)=20`.
//! * A tool with no `parameters` has no parentheses; `name()` is an object with no properties.
//! * An enum of bare integers is an integer enum. String values that are not plain identifiers,
//!   or that spell a type keyword or an integer, are quoted.
//! * Text is single-quoted because the definitions travel inside a JSON string, where `"` costs
//!   an escape.
//!
//! # Grammar: calls
//!
//! ```text
//! output = { text | call }
//! call   = "<<call" ws name *ws object *ws ">>"
//! object = a JSON object
//! ```
//!
//! **Escaping.** There is none for the model to learn. The decoder finds the end of `object` by
//! tracking JSON strings and nesting, so `>>`, `}` or `<<call` inside a string argument is
//! content. Once `<<call` has been read the output is committed to being a call: anything that
//! does not complete one is an error, not text.
//!
//! # Unsupported schema features
//!
//! Supported keywords: `type`, `description`, `properties`, `required`, `items`, `enum`,
//! `format`, `default` (scalars), `minimum`/`maximum`, `minLength`/`maxLength`,
//! `minItems`/`maxItems`, and `additionalProperties: false`. Decoding enforces all of them except
//! `format` and `default`, which describe a value without constraining its JSON type.
//!
//! Everything else is refused: `$ref`/`$defs`, `oneOf`/`anyOf`/`allOf`, `not`, `const`,
//! `pattern`, `title`, `exclusiveMinimum`/`exclusiveMaximum`, `multipleOf`, `uniqueItems`,
//! `additionalProperties` set to `true` or a schema, array or object defaults, type unions other
//! than `[T, "null"]`, a bare `null` type, arrays without `items`, and mixed-type enums. One
//! unsupported tool fails the whole [`encode_tools`] call; the caller sends the native
//! definitions instead.
//!
//! # Config
//!
//! This crate never reads the environment and has no switches. Whether to compact at all is the
//! caller's decision.

#![forbid(unsafe_code)]
// UTF-8 safety and "no panics" are compiler rules here, not review rules.
#![deny(
    clippy::string_slice,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic
)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

mod decode;
mod encode;
mod error;
mod parse;
mod schema;
mod stream;
mod text;
mod types;
mod validate;

pub use decode::{decode, decode_calls};
pub use encode::{encode_tools, render_calls};
pub use error::{CompactError, Result};
pub use parse::decode_tools;
pub use stream::StreamDecoder;
pub use types::{CompactTools, Decoded, Event, ToolCall, ToolDef};

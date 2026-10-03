//! Compact tool schemas: describe tools to a model in far fewer tokens than native JSON Schema,
//! and decode the model's compact calls back into standard tool calls.
//!
//! # Definitions grammar
//!
//! One signature line per tool, required parameters first:
//!
//! ```text
//! create_calendar_event(start:datetime (Start time, ISO 8601), title:string, visibility?:public|private) - Create an event.
//! ```
//!
//! ```text
//! line     := NAME "(" [params] ")" [" - " DESCRIPTION]
//! params   := param (", " param)*
//! param    := NAME ["?"] ":" type [" " desc]  |  "..."     -- "?" optional; "..." extra keys allowed
//! desc     := "(" text with balanced parentheses ")"  |  JSON string
//! type     := alt ("|" alt)*                               -- enum literals, or T|null
//! alt      := atom ("[]")*
//! atom     := "string" | "int" | "number" | "bool" | "null" | "any" | "object" | "{}"
//!           | "datetime" | "date" | "email" | "uri"
//!           | "{" params "}" | "(" type ")" | literal
//! literal  := bare word that is not a keyword | JSON string | JSON number   -- enum values
//! ```
//!
//! # Call grammar
//!
//! ```text
//! call     := "<<call" WS NAME WS? JSON_OBJECT WS? ">>"
//! ```
//!
//! Everything outside a call is plain text. `<<` not followed by `call` and whitespace is text.
//! The scanner tracks JSON strings and nesting, so `>>` inside a string argument needs no
//! escaping: the call only closes after its argument object does.
//!
//! # Invariants
//!
//! * **Fail closed** — an unknown tool, invalid JSON, a duplicate key, a missing required field,
//!   a wrong type, an enum violation or an unknown field is an [`Error`], never a guessed call.
//!   Arguments are returned exactly as the model wrote them: nothing is coerced or dropped.
//! * **Bypass, don't approximate** — schemas the grammar cannot carry exactly (`$ref`, `oneOf`,
//!   `pattern`, numeric bounds, `default`, unknown `format`s, …) make [`encode_tools`] return
//!   [`Error::Unsupported`]; the caller sends the native request instead.
//! * **Deterministic** — pure string processing. No clock, no RNG, no IO, no environment.
//! * **Chunking-independent** — [`StreamDecoder`] gives the same result however output is split.
//!
//! Field descriptions that only repeat the field and tool names are dropped; all others are kept.

#![forbid(unsafe_code)]

mod call;
mod error;
mod parse_defs;
mod render;
mod stream;
mod ty;
mod types;
mod validate;

pub use error::{Error, Result};
pub use render::render_calls;
pub use stream::StreamDecoder;
pub use types::{CompactTools, Decoded, Event, ToolCall, ToolDef};

use ty::{clean_description, is_valid_name, obj_to_schema, params_from_schema};

/// Render tools compactly, with call instructions.
pub fn encode_tools(tools: &[ToolDef]) -> Result<CompactTools> {
    let mut lines = Vec::with_capacity(tools.len());
    let mut seen = Vec::new();
    for t in tools {
        let unsupported = |reason: String| Error::Unsupported {
            tool: t.name.clone(),
            reason,
        };
        if !is_valid_name(&t.name) {
            return Err(unsupported("tool name is not supported".into()));
        }
        if seen.contains(&&t.name) {
            return Err(unsupported("duplicate tool name".into()));
        }
        seen.push(&t.name);
        let params = params_from_schema(t.parameters.as_ref()).map_err(unsupported)?;
        let desc = t.description.as_deref().and_then(clean_description);
        lines.push(render::render_tool(&t.name, desc.as_deref(), &params));
    }
    Ok(CompactTools {
        definitions: lines.join("\n"),
        instructions: render::INSTRUCTIONS.to_string(),
    })
}

/// Decode a complete model reply into text and validated calls.
pub fn decode(text: &str, tools: &[ToolDef]) -> Result<Decoded> {
    let mut decoder = StreamDecoder::new(tools)?;
    let mut events = decoder.push(text)?;
    events.extend(decoder.finish()?);
    Ok(Decoded::from_events(events))
}

/// Decode a complete model reply into validated calls. A reply with no call yields `[]`.
pub fn decode_calls(text: &str, tools: &[ToolDef]) -> Result<Vec<ToolCall>> {
    decode(text, tools).map(|d| d.calls)
}

/// Parse compact definitions back into tool definitions with normalized JSON Schemas, so
/// schema preservation can be checked mechanically.
pub fn decode_tools(compact: &CompactTools) -> Result<Vec<ToolDef>> {
    let parsed =
        parse_defs::parse_definitions(&compact.definitions).map_err(Error::InvalidDefinitions)?;
    Ok(parsed
        .into_iter()
        .map(|t| ToolDef {
            name: t.name,
            description: t.desc,
            parameters: Some(obj_to_schema(&t.params)),
        })
        .collect())
}

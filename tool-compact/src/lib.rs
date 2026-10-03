//! Compact tool schemas for LLM routers.
//!
//! # Purpose
//!
//! Tool definitions dominate prompt tokens. This crate encodes OpenAI-style tool
//! schemas into a short text form and decodes model output back into standard
//! tool calls — validating every call against the original schema (**fail closed**).
//!
//! # Grammar
//!
//! ## Tool list (encoder → model)
//!
//! ```text
//! tool_line   := NAME '(' fields ')' (' - ' DESCRIPTION)?
//! fields      := field (', ' field)*
//! field       := NAME '?'? ':' type
//! type        := 'str' | 'int' | 'float' | 'bool' | 'datetime' | 'null'
//!              | 'object' | '[' type ']' | '{' fields '}'
//!              | enum_lit ('|' enum_lit)*
//! call_format := fixed instructions (see [`encode::CALL_FORMAT`])
//! ```
//!
//! Optional fields are marked with `?` after the name. Descriptions may be
//! shortened but are never dropped when present (they disambiguate tools).
//!
//! ## Tool calls (model → decoder)
//!
//! ```text
//! <<call NAME {json object}>>
//! ```
//!
//! - `>>` inside a JSON string value is literal; the call ends at `>>` **after**
//!   the JSON object closes.
//! - Text before/after markers is ignored.
//! - Multiple calls: one marker each (typically one per line).
//! - No markers ⇒ plain answer (empty call list).
//!
//! ## Unsupported schema features (bypass compaction)
//!
//! `$ref`, `$defs`, `definitions`, `anyOf`/`oneOf`/`allOf`/`not`, conditionals,
//! `patternProperties`, tuple `items`, `additionalProperties` as a nested schema,
//! and nesting deeper than 8. [`encode_tools`] returns
//! [`CompactError::UnsupportedSchema`] so the router can send native tools instead.
//!
//! # Invariants
//!
//! * **Fail-closed** — unknown tool / invalid args ⇒ error, never a guessed call.
//! * **Deterministic** — pure functions; no clock, RNG, I/O, or env reads.
//! * **No provider code** — the router converts to/from its IR at the seam.

#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

mod decode;
mod encode;
mod schema;
mod types;

pub use decode::{StreamDecoder, decode_calls};
pub use encode::{CALL_FORMAT, decode_tools, encode_tools, render_calls, tool_call};
pub use types::{CompactError, CompactTools, ToolCall, ToolDef};

#[cfg(test)]
mod property_tests {
    use super::*;
    use proptest::prelude::*;
    use serde_json::{Value, json};

    fn simple_tool() -> ToolDef {
        ToolDef {
            name: "echo".into(),
            description: Some("Echo a message.".into()),
            parameters: Some(json!({
                "type": "object",
                "properties": {
                    "msg": {"type": "string"},
                    "n": {"type": "integer"}
                },
                "required": ["msg"]
            })),
        }
    }

    proptest! {
        #[test]
        fn roundtrip_string_arg(msg in "[a-zA-Z0-9 _>]{0,40}") {
            let tools = [simple_tool()];
            let call = tool_call("echo", &json!({"msg": msg})).unwrap();
            let rendered = render_calls(&[call.clone()]).unwrap();
            let decoded = decode_calls(&rendered, &tools).unwrap();
            assert_eq!(decoded.len(), 1);
            assert_eq!(decoded[0].name, "echo");
            let args: Value = serde_json::from_str(&decoded[0].arguments).unwrap();
            assert_eq!(args["msg"], call_msg(&call));
        }
    }

    fn call_msg(call: &ToolCall) -> Value {
        serde_json::from_str::<Value>(&call.arguments).unwrap()["msg"].clone()
    }

    #[test]
    fn encode_decode_tools_identity() {
        let tools = vec![simple_tool()];
        let compact = encode_tools(&tools).unwrap();
        assert_eq!(decode_tools(&compact).unwrap(), tools);
    }
}

# nasiko-tool-compact

Compact prompt serialization and decoding for JSON Schema tool definitions.
This crate is a pure library and deliberately does not depend on
`nasiko-llm-router` or call an AI service. The application that owns a model
request can encode its tool definitions, include the returned text in the
prompt, and decode the model's text response.

## Library API

```rust
use nasiko_tool_compact::{decode_calls, encode_tools, ToolDef};

let tools = vec![ToolDef {
    name: "create_calendar_event".into(),
    description: Some("Create an event in the calendar.".into()),
    parameters: Some(serde_json::json!({
        "type": "object",
        "properties": {
            "title": { "type": "string", "description": "Event title" },
            "start": { "type": "string", "format": "date-time" },
            "duration_min": { "type": "integer" },
            "visibility": { "type": "string", "enum": ["public", "private"] }
        },
        "required": ["title", "start"]
    })),
}];

let compact = encode_tools(&tools)?;
let response = r#"<<call create_calendar_event {"title":"Review","start":"2026-10-05T15:00:00+05:30"}>>"#;
let calls = decode_calls(response, &tools)?;
```

The encoded prompt lists required fields as `name:type`, optional fields as
`name?:type`, arrays as `[type]`, nested objects with braces, and string enums
with `|`. Objects that leave JSON Schema's default `additionalProperties`
behavior in place include `...:any`; objects with `additionalProperties: false`
omit it. Property descriptions are retained. Supported string formats are
`date-time`, `date`, `time`, `email`, `uri`, and `uuid`.

The decoder returns only calls, ignoring ordinary text before, between, and
after them. It checks tool names, JSON syntax, required fields, and supported
types. `StreamDecoder::new(tools)` accepts chunks with `push(&str)` and returns
calls as soon as they are complete; call `finish()` at end of stream to detect
an incomplete call. `ToolCall.arguments` is serialized JSON text.

Schemas with unsupported or unrepresented constraints fail closed. This
includes combinators (`oneOf`, `anyOf`, `allOf`), references, conditional
schemas, numeric/string/array bounds, patterns, and explicitly constrained
`additionalProperties` schemas other than `false`. Keep constraints to the
supported subset or extend the compact grammar before using those schemas.

## Tests

```sh
cargo test -p nasiko-tool-compact
```

## Token evaluation and AI usage

The optional router example at `llm-router/examples/compact_tools_eval.rs`
counts the original and compact schema prompts offline with the pinned
`tiktoken-rs` `o200k_base` tokenizer. Input is JSON with an `examples` array;
each row has `id`, `tools` (the `ToolDef` JSON shape), and an optional `prompt`.

```sh
EVAL_SET=/tmp/tools.json OUT=/tmp/compact-results.jsonl \
  cargo run --release -p nasiko-llm-router --example compact_tools_eval
```

To exercise the model interaction in the evaluator, set `LIVE=1`, `LIVE_MODEL`,
and `OPENAI_API_KEY`; `OPENAI_BASE_URL` is optional and defaults to the OpenAI
API base URL. Live mode sends the compact schema in a system message to a
Chat Completions compatible endpoint and attempts to decode its text response.
The library itself has no model configuration and does not make model calls.

The router currently declares the crate dependency for the integration seam,
but does not insert compact schemas into production requests. Existing router
tool behavior therefore remains the default until a separately tested wiring
change enables it.

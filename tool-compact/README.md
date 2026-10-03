# nasiko-tool-compact

Compact tool definitions for LLM prompts, plus a decoder that turns the model's reply back into
standard tool calls. Every call is validated against the **original** JSON Schema; anything unknown
or invalid is an error, never a guessed call.

Pure library: no IO, no environment reads, no provider code, no dependency on `nasiko-llm-router`.
The router converts its own types at the seam and assigns call ids.

Tool schemas are re-sent on every request. This renders each tool as one line and asks the model to
call tools with `<<call name {"arg":value}>>`:

```text
Tools (? = optional):
get_weather(city:str, unit?:"celsius"|"fahrenheit")
|Get the current weather for a city.
To call tools, write <<call name {"arg":value}>> for each call needed (several allowed).
```

## Quick start

Encode the tools and put the result in the system prompt instead of the native `tools` array:

```rust
use nasiko_tool_compact::{ToolDef, encode_tools};
use serde_json::json;

let tools = vec![ToolDef {
    name: "get_weather".into(),
    description: Some("Get the current weather for a city.".into()),
    parameters: Some(json!({
        "type": "object",
        "properties": {
            "city": {"type": "string", "description": "City name"},
            "unit": {"type": "string", "enum": ["celsius", "fahrenheit"]}
        },
        "required": ["city"]
    })),
}];

// `Err(Error::Bypass(_))` (unsupported schema, or the compact form would not be smaller)
// means: send the native tools instead.
let compact = encode_tools(&tools).unwrap();
assert!(compact.prompt.contains(r#"get_weather(city:str, unit?:"celsius"|"fahrenheit")"#));
```

Decode the model's reply. `decode_reply` also returns the text around the calls:

```rust
use nasiko_tool_compact::{Error, ToolDef, decode_calls, decode_reply};
use serde_json::json;

let tools = vec![ToolDef {
    name: "get_weather".into(),
    description: None,
    parameters: Some(json!({
        "type": "object",
        "properties": {"city": {"type": "string"}, "unit": {"type": "string", "enum": ["celsius", "fahrenheit"]}},
        "required": ["city"]
    })),
}];

let reply = decode_reply(r#"Checking. <<call get_weather {"city":"Oslo"}>>"#, &tools).unwrap();
assert_eq!(reply.text, "Checking.");
assert_eq!(reply.calls[0].name, "get_weather");
assert_eq!(reply.calls[0].arguments, r#"{"city":"Oslo"}"#); // the model's own JSON, unmodified

// Fail closed: a bad enum value is an error and no call is returned.
let err = decode_calls(r#"<<call get_weather {"city":"Oslo","unit":"kelvin"}>>"#, &tools).unwrap_err();
assert_eq!(err.code(), "invalid_arguments");
assert!(matches!(
    decode_calls(r#"<<call delete_everything {}>>"#, &tools),
    Err(Error::UnknownTool(_))
));
```

Streaming: feed chunks as they arrive. A call is returned once, as soon as it closes and validates;
a marker split across chunks (`<<ca` + `ll …`) is held back until it is known to be text or a call.

```rust
use nasiko_tool_compact::{StreamDecoder, ToolDef};
use serde_json::json;

let tools = vec![ToolDef {
    name: "get_weather".into(),
    description: None,
    parameters: Some(json!({"type": "object", "properties": {"city": {"type": "string"}}, "required": ["city"]})),
}];

let mut decoder = StreamDecoder::new(&tools).unwrap();
let mut calls = vec![];
for chunk in ["<<ca", "ll get_weather {\"city\":\"Os", "lo\"}>", ">"] {
    calls.extend(decoder.push(chunk).unwrap());
}
calls.extend(decoder.finish().unwrap()); // an unterminated call is an error here
assert_eq!(calls.len(), 1);
```

## API

| Item | Purpose |
|---|---|
| `encode_tools(&[ToolDef]) -> Result<CompactTools>` | Render the prompt block. `Err(Error::Bypass)` means send native tools. |
| `decode_calls(text, &[ToolDef]) -> Result<Vec<ToolCall>>` | All-or-nothing: one bad call fails the whole reply. |
| `decode_reply(text, &[ToolDef]) -> Result<Reply>` | Calls plus the plain text around them. |
| `StreamDecoder` | Incremental `push(chunk)` / `finish()`; stays failed after an error. |
| `decode_tools(&CompactTools) -> Result<Vec<ToolDef>>` | Parse the prompt back, to check schema meaning survived. |
| `Error::code()` | `unknown_tool`, `invalid_arguments` (bad JSON, missing `>>` and bad values map here too), or `bypass`. |

## Guarantees

- **Deterministic:** the same tools always render the same text.
- **Fail closed:** unknown tool, missing required field, unknown key, wrong type, a float for
  `integer`, `null`, a value outside `enum` (also nested), bad JSON, non-object arguments, a missing
  `>>`, or an unterminated call all produce an error and no call.
- **Not altered:** `ToolCall::arguments` is the model's own JSON text for the object.
- **Escaping:** JSON is parsed, not scanned, so `>>` or `<<call` inside a string argument is data.
- **Bypass, never approximate:** a schema feature the format cannot represent makes `encode_tools` and
  `StreamDecoder::new` return `Error::Bypass`.

## Supported schemas

`type` (string, integer, number, boolean, array, object), `properties`, `required`, `items`, `enum`,
`format` (`date-time`, `date`, `email`), `description`, `additionalProperties: false`. Everything else
(`oneOf`/`anyOf`/`allOf`, `$ref`, `pattern`, numeric or length limits, `default`, union types,
free-form objects, …) bypasses compaction. The exact grammar and the full list are in
[GRAMMAR.md](GRAMMAR.md).

## In the router

`llm-router/src/compact_tools.rs` applies this crate to non-streaming chat requests behind
`TOKEN_COMPACT_TOOLS=1` (default off): `apply` swaps `tools` for the system block, `restore` decodes the
reply into standard `tool_calls`. Streaming, conversations with tool turns, a `tool_choice` other than
`auto`, and unsupported schemas are sent unchanged.

## Tests and evaluation

```sh
cargo test -p nasiko-tool-compact                      # unit tests, chunk-split tests and these examples
EVAL_SET=/tmp/compact-tools-eval.json OUT=/tmp/out.jsonl \
  cargo run --release -p nasiko-llm-router --example compact_tools_eval
```

On the public sample the eval measures 30.3% fewer request tokens (`o200k_base`, whole request body),
3/3 exact round trips and 5/5 decoder cases. Live runs against four model families, native
comparison, and the known limits are in the PR description.

## Limits

`format` is not validated beyond "is a string". Models can still write wrong arguments (invented
optional arguments, wrong relative dates) exactly as with native tool calling; this crate only
guarantees that what is returned matches the schema, not that it is what the user meant.

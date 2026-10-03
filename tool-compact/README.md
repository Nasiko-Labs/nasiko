# Compact tool schemas

`nasiko-tool-compact` is a pure Rust library for reducing tool-definition overhead and decoding model text into validated calls. It has no IO, environment reads, provider code or dependency on the LLM router.

```rust
use nasiko_tool_compact::{ToolDef, encode_tools, decode_tools, decode_calls, StreamDecoder};
fn demo(tools: &[ToolDef]) -> Result<(), Box<dyn std::error::Error>> {
let compact = encode_tools(tools)?;
assert_eq!(decode_tools(&compact)?, tools);
let calls = decode_calls("plain answer", tools)?;
assert!(calls.is_empty());
let mut stream = StreamDecoder::new(tools)?;
stream.push("plain ")?;
stream.push("answer")?;
assert!(stream.finish()?.is_empty());
Ok(())
}
```

The caller owns original schemas and passes them to decoding. `CompactTools` contains only transmitted definition text. `decode_tools` parses that text to reconstruct the original definitions; it does not return a hidden saved copy.

## Definition grammar

Definitions are newline-separated records. Strings and annotation values use JSON escaping.

```text
definitions := (name SP description SP schema LF)*
description := JSON_STRING | null
schema      := base required? enum? description_modifier? format? attributes?
base        := str | int | num | bool | null | object | array
             | '{' (field (',' field)*)? '}'
             | '[' schema ']'
field       := key '!'? ':' schema
key         := BARE_ATOM | JSON_STRING
required    := '!' JSON_STRING_ARRAY
enum        := '=' JSON_ARRAY
description_modifier := '~' JSON_STRING
format      := '/' 'date-time'
attributes  := '@' JSON_OBJECT
```

`name` is 1 to 64 ASCII letters/digits/underscore/hyphen/period. `BARE_ATOM` uses the same characters without the length restriction; other property names are JSON strings. `{...}` means an object with an explicit `properties` map. `[schema]` means an array with explicit `items`. Bare `object` and `array` preserve absent `properties` and `items`. `none` represents absent tool parameters, separately from a null-valued argument schema.

A field marked `!` is required; an unmarked field is optional. The encoder writes required properties first, in the original `required` list order. Explicit empty required lists or required names not described in `properties` use the separate `![]` modifier. Remaining attributes retain their full JSON Schema names and values. Modifier order is fixed as shown above.

Example:

```text
send_email "Send an email from the user's account." {to!:[str]~"Recipient emails",subject!:str~"Subject line",body!:str~"Plain-text body",cc:[str]~"CC emails"}
```

All descriptions, enums, annotations, field names and required-array order survive reconstruction. Descriptions are not shortened or removed. The prompt includes `INSTRUCTIONS` explaining the required marker and call format. Types and enum/description modifiers use readable notation; the library does not rename tools or arguments.

## Call grammar

```text
output := (text | call)*
call   := '<<call' WS name WS JSON_OBJECT WS? '>>'
        | '<<' name WS JSON_OBJECT WS? '>>'
```

Text before, between and after calls is ignored by `decode_calls`. Plain answers return no calls. `>>` and `<<call` inside a JSON string belong to that argument and do not terminate/start calls. JSON quotes and backslashes use normal JSON escaping. Extra whitespace around arguments and before the closing marker is accepted. Tool markers are reserved syntax: malformed markers and trailing partial markers starting `<<` cause `malformed_output`.

`StreamDecoder::push` accepts arbitrary UTF-8 string chunks, including split opening/closing markers and split escaped JSON strings. Calls are staged as complete values arrive. `finish` releases all calls only after the entire output succeeds. A later bad call rejects the whole result. Once `push` fails, that error remains until `finish`; pushing or finishing after completion returns `decoder_finished`.

The published `<<call ...>>` form remains canonical and is rendered by the offline example. Live models can also emit the explicitly supported `<<NAME {...}>>` form. Both are validated identically; names and arguments are never repaired. Official decoder cases are fed unchanged because their original grammar is directly supported; no conversion or expected-outcome lookup is needed.

## Validation and bypass policy

Supported schemas have an explicit single `type`: object, array, string, integer, number, boolean or null. Supported keywords:

- `properties`, `required`, `items`, `enum`, `additionalProperties` (boolean or supported schema).
- `description`, `title`, `default`, `examples` (annotations preserved).
- Numeric `minimum`, `maximum`, `exclusiveMinimum`, `exclusiveMaximum`.
- `minLength`, `maxLength`, `minItems`, `maxItems`, `uniqueItems`, `minProperties`, `maxProperties`.
- `format: date-time`, asserted through RFC3339 parsing.

Unknown tools, missing required fields, incorrect types, enum violations, forbidden additional properties and failed supported constraints reject the call. Arguments are never filled, coerced, renamed or silently changed. Duplicate JSON keys are rejected at every nesting level. Tool calls carry a typed argument object; the router serializes that object to the standard OpenAI `function.arguments` string and assigns ids.

Unsupported features cause `encode_tools` to return `unsupported_schema`. The eval and router bypass compaction for the whole request. These include `$ref`/`$defs`, unions, `oneOf`/`anyOf`/`allOf`, nullable type arrays, tuple schemas, boolean schemas, patterns, pattern properties, dependencies, unknown formats including email, `const`, unknown keywords and non-function tool types. Unknown tool/function-level fields are not discarded. An absent `additionalProperties` retains JSON Schema's permissive default. An absent parameters schema accepts any argument object.

Limits: 128 tools, 128 calls, 32 schema/validation levels, 2 MiB encoded definitions or model output. JSON parsing retains serde_json's own nesting limit. Oversize output returns `resource_limit`. Stream input is bounded, but an incomplete call is reparsed when each chunk arrives; adversarial one-byte chunks can cost more CPU than normal text deltas. Numeric comparisons use serde_json/IEEE-754 numbers; constrained values outside the exact integer range are rejected conservatively.

## Tests

```sh
cargo test -p nasiko-tool-compact
```

Tests cover schema reconstruction, constraints, multiple/plain/mixed responses, duplicate keys, every UTF-8 split position, unknown/invalid calls, unsupported features, resource limits and atomic failure. Property tests exercise arbitrary Unicode strings, chunk sizes and nested schema trees.

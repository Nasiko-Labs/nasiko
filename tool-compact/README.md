# nasiko-tool-compact

`nasiko-tool-compact` renders OpenAI-style function definitions as a compact text
prompt and decodes text calls back to validated structured values. It is pure: it
does no IO, reads no environment variables, and does not depend on the router.

## Grammar

```text
call = "<<call" ws name ws json-object ws? ">>"
```

`json-object` is ordinary JSON. The closing `>>` is recognized only after a
complete JSON object has been read, so `>>` inside a JSON string is safe. Text
before, between, and after calls is ignored by the decoder.

The compact prompt uses `str`, `int`, `num`, `bool`, `dt` (ISO-8601 datetime),
`?` (optional), `[T]` (array), `!` (no additional object keys), and `|` for
simple string enums. Tool descriptions are retained; field names and types carry
the compact field-level meaning.

The supported JSON Schema subset is `type`, `properties`, `required`, `items`,
`enum`, `description`, `format`, and boolean `additionalProperties`. Schemas using
composition, references, constraints, or other keywords are rejected by
`encode_tools`; the caller can then send those tools natively.

## Failure policy

Calls are accepted only when the tool name exists and the decoded JSON object validates
against its original schema. Unknown tools, malformed calls, missing required fields,
type mismatches, enum mismatches, and disallowed extra properties return an error. The
decoder never invents or repairs a call.

## API

```rust
let compact = nasiko_tool_compact::encode_tools(&tools)?;
let calls = nasiko_tool_compact::decode_calls(model_text, &tools)?;

let mut stream = nasiko_tool_compact::StreamDecoder::new(&tools)?;
let completed = stream.push(chunk)?;
let remaining = stream.finish()?;
```

`decode_tools(&compact)` returns the original supported definitions, which makes schema
preservation directly testable.

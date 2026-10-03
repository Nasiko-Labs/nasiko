# nasiko-tool-compact

Compact tool-definition format for LLM requests, plus a string-aware incremental
decoder that converts model output back into standard OpenAI-shaped tool calls.

## Rationale

OpenAI-shaped tool definitions are JSON Schema objects. A single realistic tool
can consume 100–300 prompt tokens. This crate renders them as terse single-line
descriptions, injects a 3-line call grammar instruction, and decodes the model's
output back into `ToolCall` objects the rest of the router handles unchanged.

## Grammar (EBNF)

```ebnf
call        := "<<call" SP name SP json_object ">>"
name        := [A-Za-z_][A-Za-z0-9_.-]*   ; must equal a declared tool name
json_object := RFC 8259 JSON object
SP          := " "
```

- **Zero-argument call**: `<<call name {}>>`.
- Multiple calls, text before/after calls, and plain answers with no call are all
  valid. Text outside markers is ignored by `decode_calls`.
- A stray `<` or `<<` that never becomes `<<call` is plain text — no error.
- The end marker `>>` is located by a string-aware, escape-aware JSON scanner
  that tracks `{}`/`[]` depth. A `>>` inside a string argument **does not** end
  the call (test: `{"body":"a >> b"}`).

## Type mapping table

| JSON Schema type / format              | Compact notation        |
|----------------------------------------|-------------------------|
| `string` (no format)                   | `str`                   |
| `string` + `format: "date-time"`       | `datetime`              |
| `string` + `format: "date"`            | `date`                  |
| `integer`                              | `int`                   |
| `number`                               | `num`                   |
| `boolean`                              | `bool`                  |
| `array` of T                           | `[T]`                   |
| `enum` values                          | `a\|b\|c`               |
| nested `object`                        | `{k:T, k2?:T}`          |
| nullable (`"type":["T","null"]`)       | type appended with `?`  |
| optional parameter (not in `required`) | name suffixed with `?`  |

## Call instruction block (verbatim; 3 lines)

```
To call a tool, write exactly: <<call name {"arg":value}>> (zero-arg: <<call name {}>>).
Place calls anywhere in your reply; text before/after is fine.
Example: <<call get_weather {"city":"London"}>>
```

## Unsupported schema features (native fallback)

Tools using any of the following are kept in native JSON form with a recorded reason:

- `$ref`, `oneOf`, `anyOf`, `allOf`, `not`
- `patternProperties`, `if`/`then`/`else`
- Tuple `items` (array `items` that is itself an array)
- Numeric constraints: `minimum`, `maximum`, `exclusiveMinimum`, `exclusiveMaximum`, `multipleOf`
- String constraints: `minLength`, `maxLength`, `pattern`
- `default` values

Native tools are returned in `CompactTools::native_tools` and must still be
passed in the `tools` field of the OpenAI request so the provider enforces them.

## Failure policy

If **any** call in a decoded output is invalid (unknown tool, type mismatch,
missing required field, extra property under `additionalProperties:false`,
out-of-range enum, `null` where not allowed, invalid date-time format, etc.),
the **entire** decode returns an error. There are no silent partial results.

## Validation coverage

- `type`: `string`, `integer`, `number`, `boolean`, `array`, `object`
- `required` / `properties` / nested objects
- `additionalProperties: false`
- `enum`
- `items` (array element validation, recursive)
- `nullable`: `"type": ["T", "null"]`
- `format: "date-time"`: RFC 3339 syntactic check (structure + offsets, not leap-second validity)
- `format: "date"`: `YYYY-MM-DD` syntactic check

## Determinism

- `encode_tools` output is byte-identical for identical input. Property iteration
  follows JSON object insertion order (preserved by `serde_json::Map`).
- `decode_calls` and `StreamDecoder` share one core state machine (`scan.rs`).
  Any split of a valid output produces the same result as one-shot decoding.
- `arguments` in decoded tool calls preserves the key order emitted by the model.

## Public API

```rust
// Encode
pub fn encode_tools(tools: &[ToolDef]) -> Result<CompactTools>;
pub fn decode_tools(compact: &CompactTools) -> Result<Vec<ToolDef>>;

// Decode
pub fn decode_calls(text: &str, tools: &[ToolDef]) -> Result<Vec<ToolCall>>;
pub struct StreamDecoder { /* incremental; push chunks, then finish */ }

// Types (mirror llm-router/src/ir/chat.rs)
pub struct ToolDef    { kind, function: FunctionDef, extra }
pub struct FunctionDef { name, description: Option<String>, parameters: Option<Value> }
pub struct ToolCall   { id, kind, function: FunctionCall, extra }
pub struct FunctionCall { name, arguments: String }  // arguments is a JSON string

// Errors
pub enum Error {
    UnknownTool(String),
    InvalidArguments { tool, reason },
    InvalidCall(String),
    UnsupportedSchema(String, String),
    UnterminatedCall,
    BodyTooLarge { limit },
    DepthExceeded { limit },
}
```

## Known limitations and trade-offs

- **Description shortening**: descriptions are truncated to the first sentence / 120
  characters. Descriptions that are a single long sentence may be truncated. The
  encoder always includes unit and format hints (e.g. "ISO 8601", "minutes") as
  part of the type annotation rather than the description.
- **Key-order preservation**: the model's key order in arguments is preserved
  (not sorted). This means two semantically identical calls with different key
  orders produce different `arguments` strings. Callers that need canonical form
  should re-parse and re-serialize.
- **Streaming**: `StreamDecoder` accumulates full call bodies before validating.
  Very large streaming tool calls will buffer up to `max_body_bytes` (default 256 KiB).
- **No `anyOf`/`oneOf`**: these fall back to native. This is the most common
  real-world limitation; tools with optional-group schemas stay native.
- **All-or-nothing error**: if call 3 of 5 is invalid, the whole decode errors.
  This is intentional — guessing or returning partial results hides schema drift.

# nasiko-tool-compact

Compact tool schemas, and a strict decoder for the calls a model writes back. Pure library: no
I/O, no env reads, no provider code, no dependency on the router (`llm-router` converts its own
`ToolDef`/`ToolCall` at the seam in `llm-router/src/tool_compact.rs`).

```rust
pub fn encode_tools(tools: &[ToolDef]) -> Result<CompactTools>;      // or Err(UnsupportedSchema) → bypass
pub fn decode_tools(compact: &CompactTools) -> Result<Vec<ToolDef>>;  // compact text → JSON Schema
pub fn decode_calls(text: &str, tools: &[ToolDef]) -> Result<Vec<ToolCall>>;
pub fn decode(text: &str, tools: &[ToolDef]) -> Result<Decoded>;      // calls + surrounding prose
pub struct StreamDecoder;                                             // push(chunk) / finish()
pub fn render_calls(calls: &[ToolCall]) -> String;                    // calls → compact text
```

## What the model sees

Native (122 tokens for this tool alone):

```json
{"type":"function","function":{"name":"create_calendar_event","description":"Create an event in the user's calendar.","parameters":{"type":"object","properties":{"title":{"type":"string","description":"Event title"}, …}}}}
```

Compact (one line, plus a shared two-line contract):

```text
Tools:
create_calendar_event(title:str "Event title", start:datetime "Start time, ISO 8601", attendees?:[str] "Attendee emails", duration_min?:int "Duration in minutes", visibility?:public|private) - Create an event in the user's calendar.
Call as <<call name {"arg":value}>>, one per call; ? marks optional args. If no tool fits, reply normally.
```

The model replies `<<call create_calendar_event {"title":"Design review","start":"2026-10-05T15:00:00+05:30"}>>`.

## Grammar

```text
line     := name "(" [field ("," " "? field)*] ")" [" - " tool_description]
field    := name ["?"] ":" type ["=" json_default] [" " json_string_description]
type     := "str" | "int" | "num" | "bool"
          | "datetime" | "date" | "time" | "email" | "uri" | "uuid"     (string + format)
          | "[" type "]" | "{" fields "}" | literal ("|" literal)*        (array, object, enum)
literal  := bare_word | json_string | json_integer

output   := (text | call)*
call     := "<<call" ws+ name ws* json_object ws* ">>"
```

No escaping is needed in calls: the argument object is scanned with a string-aware bracket
matcher, so `>>`, `}` or `<<call` inside a JSON string are inert.

## Invariants

- **Fail-closed.** Unknown tool → `unknown_tool`. Missing required field, wrong type, value
  outside an enum, unknown argument, `null` for a non-nullable field, or non-JSON arguments →
  `invalid_arguments`. Bad or unterminated marker → `malformed_call`. One bad call fails the
  whole reply; nothing is guessed, repaired or coerced.
- **Bypass, never approximate.** Schemas outside the supported subset return
  `UnsupportedSchema`, so the caller sends native tools.
- **Faithful arguments.** `ToolCall::arguments` is the model's own JSON text.
- **Deterministic.** Required fields first (in `required` order), then optional by name.
- **Streaming = buffered.** `decode_calls` is `StreamDecoder` fed one chunk; a property test
  checks arbitrary splits (including inside multi-byte characters) give identical results.

## Supported schema subset

Root `type: object` with `properties`/`required`; `string` (+ `format` date-time, date, time,
email, uri, uuid), `integer`, `number`, `boolean`, `array` + `items`, nested `object` +
`properties`; all-string or all-integer `enum`; `description` and `default` on properties;
`additionalProperties: false`; `title`/`$schema` (dropped).

**Unsupported, so compaction is bypassed:** `$ref`/`$defs`, `oneOf`/`anyOf`/`allOf`/`not`,
`const`, type unions (`["string","null"]`), numeric/length/pattern constraints, other formats,
free-form objects, open `additionalProperties`, descriptions on array items, non-identifier
property names. `format` is shown to the model but, as in JSON Schema, not enforced.

`decode_tools(encode_tools(x)) == x` up to: whitespace in descriptions collapsed, annotations
and `additionalProperties: false` dropped, enums gain an explicit `type`, and no-argument tools
normalized to `{"type":"object","properties":{}}`.

## Tests

```sh
cargo test -p nasiko-tool-compact   # unit tests + proptest properties
```

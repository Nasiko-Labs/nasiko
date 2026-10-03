# nasiko-tool-compact

Compact tool schemas: render OpenAI function tools as one-line signatures instead of JSON
Schema, and decode the model's compact calls back into standard, validated tool calls.

Pure library — no IO, no env reads, no provider code. `nasiko-llm-router` depends on it (behind
the opt-in `TOKEN_COMPACT_TOOLS` flag); it does not depend on the router.

## Example

A native tool definition (≈120 `o200k_base` tokens as JSON)…

```json
{"type":"function","function":{"name":"create_calendar_event","description":"Create an event in the user's calendar.","parameters":{"type":"object","properties":{"title":{"type":"string","description":"Event title"},"start":{"type":"string","format":"date-time","description":"Start time, ISO 8601"},"duration_min":{"type":"integer","description":"Duration in minutes"},"attendees":{"type":"array","items":{"type":"string"},"description":"Attendee emails"},"visibility":{"type":"string","enum":["public","private"]}},"required":["title","start"]}}}
```

…is injected as a system message:

```text
Tools (?=optional):
create_calendar_event(title:str 'Event title', start:datetime 'Start time, ISO 8601', attendees?:[str] 'Attendee emails', duration_min?:int 'Duration in minutes', visibility?:public|private) - Create an event in the user's calendar.
To call a tool write <<call NAME {JSON args}>>, one per call; omit unused optional args. Otherwise reply normally.
```

The model replies `<<call create_calendar_event {"title":"Design review","start":"2026-10-05T15:00:00+05:30"}>>`
and the router returns a standard `tool_calls` entry whose `arguments` is that JSON object,
byte for byte.

## API

```rust
pub fn encode_tools(tools: &[ToolDef]) -> Result<CompactTools>;     // Err(Unsupported) ⇒ bypass
pub fn decode_tools(compact: &CompactTools) -> Result<Vec<ToolDef>>; // schema recovery
pub fn decode(text: &str, tools: &[ToolDef]) -> Result<Decoded>;     // text + calls
pub fn decode_calls(text: &str, tools: &[ToolDef]) -> Result<Vec<ToolCall>>;
pub fn render_calls(calls: &[ToolCall]) -> String;                   // calls in the compact grammar
pub struct StreamDecoder; // new(tools) / push(chunk) -> Vec<StreamEvent> / finish() -> Decoded
```

`decode`/`decode_calls` are a `StreamDecoder` fed the whole text, so the streaming and
non-streaming paths cannot disagree.

## Grammar

Signatures (what the model reads) and calls (what it writes); also in the crate docs.

```text
line   := NAME "(" fields? ")" [" - " desc]
fields := field (", " field)*
field  := fname ["?"] ":" type [" " QUOTED]        ; "?" optional; QUOTED = description
type   := str | int | num | bool | obj | datetime | date | time | email | uri
        | "[" type "]" | "{" fields? "}" | lit ("|" lit)*
lit    := BARE_WORD | QUOTED | INTEGER              ; a bare word needs 2+ enum values
QUOTED := single-quoted, escapes \\ \' \n \r \t

call   := "<<call" WS+ NAME WS* [JSON_OBJECT] WS* ">>"
```

- `<<call` opens a call only when followed by whitespace (`<<callback` is text).
- The JSON object is scanned structurally (string- and escape-aware), so `>>`, `}` or `<<call`
  inside a string argument needs no escaping beyond ordinary JSON.
- Text before, between and after calls is kept; a reply with no marker is a plain answer.
- Single quotes in signatures avoid JSON-escaping cost when the prompt sits inside a request
  body (every `\"` is an extra token).

## Fail-closed decoding

Every call is validated against the **original** schema. Each of these is an error, never a
guessed or repaired call:

| condition | error |
|---|---|
| tool not offered | `unknown_tool` (raised as soon as the name is complete, mid-stream) |
| arguments not a JSON object, duplicate key, missing required field, undeclared field, wrong type, `null` for an optional field, enum violation, bad `date-time`/`date`/`time`/`email`/`uri` | `invalid_arguments` |
| `<<call` not followed by `NAME {…}>>`, or the stream ends inside a call | `malformed_call` |

One bad call fails the whole response. The `StreamDecoder` stays failed after its first error.

## Supported schemas

Root `parameters` is `{"type":"object"}` with `properties`/`required` (or absent). Properties may
use `type` (`string`, `integer`, `number`, `boolean`, `array` + `items`, `object` with or without
`properties`), `format` (`date-time`, `date`, `time`, `email`, `uri`), `enum` (all strings or all
integers) and `description`, nested to any depth.

**Unsupported → compaction bypassed** for the whole request (native tools are sent):
`$ref`/`$defs`, `anyOf`/`oneOf`/`allOf`/`not`, type unions and nullable types,
`additionalProperties`, `default`, `const`, `examples`, `title`, numeric/length/pattern
constraints, other formats, descriptions on `items` or on the root schema, duplicate tool names,
names outside `[A-Za-z0-9_.-]{1,64}`, and non-`function` tools.

`decode_tools` returns the canonical schema: identical to the input for supported schemas,
except that an empty `required` is omitted, a missing `parameters` comes back as an empty object
schema, and properties are reordered (required first, in `required` order). Descriptions are
kept verbatim, never shortened.

## Router integration (`TOKEN_COMPACT_TOOLS=true`, default off)

`llm-router/src/compact_tools.rs`, called from the chat handler after the brevity seam.

- **Covered:** non-streaming `POST /v1/chat/completions`, every provider (it runs on the IR).
- **Bypassed (sent exactly as today):** streaming, the Responses API, `tool_choice` other than
  absent/`"auto"`, conversations already containing `tool_calls`/`tool` results (so only the
  first step of a tool loop is compacted), and unsupported schemas.
- Definitions go in a new **leading** system message; author messages are untouched.
  `parallel_tool_calls` is removed with `tools`.
- A response that does not decode is a `502` (`model produced an invalid compact tool call`);
  the client never sees `<<call` markers. Usage is still logged.
- With the flag off the request and response are byte-identical (unit-tested).

## Eval

```sh
curl -fsSL https://registry.nasiko.dev/r/nasiko/compact-tools-eval -o /tmp/compact-tools-eval.json
EVAL_SET=/tmp/compact-tools-eval.json OUT=/tmp/out.jsonl \
  cargo run --release -p nasiko-llm-router --example compact_tools_eval
```

Offline and deterministic by default. Set `PROVIDER_BASE_URL` + `MODEL` (optionally
`PROVIDER_API_KEY`, and `LIVE_BASELINE=1` to also send the native request) for live mode. The
example builds every request through the router's own `compact_tools::compact`.

## Tests

```sh
cargo test -p nasiko-tool-compact   # unit, contract and property tests
cargo test -p nasiko-llm-router compact_tools
```

Property tests generate nested schemas (including names and literals full of `>>`, quotes,
backslashes, newlines and multi-byte characters) and check that `decode_tools(encode_tools(t))
== t`, that rendered calls decode back exactly at every stream split, and that noise never
produces a call.

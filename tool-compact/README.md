# nasiko-tool-compact

Compact function-tool schemas, and a decoder that turns the model's compact
calls back into OpenAI-shaped tool calls.

Tool definitions are mostly JSON Schema boilerplate (`type`, `properties`,
`required`, repeated `{"type":"string"}`). That boilerplate is prompt tokens.
This crate replaces a tool list with a short signature prompt when every tool
can be represented without dropping information. The model answers with
`<<call name {json}>>`. The decoder checks that text against the **original**
schema and returns ordinary `tool_calls`. It does not call the tools.

The crate is a pure library: no I/O, no environment variables, no provider
client. `nasiko-llm-router` may depend on it. It does not depend on the router.

## Why this is not `nasiko-compress`

`nasiko-compress` shortens tool **results** (logs, JSON payloads, diffs). It is
fail-open: any doubt returns the original text. A tool schema cannot use that
rule. Dropping `required`, an enum, or a nested field would make later calls
look valid when they are not. Compact schemas therefore live here. If a schema
uses a keyword this crate does not support, the whole list is left
uncompressed (`compacted: false`) and the caller sends the original tools.

## Grammar

```text
prompt      = "Tools\n" tool-line* hint "\n"
hint        = "Call <<call name {\"k\":\"v\"}>>"
tool-line   = name "(" fields ")" "!"? ( " " description )? "\n"
fields      = ( field ( "," field )* )?
field       = ident "?"? ":" type ( " " json-string )?
type        = union
union       = single ( "|" single )*
single      = atom suffix*
atom        = prim | format | array | object | "anyOf(" union ")" | "oneOf(" union ")"
prim        = "str" | "int" | "num" | "bool" | "null" | "json"
format      = "datetime" | "date" | "time" | "email" | "uri" | "uuid"
array       = "[" type "]"
object      = "{" fields "}"
suffix      = ">=" number | "<=" number | ">" number | "<" number
            | "#" integer? ".." integer?
            | "~" json-string
            | "!dup" | "!"
            | "=" enum-value ( "|" enum-value )*
call        = "<<call " name " " json-object ">>"
```

`?` is optional. A field without `?` is required. `!` after `)` or after an
object type means `additionalProperties: false`. `str=public|private` is a
string enum. `datetime` means `format: date-time`. `json` accepts any JSON
value. `!dup` means `uniqueItems: true`. `#1..20` is `minLength`/`maxLength`
on strings and `minItems`/`maxItems` on arrays.

### Example schema

```text
Tools
create_calendar_event(attendees?:[str],duration_min?:int>=1,start:datetime,title:str "Event title",visibility?:str=public|private)! Create a calendar event
Call <<call name {"k":"v"}>>
```

Property order follows `serde_json` map order (sorted in this workspace). The
same input always produces the same prompt.

### Example model output

```text
<<call create_calendar_event {"title":"Design review","start":"2026-10-05T15:00:00+05:30","attendees":["riya@example.com"]}>>
```

Arguments are JSON. One object, no trailing commas, no comments. Several calls
may appear in one answer, with prose before, between, or after them. An answer
with no marker has no calls.

## Why this grammar

| Form | Why it lost |
| --- | --- |
| `@tool name(args)` | `@` is common in emails and mentions, which are normal argument values. Parentheses inside strings need a second lexer. |
| `<tool:name>{...}` | Looks like markup. `>` inside prose or inside a string is a bad terminator. |
| A custom argument language | The model already emits JSON tool arguments. A second syntax is easier to get wrong and harder to validate against JSON Schema. |
| `<<call name {json}>>` | `<<call` followed by whitespace is rare in prose (`<<calling` is not a call). JSON keeps nesting, quotes, and `>>` inside strings well-defined. The closing `>>` is only read outside strings. The hint the model must copy is one short line. |

## Decoder

`decode_calls(text, tools)` scans for committed calls, in order.

- Prose is ignored.
- `<<call` plus whitespace starts a call. The call must then be complete.
- The name must be in `tools`. Duplicates are an error.
- Arguments must be one JSON object. Invalid JSON is `Error::Malformed`.
- The object is checked against the original `parameters` schema.
- Unknown tools, missing required fields, wrong enums, and wrong primitive
  types are errors. Values are not coerced and missing fields are not filled.
- Call ids are `call_0`, `call_1`, … in decode order. No clocks and no random
  ids.

`decode_tools` parses a prompt from `encode_tools` back into tool definitions
so tests can check the signature. Descriptions are whitespace-collapsed.
`nullable: true` may come back as a union with `null`. Draft-04
`exclusiveMinimum: true` plus `minimum` may come back as a numeric
`exclusiveMinimum`. Those forms accept and reject the same values. Call
validation does not use the rebuilt schema; it uses the original one.

## Streaming

`StreamDecoder` appends chunks and emits text or calls only when they are
complete. The marker, the name, JSON, strings, escapes, and `>>` may split at
any byte. `>>` inside a JSON string does not end the call.

`finish` treats a dangling prefix (`<`, `<<`, `<<call` with no following
space) as text. A call that was started (`<<call` plus whitespace) and not
closed is `Error::Malformed`.

## Validation

Checked against the original schema:

- `type` (including unions and integer-versus-number)
- `required`, `properties`
- `additionalProperties` when it is a boolean (default is allow)
- `enum`, `const`
- `items` (one schema, not tuples)
- `minimum` / `maximum` and exclusive bounds, both numeric and draft-04 boolean
- `minLength` / `maxLength`, `minItems` / `maxItems`, `uniqueItems`
- `pattern` (Rust `regex` crate, unanchored)
- `format` for `date-time`, `date`, `time`, `email`, `uri`, `uuid`
- `nullable`, `anyOf`, `oneOf`
- nested objects and arrays

Unknown formats are not compacted, so they are not silently skipped.

## Unsupported schemas (compaction is skipped)

`$ref` and `$dynamicRef`, `allOf`, `not`, `if` / `then` / `else`,
`dependentSchemas`, `dependentRequired`, `patternProperties`, `prefixItems`,
tuple `items`, `unevaluatedProperties`, `unevaluatedItems`, `contains`,
`propertyNames`, non-boolean `additionalProperties`, non-scalar enums, unknown
`format`s, tool `type` other than `function`, duplicate tool names, extension
fields on the tool object, and any other keyword.

`pattern` uses the Rust `regex` engine. A pattern that engine cannot compile
is unsupported and bypasses compaction.

## Failure

There is no closest-tool match, no default injection, and no repair of
trailing commas or quotes. A bad call fails the decode. The router, when
compact mode is on, leaves the raw assistant text in place if decoding fails
and does not invent a `tool_call`.

## Token savings

Savings come from deleting repeated schema keywords, not from deleting
descriptions that distinguish fields. Field descriptions are kept, with
whitespace collapsed. The evaluation example measures the full request body
with `tiktoken-rs` (`o200k_base`). That dependency is not part of this crate
and is not a runtime dependency of the router.

## Evaluation

```sh
EVAL_SET=/tmp/compact-tools-eval.json \
OUT=/tmp/out.jsonl \
cargo run --release -p nasiko-llm-router --example compact_tools_eval
```

`EVAL_SET` is a JSON object with `cases` (or `evals` / `items`) or a bare
array. Tools live at the top, on the case, or on `request.tools`.

Request cases use `messages`, `request`, or one of `prompt` / `query` /
`input` / `user`. The example writes one JSON object per line:

```json
{"id":"ct-001","compact_request":{},"compacted":true,"rendered_calls":"<<call ...>>","roundtrip_calls":[]}
```

Decoder cases set `chunks` (a string or an array of strings) or `text` and do
not set a user message:

```json
{"id":"dc-002","decoded":{"calls":[]}}
```

A decode failure is `decoded.error` and the process still exits 0. The process
exits non-zero only when the file cannot be read or parsed. It does not write
`results.json`.

Token counts are printed to stderr. They are not part of the JSONL, so two
runs with the same input produce the same file.

## Live mode

Set both:

- `PROVIDER_BASE_URL` — OpenAI-compatible origin, or a full `/chat/completions` URL
- `MODEL`

Optional:

- `PROVIDER_API_KEY` — bearer token. Not required to be present, and never
  committed.

The example sends `compact_request` with `temperature` 0 and records
`raw_output` plus `live_calls` on those lines. Offline runs omit those fields.

## Router integration

`LLM_COMPACT_TOOLS=true` (or `1`) turns it on. The default is off.

When the flag is off, chat requests are not passed through this crate. When it
is on and every tool can be compacted, the router removes `tools` and
`tool_choice`, prepends the compact prompt as a system message, and decodes
assistant text into OpenAI `tool_calls` before the client sees the response.
Streaming chunks are decoded the same way. Clients never see the compact
grammar on the success path.

History is not rewritten. A multi-turn transcript still contains ordinary
`tool_calls` from earlier turns. Schemas this crate cannot represent keep the
original request bytes.

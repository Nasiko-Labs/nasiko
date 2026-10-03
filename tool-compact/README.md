# nasiko-tool-compact

Compact tool definitions for LLM requests, and a validating decoder for the calls models write
back. This is a pure library: no IO, no env reads, no provider code. `nasiko-llm-router` depends on it
and converts its IR types at the seam (`llm-router/src/compact_tools.rs`).

```rust
pub fn encode_tools(tools: &[ToolDef]) -> Result<CompactTools>;          // Err ⇒ bypass
pub fn decode_calls(text: &str, tools: &[ToolDef]) -> Result<Vec<ToolCall>>;
pub fn decode(text: &str, tools: &[ToolDef]) -> Result<Decoded>;          // text + calls
pub struct StreamDecoder;  // new(tools) → push(chunk) → finish(), emits Text / Call events
pub fn decode_tools(compact: &CompactTools) -> Result<Vec<ToolDef>>;      // schema check
pub fn render_calls(calls: &[ToolCall]) -> Result<String>;                // history replay
```

## The format

Native tool calling sends the full JSON Schema on every request. The compact version is one
system message:

```text
To use a tool, reply <<call NAME {JSON args}>>, always with the word call and closing >>. Several allowed; stop after the last. Otherwise answer normally.
create_calendar_event: Create an event in the user's calendar.
 title: str
 start: str(date-time) # Start time, ISO 8601
 attendees?: str[] # Attendee emails
 duration_min?: int # Duration in minutes
 visibility?: 'public'|'private'
send_email: Send an email from the user's account.
 to: str[] # Recipient emails
 subject: str # Subject line
 body: str # Plain-text body
 cc?: str[]
```

Nested objects are wrapped in braces:

```text
create_order: Place a retail order for the user.
 items: obj[](minItems=1) { # Products to order
  sku: str # Product SKU
  qty: int(min=1) # Quantity
  gift_wrap?: bool
 }
 shipping: obj {
  name: str # Recipient full name
  address: obj {
   line1: str # Street address
   ...
  }
 }
 priority?: 'standard'|'express' # Shipping speed
```

The model replies with calls like this, with any amount of text around them:

```text
<<call send_email {"to":["sam@example.com"],"subject":"Build status","body":"The build is green."}>>
<<call create_calendar_event {"title":"Retro","start":"2026-10-04T10:00:00+05:30","duration_min":30,"visibility":"private"}>>
```

Design choices:

- **Signatures read like a TypeScript interface.** `?` means optional, `T[]` is a list, `'a'|'b'` is an enum, and `{ … }` holds the fields of an object. Models already know these conventions, so the instructions explain only the call syntax.
- **Arguments are plain JSON.** Models write JSON reliably, and the decoder can check it strictly.
- **The definitions contain no `"` characters.** Enum strings are single-quoted, so the definitions add no `\"` escapes once they are embedded in a JSON request body.
- **Required fields come first, in their `required` order.** The model sees the important fields first, and the order of `required` survives a round trip exactly.

Several of these choices came from live testing (see [Live results](#live-results)):

- **Braces around nested objects.** A first version marked nesting only with indentation. Three models from different providers then put a field that follows a nested object *inside* it (`priority` inside `shipping`). Braces fixed this, at about one token per object.
- **The instructions name the two parts models drop.** They are 35 tokens and say "always with the word call and closing >>". Without that clause, gpt-oss-120b and Ministral-8B wrote `<<send_email {…}>>` on most calls and scored 7 of 18 in compact mode each (15 and 13 with native tools). With it, they scored 13 and 15. A 28-token rewording that kept the meaning but dropped "the word call" lost half of that gain again. A 100-token version with a full notation key cost 8 points of token reduction for no measured benefit.
- **`?=optional` is left out of the instructions.** It cost 4 tokens per request. The only failure seen without it was one 8B model writing a key as `"repeat?"`, which the decoder rejects.

## Grammar

### Definitions

```text
definitions := tool ( "\n" tool )*
tool        := header ( "\n" field )*
header      := TOOL_NAME [ "(closed)" ] ":" [ " " TEXT ]
field       := INDENT FIELD_NAME [ "?" ] ": " type [ " {" ] [ " # " TEXT ]
               [ ( "\n" field )* "\n" INDENT "}" ]
               -- INDENT: one space per nesting level (top-level fields: one space).
               -- " {" appears exactly when the type is `obj` or an array of `obj`; the
               -- object's fields follow at INDENT + 1, closed by "}" at INDENT.
type        := union postfix*
postfix     := "[]" [ ann ]                       -- array of the preceding type
union       := "(" type ")"                       -- grouping, for arrays of unions
             | member ( "|" member )* [ ann ]
member      := "str" | "int" | "num" | "bool" | "null" | "any" | "obj" | literal
               -- only type names: a type union; any literal: an enum (`null` = literal)
literal     := "'" CHARS "'" | INTEGER | "true" | "false" | "null"
               -- CHARS escapes: \\  \'  \n  \r  \t
ann         := "(" item ( "," item )* ")"
item        := "closed"                           -- additionalProperties: false
             | FORMAT_WORD                        -- format: date-time, email, uri, …
             | ("min" | "max") "=" NUMBER         -- minimum / maximum
             | ("minLen" | "maxLen" | "minItems" | "maxItems") "=" INTEGER
             | "pattern=" "'" CHARS "'"
             | "default=" literal
TOOL_NAME   := [A-Za-z0-9_-]{1,64}
FIELD_NAME  := [A-Za-z0-9_.$@-]+
TEXT        := description, whitespace collapsed to single spaces, to end of line
```

### Calls

```text
output := ( TEXT | call )*
call   := "<<call" WS+ NAME WS* [ JSON_OBJECT WS* ] ">>"
```

- `NAME` runs up to the first whitespace, `{`, `(` or `>`.
- `JSON_OBJECT` is scanned with awareness of strings and escapes, so `>>`, `{` or `}` inside a string value never ends a call.
- `<<call` that is not followed by whitespace (for example `<<callback`) is ordinary text.
- `<<` directly followed by the name of an offered tool (`<<send_email {…}>>`) is a **malformed call**, not text. It is a call attempt in the wrong shape, and treating it as text would turn a failed call into a silent "no call". `<<` followed by anything else is text.
- A call without arguments means `{}`.
- Whitespace and newlines inside a call are allowed.

## Fail-closed decoding

The decoder returns a validated call or an error. It never returns a guessed or repaired call. The error codes are stable:

| code | when |
|---|---|
| `unknown_tool` | the call names a tool that was not offered |
| `invalid_arguments` | a required field is missing, an undeclared field is present, or a value has the wrong type, an enum value outside the list, or a value outside min/max, length or pattern limits |
| `malformed_call` | the call doesn't follow the grammar: bad JSON (including comments or unquoted keys), a duplicate key, a missing or short `>>` (`}>`), `name(...)` syntax, `<<tool_name` without `call`, or a call left unterminated at end of output |

The decoder deliberately does **not** accept near-misses that it could recover unambiguously, such as `}>` or a missing `call`. Live, these cost Qwen3-32B two or three cases. But the screening rule is that no error case may produce a call, and in the router a rejected reply is simply resent with native tools.

- **All or nothing.** One bad call fails the whole response, because returning only the valid calls would silently drop part of what the model asked for.
- **Arguments are unaltered.** A decoded call's `arguments` is the exact JSON text the model wrote. It is validated but never reformatted, coerced or filled with defaults.
- **Duplicate keys are rejected.** `serde_json` would silently keep the last value, while another parser might keep the first.
- **Undeclared keys are always rejected.** Schemas that allow extra keys bypass compaction instead.
- **Integers are judged by value.** `30.0` passes as an integer, as in JSON Schema, and is passed through as written.
- **`format` is not validated.** JSON Schema treats it as an annotation, and so does this crate.
- **Streaming is bounded.** `StreamDecoder` holds back only text that might still become `<<call`. It caps a single call at 1 MiB, and after its first error every later call returns that same error.

## Schema support

These are carried losslessly:

- `type` (one type, or a union of scalar types such as `["string","null"]`), `description`, `properties`, `required`, `items`, and `additionalProperties: false`
- `enum` of strings, integers, booleans or null
- `format`, `minimum`, `maximum`, `minLength`, `maxLength`, `minItems`, `maxItems`, `pattern`, and scalar `default`
- nested objects and arrays, up to 16 levels deep

These are **unsupported**. The encoder returns `unsupported_schema` and the request bypasses compaction:

- `anyOf`, `oneOf`, `allOf`, `not`, `$ref`, `$defs`, `const`, `if`/`then`/`else`, `patternProperties`, `prefixItems`, `examples`, and any other keyword not listed above
- `additionalProperties: true` or a schema; type unions that include `object` or `array`
- enums of floats or non-scalars, an enum whose `type` disagrees with its values, and non-scalar or float defaults
- a `description` on array `items`, and a description or annotations on the top-level parameters object
- property names that would need quoting, such as names containing spaces or colons
- a `pattern` the Rust `regex` engine cannot compile, such as lookaround or backreferences
- tools with extra top-level fields (for example `strict`) or a `type` other than `function`
- nesting deeper than 16 levels

`decode_tools` rebuilds the schemas from the definitions text alone, so matching it against the originals proves the meaning is in the text the model sees. It applies these documented normalizations:

- whitespace inside descriptions is collapsed
- a field description made only of words from the field and tool names is dropped, since it cannot disambiguate anything (`title` → "Event title", `cc` → "CC emails"); every description with at least one new content word is kept
- `title` and `$schema` are dropped
- objects always carry `properties`
- `items: {}` is omitted
- an untyped enum or object gains its implied `type`
- absent `parameters` becomes an empty object schema

## Router wiring (opt-in)

Set `TOKEN_COMPACT_TOOLS=true`, read in `llm-router/src/config.rs`. The default is **off**. With the flag off, `apply` returns before touching the request, and the following tests prove the request is byte-identical:

- `compact_tools::tests::flag_off_leaves_the_request_byte_identical`
- `handlers::chat::tests::compact_tools_off_sends_the_native_request_unchanged`

Covered:

- **Non-streaming chat:** OpenAI, Anthropic and Gemini inbound surfaces alike, because the rewrite happens on the shared IR. Native tools are replaced by a leading system message, and compact calls in the reply are turned back into native `tool_calls` with ids `call_{response_id}_{choice}_{n}` and `finish_reason: "tool_calls"`.
- **Conversation history:** previous assistant `tool_calls` are replayed as `<<call …>>` text. Each `tool` result becomes a `user` message headed `<<result NAME>>`.
- **Decode failure:** the router resends the original request with its native tools. It never returns a guessed call.
- **Text after the calls is dropped.** A native tool-calling turn ends at its calls. Text after them is the model describing results it hasn't received yet; gpt-oss-120b produced invented flight tables this way in live testing. Text before the first call is kept as `content`.
- **Bypass cases:** `tool_choice` other than `auto` (a forced or required call can't be guaranteed by a prompt), `parallel_tool_calls: false`, legacy `functions`, unsupported schemas, a tool result with no matching call, and a compact body that isn't smaller than the native one.

Not covered yet:

- **Streaming through the router.** `StreamDecoder` is ready and tested, but the SSE path still sends native tools.
- **Usage of a discarded compact attempt.** When a decode failure triggers the native retry, the failed attempt's tokens aren't logged.

## Eval

```sh
EVAL_SET=/tmp/compact-tools-eval.json OUT=/tmp/out.jsonl \
cargo run --release -p nasiko-llm-router --example compact_tools_eval
```

It runs offline and deterministically, and token counts (`o200k_base`) go to stderr.

- **Live mode:** set `PROVIDER_BASE_URL` and `MODEL`, and optionally `PROVIDER_API_KEY`.
- **Native baseline:** `LIVE_NATIVE=1` also sends each case with native tools on the same model, for comparison.
- **Output fields:** see the header of `llm-router/examples/compact_tools_eval.rs`.

`testdata/dev-eval.json` is our own development set in the same format. It has 18 cases over 8 tools, with:

- nested objects and arrays of objects, nullable fields, patterns and limits
- multi-call, no-call and conversation-history cases
- an escaping case and a forced `tool_choice`
- a case with all 8 tools
- 8 decoder cases

### Token reduction

| set | native | compact | reduction |
|---|---|---|---|
| public (3 cases) | 656 | 460 | **29.9%** |
| dev (18 cases) | 7,143 | 5,345 | **25.2%** |

All public and dev decoder cases give the expected calls or error codes. The definitions alone are about 55% smaller than the native JSON. Two things cap the reduction:

- **Fixed costs.** Every request pays the 35-token instructions and its own system and user messages, which cost the same in both formats. Requests with only one or two small tools save least; the dev cases with all 8 tools save about 36%.
- **Description text.** Descriptions are kept nearly verbatim, so they cost about the same in both formats.

### Live results

| model | compact | native |
|---|---|---|
| deepseek.v3.2 | **16** | 12 |
| mistral.mistral-large-3-675b-instruct | **17** | 15 |
| zai.glm-4.7 | **16** | 15 |
| mistral.ministral-3-8b-instruct | **15** | 13 |
| moonshotai.kimi-k2.5 | 16 | 16 |
| qwen.qwen3-32b | 14 | 14 |
| openai.gpt-oss-120b | 13 | **15** |
| **total** | **107 / 126** | 100 / 126 |

These are dev-set cases (out of 18) where the decoded calls had the right tools and arguments. The run used an OpenAI-compatible Bedrock endpoint at temperature 0, with both modes run on the same model in the same run. The comparison was:

- right tools, in any order
- every expected argument equal, except free-text fields, which are checked for presence and type only
- extra optional arguments allowed

What the numbers include:

- **Every model misses dev-017.** My expected filter order is arbitrary, so this is a test-set artifact. Most other misses are the same date or semantic slips in both modes, for example "next Friday" read as 2026-10-10.
- **Where compact wins,** the native mode either skipped the call (DeepSeek did this twice) or wrote a less exact argument.
- **Compact misses that native doesn't have:**
  - Qwen ends pretty-printed JSON with `}>` (rejected).
  - gpt-oss-120b sometimes drops `call` or writes nothing.
  - Ministral-8B once wrote JavaScript-style unquoted keys.
- **Single run, temperature 0, 18 cases.** Expect a point or two of noise per model.
- **Anthropic and the newer OpenAI models weren't tested.** On this endpoint they reject `/v1/chat/completions`.

## Tests

```sh
cargo test -p nasiko-tool-compact      # 31 unit + 7 property tests (400 cases each)
cargo test -p nasiko-llm-router --lib  # includes compact_tools and chat-handler wiring tests
```

The property tests generate random supported schemas and check that:

- encode → `decode_tools` gives the original schema back
- random valid arguments survive render → decode
- random chunk boundaries never change what the stream decoder returns
- a removed required field or an added undeclared field is always rejected
- random garbage never panics and never yields a call to a tool that wasn't offered

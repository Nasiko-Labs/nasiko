# nasiko-tool-compact

Compact tool schemas without breaking tool calls. A pure library: no IO, no env reads, no
provider code, and no dependency on `nasiko-llm-router` (the router depends on this crate, not
the other way round).

```text
ToolDef[] ─► encode_tools ─► CompactTools { instructions, definitions } ─► model (system message)
                 │ unsupported / not smaller ⇒ Err  (caller sends the native tools: "bypass")
model text ─► StreamDecoder (chunk-safe) ─► Text / Call events ─► validated ToolCall[] | DecodeError
```

## API

| Function | Purpose |
|---|---|
| `encode_tools(&[ToolDef]) -> Result<CompactTools, EncodeError>` | Compact the tools, or refuse (bypass) |
| `encode_tools_with(&[ToolDef], &EncodeOptions)` | Same, with `DescriptionPolicy::{Verbatim, DropRedundant}` |
| `decode_tools(&CompactTools) -> Result<Vec<ToolDef>, ParseError>` | Rebuild schemas from the **rendered text alone** |
| `normalize(&ToolDef, policy)` | The canonical schema `decode_tools` returns for a tool |
| `render_calls(&[ToolCall]) -> String` | Calls → canonical `<<call …>>` lines |
| `decode_calls(&str, &[ToolDef]) -> Result<Vec<ToolCall>, DecodeError>` | Whole text → validated calls |
| `decode(&str, &[ToolDef]) -> Result<Decoded, DecodeError>` | Whole text → text + validated calls |
| `decode_with(&str, &[ToolDef], DecodeOptions)` / `StreamDecoder::with_options` | Same, with `bare_tool_markers` (see below) |
| `StreamDecoder::{new, feed, finish}` | Incremental decoding; `decode` is built on it |
| `DecodeError::code()` | `"unknown_tool"` or `"invalid_arguments"` |

## Call language (what the model writes)

```ebnf
output = { text | call } ;
call   = "<<call" , ws1 , name , ws , json_object , ws , ">>" ;
```

1. `<<call` followed by whitespace starts a call. `<<caller`, `<<callback>>` and `a << b` stay text.
2. Once started, the decoder is committed: anything other than `NAME {json} >>` is an error,
   never re-read as text.
3. The JSON is scanned string- and escape-aware: `{`, `}`, `>>` inside strings are content.
4. Arguments are required (`<<call ping {}>>` for a tool without parameters).
5. Calls keep their order. Text may appear before, between and after them.
6. Arguments must be strict JSON. After validation the model's exact text is passed through
   verbatim as `ToolCall.arguments` (never re-serialized).

**Bare-name alias (opt-in, `DecodeOptions { bare_tool_markers: true }`).** Live models (the
OpenAI family especially) often drop the keyword and write `<<send_email {…}>>`. With the
option on, `<<NAME` also opens a call when `NAME` is *exactly* one of the request's tools and is
followed by `{` or whitespace. Arguments are validated identically; `<<unknown {…}>>`,
`<<pings`, `a << b` stay text. Default **off**, so `StreamDecoder::new`/`decode` accept only the
brief's grammar (the eval's offline decoder cases run strict); the router and the eval's live
mode turn it on.

**Errors.** An unknown tool name gives `unknown_tool` as soon as the name ends, even if broken
JSON follows. Everything else (bad syntax, bad JSON, schema violations, an incomplete stream,
over 1 MiB of arguments) gives `invalid_arguments`. **One invalid call fails the whole
decode**: there are no partial results, and the first error in stream order sets the code.

## Definition language (what the model reads)

```text
Act via lines <<call tool_name {"arg":1} >>, no narration (?=optional). Else answer normally.
## create_calendar_event: Create an event in the user's calendar.
title: str # Event title
start: datetime # Start time, ISO 8601
attendees?: [str] # Attendee emails
duration_min?: int # Duration in minutes
visibility?: public|private
```

```ebnf
defs        = tool , { "\n" , tool } ;
tool        = "## " , name , [ ": " , text_no_newline ] , "\n" , { param_line } ;
name        = ( letter | "_" ) , { letter | digit | "_" | "-" | "." } ;    (* ≤ 64, no ':' *)
param_line  = indent , key , [ "?" ] , ": " , type ,
              [ " " , range ] , [ " = " , json_scalar ] , [ " # " , text_no_newline ] , "\n" ,
              { child_line } ;                       (* children only after obj / [obj] *)
child_line  = param_line at indent + 2 ;
indent      = { "  " } ;
key         = ( letter | "_" ) , { letter | digit | "_" | "-" } ;          (* ≤ 64 *)
type        = "str" | "int" | "num" | "bool" | "null" | "obj"
            | "datetime" | "date" | "time" | "email" | "uri" | "uuid"
            | "[" , type , "]" | enum_bare | enum_json ;
enum_bare   = key , { "|" , key } ;        (* string values that are identifiers, not type keywords *)
enum_json   = "enum" , json_array ;        (* numeric enums, or strings that are not bare-safe *)
range       = [ number ] , ".." , [ number ] ;      (* minimum..maximum, int/num only *)
```

**Rules:**

- Properties are ordered required-first (in `required`-array order), then optional ones
  alphabetically, recursively.
- The text is quote-free except in `enum[…]` and string defaults. Every `"` would be escaped
  again inside the JSON request body.
- **Self-check:** the encoder parses its own output and refuses (`EncodeError::RoundTrip`)
  unless it reproduces the same schema tree.
- **Never grows:** if `system_text()` is not smaller (in bytes) than the native `tools` JSON,
  `EncodeError::NotSmaller`.

| Compact | JSON Schema |
|---|---|
| `str` `int` `num` `bool` `null` | `{"type":"string"\|"integer"\|"number"\|"boolean"\|"null"}` |
| `datetime` `date` `time` `email` `uri` `uuid` | `{"type":"string","format":"date-time"\|"date"\|…}` |
| `[T]` | `{"type":"array","items":T}` |
| `obj` + indented children | `{"type":"object","properties":{…},"required":[…]}` |
| `a\|b` / `enum[1,2]` | `{"type":"string","enum":["a","b"]}` / `{"type":"integer","enum":[1,2]}` |
| `int 1..9` | `"minimum":1,"maximum":9` |
| `= 1` | `"default":1` (scalars only) |
| `key?` | key not in `required` |

## Supported subset (whitelist), everything else is a bypass

| Supported | Notes |
|---|---|
| `type` (single string), `properties`, `required`, `items` (one schema), `enum` | `enum` values: all strings, all integers, or all numbers |
| `description` | Properties and tools (not array items); whitespace collapsed |
| `format` | `date-time`, `date`, `time`, `email`, `uri`, `uuid` |
| `default` | Scalars |
| `minimum`, `maximum` | `integer` / `number` properties |
| `additionalProperties: false` | Accepted (validation is strict anyway) |

**Bypass list (whole request goes native):**

- `anyOf`, `oneOf`, `allOf`, `not`, `$ref`, `$defs`, `type: [..]`, `additionalProperties`
  other than `false`;
- `pattern`, `minLength`/`maxLength`, `minItems`/`maxItems`, `uniqueItems`, `const`, `title`,
  `examples`;
- an object with no `properties`, a property schema `{}`, an array with no `items`,
  non-identifier property names, duplicate tool names;
- any keyword not listed above.

`EncodeError::Unsupported { tool, path, keyword }` names the exact reason.

## Normalization (what `decode_tools` returns)

`decode_tools(encode_tools(x)) == normalize(x)`. The only differences from the input are:

- descriptions are whitespace-collapsed;
- `required` is omitted when empty;
- an enum's `type` is made explicit;
- `additionalProperties: false` is dropped;
- absent `parameters` becomes `{"type":"object","properties":{}}`;
- under `DropRedundant`, parameter descriptions whose words all appear in the parameter or
  tool name are dropped.

Key order is irrelevant.

## Validation and deliberate deviations from JSON Schema

Validation is strict and fail-closed. It does no coercion, no case-folding and no nearest-enum
guessing. Failures carry a JSON-pointer path.

- **Unknown argument keys are rejected.** A hallucinated argument is not passed through.
- **`30.0` is not an `int`.** The arguments are passed through verbatim, so a typed tool would
  receive a float.
- `date-time`, `date`, `time` and `uuid` get structural checks (RFC 3339 shapes, valid
  month/day). `email` and `uri` get minimal shape checks only.

## Invariants

| Invariant | Enforced by |
|---|---|
| Fail-closed: no partial calls | `stream.rs` (first error poisons the decoder) + tests |
| Lossless where it compacts | Encoder self round trip + `decode_tools` tests |
| Never grows | `EncodeError::NotSmaller` + test |
| Deterministic | No clock/RNG/IO/hash-map iteration; golden-text and run-twice tests |
| Chunking-invariant | Exhaustive 2-way and 3-way split tests, char-by-char test, random splits |
| Panic-free, UTF-8 safe | `deny(clippy::panic, unwrap_used, expect_used, string_slice)` + 10k garbage strings |
| Bounded | `MAX_ARGS_BYTES` (1 MiB) |
| No environment | Nothing in this crate reads env vars |

## Live adherence (measured 2026-10-03, OpenRouter, temperature 0, `max_tokens` 1024)

Public sample (3 cases) + own extra cases (8), compact arm vs native tool calling, same date
line in both arms. "Exact" = same calls in order, same argument keys, equal values (free-text
fields only need to be strings).

| Model | Compact exact | Native exact | Compact valid-format | False calls |
|---|---|---|---|---|
| openai/gpt-4o-mini | 5/11 | 6/11 | 8/11 | 0 |
| openai/gpt-4.1-mini | 7/11 | 6/11 | 11/11 | 0 |
| google/gemini-2.5-flash | 7/11 | 3/11 | 11/11 | 0 |

Token reduction with these instructions: **32.2%** public, 30.0% extra (o200k_base, vs
compact-JSON native body).

How the instruction line was chosen (gpt-4o-mini unless noted):

| Instruction | Public tokens | Observed failure |
|---|---|---|
| `To use a tool write <<call NAME {JSON args}>> per call …` (v1) | 33.1% | `{…}}>>` stray brace; narration without calling |
| `Call tools directly as <<call NAME JSON_OBJECT>> …` | 34.0% | Dropped `call` (`<<send_email {…}>>`); stray brace |
| `Act by writing calls like <<call get_time {"tz":"UTC"}>> …` | 31.2% | Dropped `call` on 5/8 |
| `Each tool call is one line: <<call tool_name {"arg":1}>> …` | 29.0% | Below token target |
| **`Act via lines <<call tool_name {"arg":1} >>, no narration …`** (shipped) | **32.2%** | Remaining errors are schema mistakes the validator rightly rejects |

Remaining compact failures are genuine argument errors (`"to":"a, b"` for an array, keys put
inside the wrong nested object, a stray `}`); native calling hides these because the provider
enforces the schema. The decoder rejects them (fail-closed) rather than repairing them.

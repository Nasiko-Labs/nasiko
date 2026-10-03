# nasiko-tool-compact

Compact tool schemas for LLM requests, plus a fail-closed decoder for the calls a model writes
back.

Native tool definitions are JSON Schema, and every request pays for them in prompt tokens.
`encode_tools` renders the same definitions as short, line-oriented text that goes in a system
message. The model replies with `<<call NAME {json}>>`. `decode_calls`, `decode_reply` and
`StreamDecoder` turn that reply back into standard tool calls, validated against the original
schema. `decode_tools` parses the compact text back into JSON Schema, which proves the round
trip instead of asserting it.

The crate is a pure library: no I/O, no environment reads, no clock, no randomness, and no
provider code. `nasiko-llm-router` depends on it, never the other way round.

```rust
use nasiko_tool_compact::{decode_calls, encode_tools, ToolDef};

let tools: Vec<ToolDef> = serde_json::from_value(openai_tools_json)?;   // {"type":"function",…}
let compact = encode_tools(&tools)?;          // Err(Unsupported) ⇒ send the native tools instead
let system_prompt = compact.system_prompt();  // instruction + definitions
let calls = decode_calls(&model_reply, &tools)?;   // Err ⇒ no call is returned at all
```

## Grammar

### Definitions (what the model reads)

```ebnf
definitions = tool { LF tool } ;
tool        = name [ " (" annotations ")" ] ":" [ " " description ] { LF property } ;
property    = indent key [ "?" ] ": " type [ " = " json ] [ " #" [ " " description ] ]
              { LF property } ;                 (* deeper indent: fields of an object *)
indent      = " " { " " } ;                     (* one space per nesting level *)
type        = term { "|" term } ;
term        = atom [ " (" annotations ")" ] { "[]" [ " (" annotations ")" ] } ;
atom        = keyword [ "<" format ">" ] | "(" type ")" | word | json-literal ;
keyword     = "string" | "integer" | "number" | "boolean" | "null" | "object" | "array"
            | "any" | "datetime" | "date" | "time" ;
annotations = annotation { ", " annotation } ;
```

- **`?` marks an optional property.** Required properties come first, in the order of the
  schema's `required` array, so that order survives the round trip. Optional properties follow
  in key order.
- **`datetime`, `date` and `time`** stand for `string` with that `format`. Any other format is
  written `string<email>`, `integer<int64>` and so on. `format` is an annotation, not an
  assertion.
- **A type is either a union or an enum.**
  - If every alternative is a type keyword, it is a type union: `string|null`, `string[]|null`.
  - Otherwise the alternatives are enum values. A value is written bare when it is a plain word
    (`public|private`) and JSON-quoted otherwise (`"New York"|"string"|3`).
  - `(type=…)` gives the declared type when it differs from the type the values imply.
  - `(const)` marks a single value as a `const`.
  - Enum and const values used as array items are always grouped: `(a|b)[]`, `(a)[]`.
- **Annotations:**

  | Group | Annotations |
  |---|---|
  | Numbers | `min=`, `max=`, `xmin=`, `xmax=`, `step=` |
  | Strings | `minlen=`, `maxlen=`, `pattern="…"` |
  | Arrays | `minitems=`, `maxitems=`, `unique` |
  | Objects | `closed` / `open` (`additionalProperties` false/true), `any` (free-form, no `properties`), `noreq` (`required: []`) |
  | Tool header only | `strict` / `nonstrict` (`function.strict`), `noparams` (no `parameters` at all) |

- **Descriptions** are kept verbatim on one line. `\`, line feed, carriage return and tab are
  written as `\\`, `\n`, `\r` and `\t`; other control characters as `\u00XX`.
- **Defaults** are written as JSON after ` = `. They are documentation only and are never
  injected into a call.

Example, as `create_calendar_event` renders:

```text
Call tools with <<call NAME {JSON args}>>.
create_calendar_event: Create an event in the user's calendar.
 title: string # Event title
 start: datetime # Start time, ISO 8601
 attendees?: string[] # Attendee emails
 duration_min?: integer # Duration in minutes
 visibility?: public|private
```

### Calls (what the model writes)

```ebnf
reply = { text | call } ;
call  = "<<call" ( ws | "{" | ">" ) { ws } name { ws } [ object ] { ws } ">>" ;
name  = any characters up to whitespace, "{" or ">" ;   (* looked up exactly *)
ws    = " " | TAB | CR | LF ;
```

- `<<call` followed by whitespace, `{` or `>` always opens a call. `<<callback` and `<<CALL`
  are plain text.
- The arguments are one RFC 8259 JSON object, scanned with string awareness, so `>>`, `}` or
  `<<call` inside a string is just content. An omitted object means `{}`.
- Text before, between and after calls is returned as text, and calls keep their order.

## Fail-closed decoding

Each of these is an error, and the whole reply then yields no calls:

| Input | Code |
|---|---|
| A name that is not one of the offered tools (no fuzzy matching; checked before the arguments are parsed) | `unknown_tool` |
| Missing required argument, wrong type, enum or `const` violation, broken bound, undeclared key on an object that declares `properties`, duplicate JSON key, malformed JSON, an integer too large to represent exactly | `invalid_arguments` |
| `<<call>>` with no name, missing `>>`, a reply that ends inside a call, a call over 1 MiB | `invalid_arguments` |

- Nothing is repaired: no trailing-comma fixes, no quote fixes, no defaults filled in.
- An integer written as `30.0` is valid for `integer`, as JSON Schema defines it, and it is
  passed through exactly as written.

`StreamDecoder` is a character-level state machine. Text is released as soon as it cannot start
a marker. Calls are released only by `finish()`, so a valid call is never handed out before a
later broken one voids the reply. Where a chunk ends cannot change the result. The tests check
every split point, every pair of split points, and one-character chunks.

## Supported schema subset, and bypass

Compaction is **lossless or bypassed**. `encode_tools` returns `CompactError::Unsupported`, with
a JSON-pointer `path` and a `feature` label, rather than drop anything.

| Rendered losslessly | Bypassed (send the native tools) |
|---|---|
| `type` (and type arrays of two or more types), `properties`, `required`, nested objects, `items`, arrays of arrays | `$ref`/`$defs`, `anyOf`/`oneOf`/`allOf`, `not`, `if`/`then`/`else` |
| `enum` (strings, numbers, booleans, `null`, mixed), `const` (scalars) | `patternProperties`, `prefixItems`, `dependent*`, `propertyNames`, `contains`, `unevaluated*` |
| `description` (tool and every property), `default`, `format` | `title`, `examples`, `$schema`, `$comment` and any unknown keyword |
| `minimum`, `maximum`, `exclusiveMinimum`, `exclusiveMaximum`, `multipleOf` | `additionalProperties` given as a schema; `minProperties`/`maxProperties` |
| `minLength`, `maxLength`, `pattern`, `minItems`, `maxItems`, `uniqueItems` | Tool or property names outside the grammar's character set |
| `additionalProperties: true/false`, `function.strict` | A description or default on `items`; `required` naming an undeclared property |

**Validation covers more than the renderer.** A bypassed tool is still validated when called:
- `anyOf`, `oneOf`, `allOf`, local non-recursive `$ref`, `not`, `if`/`then`/`else`,
  `dependentRequired`, `propertyNames` and `contains` are all enforced.
- A keyword that cannot be enforced (`patternProperties`, `prefixItems`, `dependentSchemas`,
  `unevaluated*`) makes every call to that tool fail closed, rather than pass unchecked.

`decode_tools` returns exactly the original schema for everything in the left column. The only
difference is that object keys come back in `serde_json`'s order.

## Invariants and where they are tested

| Invariant | Tests |
|---|---|
| Names of tools and arguments never change | `encode`/`decode_tools` unit tests, `tests/properties.rs`, `tests/edge_cases.rs::many_tools_keep_every_name_and_order` |
| Required arguments are never invented; defaults are never injected | `tests/decoding.rs`, `tests/edge_cases.rs::defaults_are_never_injected_into_a_call` |
| Unknown tools, invalid types and enum violations fail closed | `tests/decoding.rs` (failure-first matrix), property tests |
| Unsupported features bypass | `schema`/`encode` unit tests, `tests/edge_cases.rs` |
| Chunk boundaries do not matter | `tests/streaming.rs` (exhaustive), `tests/properties.rs` |
| `>>` inside strings, escapes, Unicode | `tests/decoding.rs`, `tests/properties.rs` |
| Malformed output never becomes a call | `tests/decoding.rs`, property `mutated_replies_…` |
| The schema round trip is exact | `decode_tools` fixtures, property `generated_schemas_roundtrip_exactly` |
| Determinism | `encoding_is_deterministic`; the eval example run twice and diffed |

## Known limits

- **`pattern` uses Rust `regex` syntax.** Patterns that only ECMA-262 accepts (lookaround,
  backreferences) bypass, and calls to such tools fail closed. Character classes such as `\d`
  match Unicode digits here, but only ASCII in JavaScript.
- **Numeric bounds** are compared as `f64`.
- **Pydantic-style schemas usually bypass,** because they put a `title` on every property.
- **History is not handled here.** Conversation history with earlier tool calls has to be
  re-rendered by the caller; the router bypasses such requests.

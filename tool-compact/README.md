# nasiko-tool-compact

A compact, reversible notation for OpenAI-style function tool definitions, and a fail-closed
decoder for tool calls written as `<<call name {json}>>`.

The crate is a pure library: no I/O, no environment, no network, no provider code, no tokenizer.
It depends on `serde`, `serde_json` and `thiserror` only. `nasiko-llm-router` depends on it and
converts at the seam; this crate never depends on the router.

## What it does

```text
native tools (JSON Schema)  ──encode_tools──▶  one signature line per tool
                                                  + a fixed header and call protocol

model reply text            ──decode_calls──▶  validated calls, or one typed error
                               StreamDecoder      (chunk by chunk, same result)
```

```rust
use nasiko_tool_compact::{ToolDef, decode_calls, encode_tools};

let tools = vec![ToolDef {
    name: "create_calendar_event".into(),
    description: Some("Create an event in the user's calendar.".into()),
    parameters: Some(serde_json::json!({
        "type": "object",
        "properties": {
            "title": {"type": "string", "description": "Event title"},
            "start": {"type": "string", "format": "date-time"},
            "visibility": {"type": "string", "enum": ["public", "private"]}
        },
        "required": ["title", "start"]
    })),
}];

let compact = encode_tools(&tools)?;
assert_eq!(
    compact.definitions(),
    "create_calendar_event(title:str 'Event title', start:datetime, visibility?:public|private) - Create an event in the user's calendar."
);
let prompt = compact.prompt(); // header + definitions + call protocol: put it in a system message

let decoded = decode_calls(
    "Booking it.\n<<call create_calendar_event {\"title\":\"Design review\",\"start\":\"2026-10-05T15:00:00+05:30\"}>>",
    &tools,
)?;
assert_eq!(decoded.calls[0].name, "create_calendar_event");
assert_eq!(decoded.content, "Booking it.\n");
```

The router assigns call ids, decides placement and enablement, and maps `kind != "function"`
tools out before calling this crate. In Nasiko, enablement is the operator's `TOKEN_COMPACT_TOOLS`
flag plus each agent's own switch (`agents.compact_tools_enabled`, on the agent's Settings tab).

## Public API

| Item | Purpose |
|---|---|
| `encode_tools(&[ToolDef]) -> Result<CompactTools>` | Render a catalog. Fails if any tool is unsupported, so the caller can keep the native request. |
| `decode_tools(&CompactTools) -> Result<Vec<ToolDef>>` | Parse the compact **text** back. For every accepted catalog, `decode_tools(&encode_tools(t)?)? == t`. |
| `decode_calls(text, &[ToolDef]) -> Result<Decoded>` | Decode a whole reply. Equivalent to one `push` and `finish`. |
| `StreamDecoder::new(&[ToolDef])`, `push(&str)`, `finish()` | Incremental decoding. Catalog errors surface at `new`. |
| `validate_call(name, arguments_json, &[ToolDef]) -> Result<ToolCall>` | The decoder's validation and per-call limits applied to a call that arrived in another shape (a provider's native tool call). |
| `validate_calls(&[(name, arguments_json)], &[ToolDef]) -> Result<Vec<ToolCall>>` | A whole native batch: `MAX_CALLS` and `MAX_TOTAL_ARGS_BYTES` are checked before any call is parsed; any failing call fails the batch. |
| `encode_call(name, &Value) -> String` | The one renderer of the call grammar (used by evaluation fixtures). |
| `canonical_json(&Value) -> String` | Compact JSON with recursively sorted keys. |
| `HEADER`, `INSTRUCTIONS`, `limits::*` | The framing text and every bound. |

`ToolDef { name, description: Option<String>, parameters: Option<Value> }` mirrors the OpenAI
`function` object. `ToolCall { name, arguments: Value }` always carries a JSON object.

## Invariants

- **Fail-closed.** An unsupported schema, invalid catalog or exceeded limit is an error from
  `encode_tools` and `StreamDecoder::new`; the caller bypasses to the native request. A reply
  that cannot be decoded and validated is an error; no call is guessed, repaired or coerced.
  There is no schema-free decoding mode.
- **Atomic release.** `finish` returns every validated call or an error with none. The first
  error poisons a `StreamDecoder`; every later `push` and `finish` returns the same error.
- **Reversible.** `encode_tools` re-parses each rendered line and refuses to return one that does
  not reproduce its input exactly (`ToolDef` equality, `serde_json::Value` equality for
  `parameters`). This self-check catches encoder mistakes; it is not evidence that the validator
  is correct (the differential tests against an independent JSON Schema validator are) or that a
  model understands the notation (live runs are).
- **Deterministic.** Pure functions of their input. Object keys are sorted explicitly wherever a
  `serde_json::Map` is iterated, so output does not depend on the `preserve_order` feature.
- **Bounded.** Every buffer, count and depth has a constant in `limits.rs`, checked before the
  allocation or recursion it guards where the input allows (see "Limits").

## Compact notation

One line per tool. The parser accepts exactly what the renderer emits (it only ever reads this
crate's own output), which keeps it small and makes the round-trip check meaningful.

```ebnf
tool    = name [ params ] [ " - " tool-desc ]       ; params absent  <=> parameters is absent (None)
params  = "(*)"                                      ; parameters is {} (any object), with optional
        | "(...)" flags                              ;   annotations/description after it
        | "()" flags                                 ; {"type":"object"} without "properties"
        | "(" arg *(", " arg) ")" flags              ; {"type":"object","properties":{}}
          [ annot ] [ " " desc ]                      ; root annotations and root description
flags   = [ "!" | "+" ] [ "=" ]                      ; ! additionalProperties:false   + :true
                                                     ; = "required": [] is present
arg     = argname [ "?" ] ":" type [ annot ] [ " " desc ]   ; ? = not in "required"
type    = "*"                                        ; {}  (anything)
        | scalar [ constraints ] [ "|null" ]
        | array [ "|null" ]
        | object [ "|null" ]
        | enum                                       ; nullability = a "null" member
scalar  = "str" | "int" | "num" | "bool" | "null"
        | "datetime" | "date" | "time" | "email" | "uri" | "uuid"   ; str with that "format"
constraints = "(" str-kv *("," str-kv) ")"           ; str: min=<u64> max=<u64> format=<bare|json-string>, in that order
            | "(" [number] ".." [number] ")"         ; int/num: inclusive minimum..maximum
            | "(" num-kv *("," num-kv) ")"           ; int/num with any exclusive bound: ge= gt= le= lt=, in that order
array   = "[" type [ annot ] [ " " desc ] "]" [ "(" [u64] ".." [u64] ")" ]   ; items, minItems..maxItems
        | "[...]" [ "(" [u64] ".." [u64] ")" ]       ; array without "items"
object  = "{" arg *(", " arg) "}" flags | "{}" flags | "{...}" flags
enum    = alt *("|" alt)                             ; >= 1 non-null member; all strings or all integers
alt     = bare | json-string | number | "null"
bare    = [A-Za-z_][A-Za-z0-9_.-]*  and not a reserved word (str int num bool null datetime date time email uri uuid true false)
annot   = "@" json-object                            ; keys title, default, examples in that order, values verbatim
desc    = "'" <no ' \ or control chars> "'" | json-string
tool-desc = raw | json-string                        ; raw: non-empty, no control chars, no leading/trailing
                                                     ;      whitespace, not starting with "
name    = [A-Za-z0-9_.-]{1,64}        argname = [A-Za-z_][A-Za-z0-9_.-]{0,63}
```

Rendering rules that make the line canonical:

- Required properties first, in `required` order; then the optional ones sorted by name. The
  `required` array therefore round-trips with no extra sigil.
- Numbers (bounds, enum members) are written and read with `serde_json::Number`, so `1` and
  `1.0`, `-0`, `u64::MAX` and `1e22` all survive.
- Enum strings are bare when they look like identifiers and are not reserved words; otherwise
  JSON-quoted (`"2"`, `"a b"`, `"str"`).
- `title`, `default` and `examples` are carried verbatim as `@{...}` annotations. The validator
  ignores them; defaults are never applied to returned arguments.
- `format` is an annotation. `datetime` means "a string whose schema says `format: date-time`";
  the decoder does **not** check that the value is a valid date-time.

Examples:

```text
get_time
reset()!= - Reset everything.
raw_json(*) - Accepts any object
search(query:str(min=1,max=200) 'Search terms', limit?:int(1..100)@{"default":10}, lang?:en|fr|"zh-Hans"|null) - Web search
schedule(when:datetime, attendees:[email](1..), repeat?:daily|weekly|"2"|null, meta?:{...}+) - Book a slot
move(dx:num(gt=0,le=1), pos:{x:int, y:int}!, tags?:[str|null]|null) - "Move the \"thing\""
```

## Supported JSON Schema dialect

Anything outside this list makes the whole catalog unsupported (bypass is all or nothing: a
request is either fully compact or fully native).

| Supported | Notes |
|---|---|
| root `{}` or `{"description": …}` | "any object" |
| root `{"type": "object", …}` | not nullable |
| `type` ∈ string, integer, number, boolean, null, array, object | |
| `type: [T, "null"]` | exactly this shape and order |
| `enum` on string (string members) or integer (number members) | a nullable enum lists `null` once; a non-nullable one does not |
| `format` on strings | kept verbatim, not validated |
| `minimum`, `maximum`, `exclusiveMinimum`, `exclusiveMaximum` | on integer/number |
| `minLength`, `maxLength`, `minItems`, `maxItems` | non-negative integers |
| `items` (single schema) or absent | |
| `properties` (object-valued, identifier-shaped names), `required`, `additionalProperties` (boolean) | `required` names must be declared and unique |
| `description`, `title`, `default`, `examples` | anywhere |

| Declined (bypass) | Why |
|---|---|
| `$ref`, `$defs`, `definitions`, `$schema`, `$id` | references and metadata |
| `allOf`, `anyOf`, `oneOf`, `not`, `if`/`then`/`else`, `const` | combinators |
| `pattern`, `patternProperties`, `propertyNames`, `multipleOf`, `uniqueItems`, `contains`, `dependencies` | constraints the validator does not implement |
| `deprecated`, `readOnly`, `writeOnly`, any unknown keyword | not carried |
| tuple `items`, schema-valued `additionalProperties`, `enum` with other constraints, `enum` on number/boolean | not representable |
| `type: ["string","null"]` with an `enum` that lacks `null` | JSON Schema says `null` is invalid here; the notation cannot say "nullable but null not allowed", so the schema is declined rather than approximated |
| property names outside `[A-Za-z_][A-Za-z0-9_.-]*` | not representable |

## Call grammar

```ebnf
output   = *( text / call )
call     = "<<call" WS1 toolname *WSP ( json-object / "(" *WSP ")" ) *WSP ">>"
         / "<<call" WS1 toolname *WSP ">>"
toolname = 1*64( ALPHA / DIGIT / "_" / "." / "-" )
text     = anything; "<<" not followed by "call" and whitespace is text
```

- `>>` or `}` inside a JSON string does not end the call: the object's end is found by tracking
  string/escape state and bracket depth.
- Whitespace after the name is optional (`<<call ping{}>>` is accepted); whitespace between the
  marker and the name is required, so `<<callx` and `<<caller` are text.
- `<<call ping>>` and `<<call ping()>>` mean `<<call ping {}>>`. Models copy the signature line
  for a tool without arguments, so the empty object may be left out or written as `()`; anything
  inside the parentheses is `malformed_call`, and the empty object is still validated, so a tool
  with required arguments rejects these spellings as `invalid_arguments`.
- Prose before, between and after calls is returned verbatim in `Decoded.content`.
- A literal `<<call` followed by whitespace in prose cannot be escaped: it starts a call, and if
  that call does not complete validly the whole reply fails. This ambiguity is accepted and
  documented rather than hidden. A reply ending in a bare `<<call` (no whitespace yet) is prose.

## Validation

Arguments are validated against the **original** schema, with JSON Schema semantics:

| Check | Rule |
|---|---|
| strings, booleans, null | exact kind, no coercion (`"1"` is not a number, `"true"` is not a boolean) |
| integer | a number with no fractional part (`1.0` passes, `1.5` fails) |
| bounds, enum membership | exact comparison: integers as `i128`, integer vs float by the float's floor, float vs float as `f64`; `2^53` and `2^53+1` are different |
| `minLength`/`maxLength` | code points |
| `format` | not checked |
| required | every listed name present |
| extra properties | allowed unless `additionalProperties: false` |
| defaults | never applied |

Error messages carry the tool name, a JSON-pointer path and a reason, never the offending value.

## Errors and atomic release

| `kind()` | When |
|---|---|
| `unknown_tool` | the name after `<<call` is not in the catalog |
| `invalid_arguments` | the arguments parsed but violate the schema |
| `malformed_call` | bad framing, JSON syntax error, duplicate object key, non-object arguments |
| `incomplete_call` | the output ended inside a call (`finish` while a call is open) |
| `limit_exceeded` | any constant in `limits.rs` |
| `unsupported_schema` | a tool uses a declined keyword or shape (reported at encode / `new`) |
| `invalid_catalog` | duplicate or invalid tool names; a compact line that does not parse |

Calls are validated as each `>>` arrives but released only by `finish`. A valid first call
followed by an invalid second call releases nothing. `validate_call` and `validate_calls` apply
the same per-call and batch limits to calls that did not come through the decoder, with the size
checks done before parsing, so a native batch can never exceed what the decoder would accept.

## Limits

| Constant | Value | Guards |
|---|---:|---|
| `MAX_TOOLS` | 128 | tools per catalog |
| `MAX_SCHEMA_BYTES` | 256 KiB | serialized catalog, checked before lowering |
| `MAX_SCHEMA_NODES` | 4096 | schema nodes per catalog, counted during lowering |
| `MAX_SCHEMA_DEPTH` | 32 | nesting of one parameters schema |
| `MAX_PROPERTIES` | 256 | properties per object |
| `MAX_ENUM_MEMBERS` | 256 | members per enum |
| `MAX_DESCRIPTION_BYTES` | 4096 | one description, title or serialized default/examples |
| `MAX_COMPACT_BYTES` | 256 KiB | rendered catalog |
| `MAX_NAME_LEN` | 64 | tool and property names |
| `MAX_CALLS` | 32 | calls per reply |
| `MAX_ARGS_BYTES` | 256 KiB | one call's JSON |
| `MAX_TOTAL_ARGS_BYTES` | 512 KiB | all calls' JSON in one reply |
| `MAX_DEPTH` | 32 | nesting inside arguments |
| `MAX_RESPONSE_BYTES` | 1 MiB | everything pushed into one decoder, prose included |

## Tests

```sh
cargo test -p nasiko-tool-compact
```

`tests/` holds one file per area: notation goldens, round trips (including 2,000 generated
catalogs), the supported/declined matrix, catalog rules, validation, a differential suite
against the `jsonschema` crate (dev-dependency only, Draft 2020-12, formats off, no reference
resolution) plus hand-written conformance fixtures, decoder behaviour, every chunk split of
representative replies, and the crate invariants over an adversarial corpus.

## Not in this version

Subtree factoring of repeated schemas, byte-level `push` with partial UTF-8 carry, quoted
property names, and any streaming of partially decoded calls (a call is released only when the
whole reply has been seen).

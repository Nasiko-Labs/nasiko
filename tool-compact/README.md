# nasiko-tool-compact

Compact tool schemas for the `compact-tools` track. A native OpenAI tool list is replaced by one
signature line per tool plus a single call instruction. The model replies with
`<<call NAME {json}>>`, which is decoded and validated against the original schema. Anything the
grammar cannot represent exactly is **bypassed**: the caller sends native tools
(`compacted: false`).

```text
create_calendar_event(title:str "Event title", start:datetime "Start time, ISO 8601", attendees?:[str] "Attendee emails", duration_min?:int "Duration in minutes", visibility?:enum public|private) - Create an event in the user's calendar.
send_email(to:[str] "Recipient emails", subject:str "Subject line", body:str "Plain-text body", cc?:[str] "CC emails") - Send an email from the user's account.

To call a tool, emit: <<call NAME {json args}>>
? = optional; args are one JSON object; datetime = RFC 3339 with offset, e.g. 2026-01-31T09:30:00+01:00
```

The `datetime = ...` part of the notes line is added only when some tool has a `datetime`
field. Its example timestamp is deliberately unrelated to any eval answer.

On the public sample set, `llm-router/examples/compact_tools_eval.rs` measures these `o200k_base`
token totals across the three full request bodies (native: 743; 0 of 3 bypassed in every mode).
Both sides include the reference-time system message. They are local estimates; the scorer's
own counts decide.

| `Instructions` | Instructions after the tool lines | Compact tokens | Saved |
|---|---|---|---|
| `Balanced` (eval default) | `Always call tools directly, never ask: <<call NAME ARGS>> (ARGS: JSON object). ?=optional. datetime=RFC3339+offset`, with parameter descriptions as `(text)` | 505 | 32.0% |
| `Terse` | `Call tools ONLY as <<call NAME {json args}>>, each ending >>` / `?=omit unless given; datetime=RFC3339+offset` | 518 | 30.3% |
| `Minimal` (library default) | `To call a tool, emit: <<call NAME {json args}>>` / `? = optional; args are one JSON object; datetime = RFC 3339 with offset, e.g. …` | 596 | 19.8% |
| `Example` | as `Minimal`, plus one sample call | 692 | 6.9% |

`Balanced` makes three changes:
- **Brace-free placeholder** (`<<call NAME ARGS>>`). Three live runs on two models
  (`gpt-4o-mini`, `qwen3-next-80b`) closed a call with an extra `}`, consistent with
  `{json args}` being read as a wrapper around the JSON.
- **No "omit unless given".** In live runs that wording seemed to make the model ask about
  optional fields instead of calling.
- **`(text)` descriptions.** These pay for "always call directly, never ask", because `(text)`
  costs one token less than a JSON string inside a JSON request body.

### Live format adherence

These are measured runs on the 3 public cases at temperature 0. A case passes when the reply
decodes and calls exactly the expected tools; argument values are not compared. The samples are
far too small for strong claims.

| Mode | `gpt-4o-mini` | `qwen3-next-80b-a3b-instruct` | Total | Native (both) |
|---|---|---|---|---|
| `Balanced` | 2/3 | 1/3 | 3/6 | 6/6 |
| `Terse` | 1/3 | 2/3 | 3/6 | 6/6 |
| `Balanced` with `<<call NAME {"arg":"value"}>>` (not kept) | 1/3 | 1/3 | 2/6 | 6/6 |

`Balanced` and `Terse` tie, so the eval example defaults to `Balanced`, which saves more. The
remaining failures are not format edge cases the decoder could accept. Some replies are prose
that announces a call without making it. Others are Python keyword-argument calls
(`create_calendar_event(title="…")`) that copy the tool-line syntax: `qwen3-next-80b` wrote
these with `Balanced`. Both are rejected, never repaired.

## API

| Item | Purpose |
|---|---|
| `encode_tools(&[ToolDef]) -> Result<CompactTools>` | `Compact { prompt }` or `Bypass { tools, reason }` |
| `encode_tools_with(&[ToolDef], &EncodeOptions)` | With `tool_choice` and `Instructions::{Minimal, Example, Terse, Balanced}` |
| `encode_tools_with_choice(&[ToolDef], Option<&Value>)` | Shorthand for the `tool_choice` rule only |
| `decode_tools(&CompactTools) -> Result<Vec<ToolDef>>` | Inverse of `encode_tools` (a bypass returns its tools) |
| `decode_calls(&str, &[ToolDef]) -> Result<Decoded>` | `Decoded { text, calls }`: the reply without its call markers, and the validated calls |
| `StreamDecoder::{new, with_limit, push, finish}` | Same as `decode_calls`, fed chunk by chunk; `push` fails past `max_bytes` |
| `render_call(&ToolCall) -> String` | The exact form the model is told to emit |
| `ToolDef::{from_openai, to_openai}` | Convert to and from `{"type":"function","function":{..}}` |

Error codes (`Error::code()`) are `unknown_tool`, `invalid_arguments`, `invalid_tools` and
`malformed_compact`.

## Grammar (EBNF)

```ebnf
prompt      = tool_line , { "\n" , tool_line } , "\n\n" , instruction , "\n" , notes ,
              [ "\n" , example ] ;
instruction = "To call a tool, emit: <<call NAME {json args}>>"
            | "Call tools ONLY as <<call NAME {json args}>>, each ending >>" ;  (* Terse *)
notes       = "? = optional; args are one JSON object" ,
              [ "; datetime = RFC 3339 with offset, e.g. 2026-01-31T09:30:00+01:00" ]
            | "?=omit unless given" , [ "; datetime=RFC3339+offset" ] ;      (* Terse *)
example     = "Example: " , call ;                     (* Instructions::Example only *)

tool_line   = name , "(" , [ fields ] , ")" , [ "!" ] , [ " - " , tool_desc ] ;
tool_desc   = bare_text | json_string ;
              (* bare: non-empty, no CR/LF, no leading/trailing space, not starting with '"';
                 anything else is written as a JSON string *)

fields      = field , { "," , ws , field } ;
field       = name , [ "?" ] , ":" , type ;            (* "?" = optional *)
type        = base , [ "<" , ann , { "," , ann } , ">" ] ,
              [ "(" , paren_text , ")" | ws , json_string ] ;
                                                       (* annotations, then description *)
paren_text  = ? one or more chars, none of ( ) CR LF ? ; (* written by Instructions::Balanced *)
base        = "str" | "int" | "num" | "bool" | "datetime" | "date"
            | "enum " , enum_value , { "|" , enum_value }
            | "[" , type , "]"                          (* array of type *)
            | "{" , [ fields ] , "}" , [ "!" ] ;        (* nested object *)
ann         = name                                      (* str only: its format *)
            | ( "min" | "max" | "gt" | "lt" ) , "=" , json_number         (* int, num *)
            | ( "minlen" | "maxlen" ) , "=" , count                       (* str, datetime, date *)
            | ( "minitems" | "maxitems" ) , "=" , count | "unique" ;      (* arrays *)

name        = name_char , { name_char } ;              (* [A-Za-z0-9_-] *)
enum_value  = enum_char , { enum_char } ;              (* alphanumeric or _ - . : / + @ *)
json_string = ? a JSON string literal, RFC 8259 escapes ? ;

call        = "<<" , ws , "call" , ws1 , name , ws , json_object , ws , ">>" ;
              (* "call" in any letter case; ws is any Unicode whitespace *)
```

`!` after `)` or `}` means `additionalProperties: false` (a closed object).

| Compact | JSON Schema |
|---|---|
| `str` | `{"type":"string"}` |
| `str<email>` | `{"type":"string","format":"email"}`; any `[A-Za-z0-9_-]+` format |
| `int` / `num` / `bool` | `{"type":"integer"}` / `"number"` / `"boolean"` |
| `datetime` | `{"type":"string","format":"date-time"}` |
| `date` | `{"type":"string","format":"date"}` |
| `enum a\|b` | `{"type":"string","enum":["a","b"]}` |
| `[T]` | `{"type":"array","items":T}` |
| `{a:str, b?:int}` | `{"type":"object","properties":{..},"required":["a"]}` |
| `{..}!` and `name(..)!` | adds `"additionalProperties":false` |
| `int<min=1,max=9>` | `minimum`, `maximum`; `gt` and `lt` map to `exclusiveMinimum` and `exclusiveMaximum` |
| `str<minlen=1,maxlen=64>` | `minLength`, `maxLength` |
| `[T]<minitems=1,maxitems=3,unique>` | `minItems`, `maxItems`, `uniqueItems: true` |
| `T "text"` or `T(text)` | adds `"description":"text"` |

**Field order** is deterministic: required fields in the schema's `required` order, then
optional fields sorted by name. Hash-map iteration order is never used, and annotations are
always written in the order of the grammar above.

**Descriptions** are kept verbatim; nothing is shortened. Shortening is safe only where a
description does not disambiguate, and this crate cannot judge that. Parameter descriptions are
always JSON-quoted, so commas, parentheses, `>>`, quotes and newlines are all safe.

## Calls and validation (fail-closed)

* **Marker rule:** `<<`, optional whitespace, then `call` in any letter case.
  * **Whitespace after `call`** means a call was intended. The tool name and then `{` must
    follow, or it is `invalid_arguments`. This covers `<<call send_email: {..}>>`,
    `<<call "send_email" {..}>>`, `<<call send_email args {..}>>` and
    `<<call send_email>>`.
  * **Text glued to `call`:** a name and `{` (`<<callsend_email {..}>>`) is
    `invalid_arguments`. Without `{` (`<<callback>>`), the `<<` is prose, like `a << b`.
  * Also `invalid_arguments`, to stay fail-closed: `<<call {..}` with no name, and input that
    ends right after `<<call` or `<<call name` (a truncated stream).
* The JSON object is read with serde_json's streaming deserializer, so `>>` or `<<call x {}>>`
  inside a string is data. `>>` is required after the object, with optional (Unicode)
  whitespace before it.
* Text before, between and after calls is allowed. Multiple calls are allowed.
  `Decoded.text` is the input with **only** the call markers removed. Every other byte is kept
  as-is, with no trimming and no joining. A reply with no call returns the text unchanged and no
  calls. An error returns no text.
* `unknown_tool`: the name is not in the tool list, including `NAME` echoed from the
  instruction. This is checked before the arguments are parsed. Names match exactly, so
  `send_email2` never resolves to `send_email`.
* `invalid_arguments` covers all of these:
  * bad JSON, non-object arguments, an unterminated call, or a missing `>>`;
  * a missing required field, a wrong type, `null`, or an enum violation;
  * a bad `date-time` / `date` (RFC 3339);
  * a violated `min`, `max`, `gt`, `lt`, length, item-count or `unique` limit;
  * a **duplicate key** at any depth, including one spelled with escapes such as `"a"`;
  * an **undeclared argument**, whether or not the object is closed.
* Numeric limits compare exactly when both the value and the bound are integers, and as `f64`
  otherwise.
* Other formats (`email`, `uri`, ...) are shown to the model and round-trip, but are **not
  validated**. A format is only an annotation in JSON Schema, so this matches what native tool
  calling enforces. `pattern` is a real constraint that would need a regex engine, so it is
  bypassed instead (see below).
* Nothing is coerced. `int` accepts integer literals only, so `"30"`, `30.0` and `3e1` are all
  `invalid_arguments`.
* One bad call fails the whole reply.
* `StreamDecoder` buffers the chunks and decodes on `finish()`, so chunk boundaries can never
  change the result. A test checks every split point. `StreamDecoder::with_limit(tools,
  max_bytes)` caps the buffer. Once a `push` would exceed it, that `push` and every later call,
  including `finish`, fail with `invalid_arguments`. The crate reads no configuration, so the
  limit is the caller's.

## `tool_choice`

Absent, `null` or `"auto"` is compacted. `"none"`, `"required"`, a named function, or any
unrecognised value is bypassed (reason `tool_choice`), because those need the provider's native
enforcement.

## Unsupported → bypass

Any of the following anywhere in `parameters` produces `CompactTools::Bypass` with reason
`unsupported_schema`. The reason includes the tool and the JSON path, e.g. `$.a[]`.

* Combinators and references: `oneOf`, `anyOf`, `allOf`, `not`, `if`/`then`/`else`, `$ref`,
  `$defs`, `definitions`.
* `const`, `default`, `examples`, `title`, `nullable`, `deprecated`, `readOnly`, `writeOnly`,
  `multipleOf`.
* `additionalProperties: true` or a schema. Absent and `false` are supported.
* `pattern`: it cannot be enforced without a regex engine, and an unenforced limit is not
  fail-closed.
* Other object and array keywords: `patternProperties`, `propertyNames`, `minProperties`,
  `maxProperties`, `dependentRequired`, `prefixItems`, tuple `items`, `contains`,
  `uniqueItems: false`. Also arrays without `items`, and objects without a `properties` map
  (a free-form object).
* A limit on the wrong type, such as `minimum` on a string or any limit on an enum. Also
  draft-4 boolean `exclusiveMinimum` / `exclusiveMaximum`, and length or item counts that are not
  non-negative integers.
* A `format` that is not a plain `[A-Za-z0-9_-]+` name.
* Type unions (`"type": ["string","null"]`), a missing `type`, `"type": "null"`.
* `enum` on a non-string type, non-string or duplicate enum values, an empty enum, and enum
  values with characters outside `[alnum _ - . : / + @]`.
* A `description` on the top-level `parameters` object, or a non-string description.
* `required` naming a missing property, duplicated, or not an array.
* Tool or property names outside `[A-Za-z0-9_-]`.
* Nesting deeper than `MAX_DEPTH` (16). The encoder and the signature parser share one rule: a
  top-level field's node is depth 1, and array items and object fields are one deeper than their
  parent. An empty object at exactly depth 16 is accepted; depth 17 is refused.
* Function-level `strict` (rejected by `ToolDef::from_openai`), because constrained decoding
  cannot be guaranteed once the tools are text.

Three other cases also bypass:

* An empty tool list (reason `no_tools`).
* A prompt that is not smaller than the minified native `tools` JSON (reason `not_smaller`), so
  the result is never worse than baseline. The eval example applies the same check to the whole
  request body.

Duplicate tool names are an error (`invalid_tools`) rather than a bypass, because decoded calls
would be ambiguous.

## Normalizations (where round-trip is not byte-exact)

* `parameters: None` decodes as `{"type":"object","properties":{}}`.
* `"required": []` is dropped.
* Key order inside schema objects is not preserved. Equality is order-insensitive.

## Router IR note

`ToolDef::from_openai` refuses any key it cannot carry (for example `function.strict`), so a lossy
conversion is never silent. The eval harness reads raw schemas from the `EVAL_SET` JSON, so
`strict` and other extra fields are visible there. The router IR is different: `FunctionDef` in
`llm-router/src/ir/chat.rs` has no `extra` map and **drops** them today, as do unknown keys on
`function`. Router wiring is out of scope for this crate.

## Known limits

* **`Balanced` nudges the model to call.** Its wording ("Always call tools directly, never
  ask") changes behaviour beyond the call format: a model that would have asked a clarifying
  question is pushed to call with what it has. It is opt-in only. The library default is
  `Minimal`, and `Balanced` is selected only by the eval example's default or
  `COMPACT_INSTRUCTIONS=balanced`.
* **Live format adherence trails native.** On the public samples compact reaches 3/6 against
  6/6 for native (see the live table above).
* Floats in arguments are parsed with serde_json's default parser, which can be off by one ulp on
  17-digit values. Numeric limits compare exactly when both sides are integers, and as `f64`
  otherwise.
* `StreamDecoder` does not emit text or calls incrementally; everything is returned at
  `finish()`. A router streaming to clients would hold the reply until the end. True
  incremental emission (releasing text that provably cannot start a marker) is not
  implemented.
* `tool_choice: "required"` and a named function are bypassed, so the provider enforces them
  natively. A possible next step is to compact them anyway: instruct "you must call at least
  one tool" (or that tool), and fail closed when no call is decoded.

## Tests

`cargo test -p nasiko-tool-compact` runs unit, integration and proptest suites. The proptests
generate random schemas with formats, closed objects and limits. The sample tools are embedded
in `tests/common`, so the git-excluded eval file is never read.

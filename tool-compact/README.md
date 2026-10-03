# nasiko-tool-compact

Compact tool definitions for LLM prompts, and a decoder for the calls a model writes back.

Native tool definitions are JSON Schema wrapped in the OpenAI `{"type": "function", "function": {...}}` envelope; most of their tokens are JSON punctuation and repeated keywords (`"type"`, `"properties"`, `"description"`, …). `encode_tools` writes the same information as one line per tool; `decode_calls` / `StreamDecoder` turn the model's text reply back into standard `{name, arguments}` tool calls, validated against the original schema.

```text
create_calendar_event(title:str 'Event title', start:datetime, duration_min?:int,
  attendees?:[str], visibility?:public|private) - Create an event in the user's calendar.

<<call create_calendar_event {"title":"Design review","start":"2026-10-05T15:00:00+05:30"}>>
```

(The definition is a single line; wrapped above for width.)

Pure library: no I/O, no env reads, no provider dependency. `nasiko-llm-router` depends on this crate, not the other way around.

## Highlights

- **Lossless or refused.** `decode_tools(encode_tools(x)) == x` exactly — proven by property tests over generated schemas, not just examples. A schema feature that cannot be carried is refused with the keyword named, never dropped.
- **Real-world schemas compact.** Ranges, defaults, nullable types in both the `type: [T, "null"]` and Pydantic `anyOf: [T, null]` spellings, titles, number/boolean enums, nested objects and arrays of objects — the shapes FastMCP/Pydantic tools actually ship with.
- **One decoding path.** `decode_calls` is `StreamDecoder` fed one chunk, and a property test checks that every way of splitting any output decodes identically.
- **Fail-closed, shown live.** Every malformed call a weaker model wrote was rejected; no call is ever guessed, repaired, or partly returned.
- **The brief's own `<<call …>>` grammar.** Decoder cases run exactly as given — no conversion step to get wrong on unseen cases.
- **House style.** Same invariants and lints as `nasiko-compress` (`forbid(unsafe_code)`, no `unwrap`/`expect`/`panic`/string slicing); deterministic, byte-identical eval output.

## Grammar

### Definitions

One tool per line.

```text
tool    = name [ "(" [ fields ] ")" [ "!" ] [ "@" quoted ] ] [ " - " ( quoted | rest-of-line ) ]
fields  = field { "," " " field }
field   = key [ "?" ] ":" typed
typed   = type [ range ] [ null ] [ "=" default ] [ "@" [ quoted ] ] [ " " quoted ]
null    = "|null" | "|null~" | "|null~~"
type    = "str" | "str<" format ">" | "datetime" | "int" | "num" | "bool" | "obj"
        | "[" typed "]"                          ; array
        | "{" [ fields ] "}" [ "!" ]             ; object
        | value "|" value { "|" value }          ; string or integer enum
        | quoted | integer                       ; enum with one value
        | "num(" number { "|" number } ")"       ; number enum
        | "bool(" boolean { "|" boolean } ")"    ; boolean enum
range   = "(" [ number ] ".." [ number ] ")"     ; inclusive, at least one bound
default = quoted | number | boolean | "null"
value   = ident | quoted | integer
key     = ident | quoted
name    = 1*( ALPHA | DIGIT | "_" | "-" | "." )
ident   = ( ALPHA | "_" ) *( ALPHA | DIGIT | "_" | "-" )
format  = 1*( ALPHA | DIGIT | "_" | "-" )
integer = [ "-" ] 1*DIGIT
number  = a JSON number
boolean = "true" | "false"
quoted  = "'" *( char | "\\" | "\'" | "\n" | "\r" | "\t" | "\u{" 1*HEX "}" ) "'"
```

Key sigils and shorthands:

- `?` marks an optional field; every other field is required.
- `!` after `)` or `}` is `additionalProperties: false`.
- `datetime` is `str<date-time>`; `obj` is an object with no declared properties.
- A range bounds a number's value (`minimum`/`maximum`), a string's length (`minLength`/`maxLength`), or an array's item count (`minItems`/`maxItems`): `int(1..10)`, `str(..80)`, `[str](1..)`.
- `|null` is `type: [T, "null"]`; `|null~` is `anyOf: [T, {"type": "null"}]` (Pydantic's spelling); `|null~~` is the same with the null branch first. All three admit null alike — the marks differ only so the schema round-trips back to the exact spelling it had.
- `@'Title'` carries the schema's `title` annotation; bare `@` on a field means the title is the field name re-cased (`min_score` → `Min Score`), which is the one-character form Pydantic emits for every field.
- `=` gives the schema's `default`, which must be a scalar: `limit?:int(1..100)=20`.
- A tool with no `parameters` has no parentheses; `name()` is an object with no properties.
- String values that are not plain identifiers, or that spell a type keyword or an integer, are single-quoted.
- Text is single-quoted throughout because definitions travel inside a JSON string where `"` costs an escape.

### Calls

```text
output = { text | call }
call   = "<<call" ws name *ws object *ws ">>"
object = a JSON object
```

The model is shown: `To call a tool, emit <<call name {json args}>>, one per call. Otherwise reply in plain text.`

**No escaping.** The decoder finds the end of `object` by tracking JSON strings and brace/bracket nesting, so `>>`, `}`, or `<<call` inside a string argument is content, not structure. Once `<<call` has been read, the output is committed to being a call: anything that does not complete one is an error — a half-written call is never returned as plain text, and never silently dropped.

## Schema Support

**Supported keywords:** `type`, `description`, `title`, `properties`, `required`, `items`, `enum`, `format`, `default` (scalars only), `minimum`/`maximum`, `minLength`/`maxLength`, `minItems`/`maxItems`, `additionalProperties: false`, and `anyOf` in exactly the shape `[T, {"type": "null"}]` (either order). Decoding enforces all of them except `format`, `default`, and `title`, which describe a value without constraining its JSON type.

**Unsupported (fail-closed):** `encode_tools` returns `Err(Unsupported { tool, reason })` naming the tool and the offending keyword — never a silent drop. One unsupported tool fails the whole call; the caller falls back to sending native definitions for the entire request.

| Feature | Why refused |
|---|---|
| `$ref` / `$defs` | The grammar has no indirection; a reference could point anywhere, including at itself. |
| `oneOf` / `allOf`, and `anyOf` other than `[T, null]` | A real union needs the model to pick a branch; the compact form has no reliable way to validate against whichever branch matches. |
| `pattern` | Enforcing it would require a regex engine whose dialect matches the provider's; a mismatch would reject valid calls or accept invalid ones. |
| `const`, `exclusiveMinimum`/`exclusiveMaximum`, `multipleOf`, `uniqueItems` | Real constraints this crate does not yet enforce; carrying them without enforcing them would make compact validation weaker than the schema it claims to represent. |
| `additionalProperties: true` or as a schema | The decoder already rejects undeclared keys — the opposite of what an open `additionalProperties` asks for. |
| Type unions other than `[T, "null"]`, bare `null` type, arrays without `items`, mixed-type enums, non-scalar defaults | Each is a real constraint that cannot be round-tripped through the compact form. |

## Public API

```rust
pub fn encode_tools(tools: &[ToolDef]) -> Result<CompactTools>;
pub fn decode_tools(compact: &CompactTools) -> Result<Vec<ToolDef>>;
pub fn decode_calls(text: &str, tools: &[ToolDef]) -> Result<Vec<ToolCall>>;
pub fn render_calls(calls: &[ToolCall]) -> Result<String>;
pub struct StreamDecoder { /* incremental; handles markers split across chunks */ }
```

`ToolDef { name, description, parameters }` and `ToolCall { name, arguments }` are this crate's own types — `arguments` is a JSON string, exactly as OpenAI's wire format expects. `CompactTools::prompt()` returns the full block to prepend to the model: a header line, one definition line per tool, and the call-format instruction.

## Running the Tests

```sh
cargo test -p nasiko-tool-compact                        # unit tests, invariants, stress set
cargo test -p nasiko-tool-compact --test properties      # proptest generators
cargo clippy -p nasiko-tool-compact --all-targets -- -D warnings
```

- `tests/invariants.rs` — the crate's invariants through the public API only: escaping, malformed output, markers split across chunks, multiple calls, plain answers, every fail-closed error.
- `tests/properties.rs` — proptest generators covering the full feature set: schema round-trips exactly, decoding is insensitive to how the text was chunked, the decoder never panics.
- `tests/stress.rs` — a hand-written set of realistic tool schemas with no overlap with the public eval sample.

## Running the Eval

```sh
curl -fsSL https://registry.nasiko.dev/r/nasiko/compact-tools-eval -o /tmp/compact-tools-eval.json
EVAL_SET=/tmp/compact-tools-eval.json OUT=/tmp/out.jsonl \
  cargo run --release -p nasiko-llm-router --example compact_tools_eval
```

On Windows PowerShell, `curl` is an alias for `Invoke-WebRequest`; use `curl.exe` and `$env:` variables:

```powershell
curl.exe -fsSL https://registry.nasiko.dev/r/nasiko/compact-tools-eval -o "$env:TEMP\compact-tools-eval.json"
$env:EVAL_SET = "$env:TEMP\compact-tools-eval.json"; $env:OUT = "$env:TEMP\out.jsonl"
cargo run --release -p nasiko-llm-router --example compact_tools_eval
```

Offline and deterministic by default: writes one JSONL line per case to `OUT` and prints a human-readable summary to stdout (per-case native vs compact token counts, one tool definition before/after, a pass/fail table for decoder cases).

**Live mode** (optional, for checking model format adherence): set `PROVIDER_BASE_URL` and `MODEL` (and `PROVIDER_API_KEY` if the endpoint requires one; it is sent as `Authorization: Bearer`). Each compact request is sent to `{PROVIDER_BASE_URL}/chat/completions` with `model` and `temperature: 0`; the output line gains `raw_output` and `live_calls`. Only live requests carry the brief's reference-time line (`Today is Friday 2026-10-02, timezone Asia/Kolkata.`): it is context for resolving relative dates, not part of compaction, and the scorer's native baseline is built from `tools` and `messages` alone — so the offline request is compared like for like.

## Results

### Public sample set (`compact-tools-eval@v1-sample`)

- **Token reduction: 32.9%** (656 → 440 tokens) against the scorer's baseline, `{messages, tools}` as each case gives them — counted with `tiktoken-rs` `o200k_base` over the full request body. Per case: 36.4%, 35.1%, 22.7%; the fixed call-format text (29 tokens) weighs most on the one-tool case.
- **Bypassed:** 0 of 3 cases (`create_calendar_event` and `send_email` both compact fully).
- **Round trip:** 3 of 3 — the expected calls, written in the compact grammar, decode back to themselves.
- **Decoder cases:** 5 of 5 pass, including the split-marker case and the `>>`-inside-a-string case.
- **Determinism:** two runs give byte-identical `OUT`.
- **Crate tests:** 77 pass (44 unit, 19 invariant, 10 property, 4 stress).

### Stress set (`tests/stress.rs`, no overlap with the sample — reported separately)

24 hand-written schemas covering nested objects, arrays of objects, every scalar and enum kind (including number and boolean enums), nullable types in both spellings, ranges, scalar defaults, titles, tools with no parameters or no description, and descriptions with apostrophes, quotes, newlines, and non-ASCII text — including three written the way Pydantic/FastMCP emits them (every field titled, optionals as `anyOf` with a `null` default).

- **17 of 24 compact and round-trip exactly** — `decode_tools(encode_tools(x))` returns the original schema, a rendered call decodes back to itself, and every call the schema rejects is an error. Definitions shrink from 8,313 to 3,301 bytes across the 17.
- **7 bypassed**, each refused for the keyword named in the error: `oneOf`; a real `anyOf` union; `$ref` inside an optional; `pattern`; `additionalProperties` as a schema; `$defs`; `exclusiveMinimum`.

### Live model results

Tested via an OpenAI-compatible endpoint (`PROVIDER_BASE_URL` + `MODEL`; no network by default). Three models across repeated runs:

- **GPT-OSS-120B** and **Mistral-Large-3** — valid, correctly-decoded compact calls on every case.
- **Qwen3-32B** — produced a malformed closing marker (`}}>` instead of `>>`) on roughly 5 of 8 call attempts across 4 runs; the decoder correctly rejected every one rather than guessing.
- **Claude models** — not tested; not served on the provided route.

Note: models sometimes get relative-date arithmetic wrong (a model reasoning issue, not a decoder issue). The public sample also disagrees with its own reference date in one place: with today = 2026-10-02, ct-002's "tomorrow" is 2026-10-03, but its expected `start` is 2026-10-04. Nothing here special-cases it.

## Known Limits

- **Not wired into the router.** This crate is a standalone library plus the eval example (`llm-router/examples/compact_tools_eval.rs`); `nasiko-llm-router` depends on it but nothing in `llm-router/src/` calls it yet. Encoding/decoding tool calls on real requests is a bonus item, not done.
- **Small live sample size.** Format-adherence results come from a handful of runs per model on the three eval call cases; the Qwen result in particular is a small sample.
- **Schema coverage is deliberately partial.** `pattern`, `oneOf`, `$ref`, and the other unsupported features are refused rather than approximated — by design.

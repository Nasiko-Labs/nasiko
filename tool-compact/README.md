# nasiko-tool-compact

Compact tool schemas for LLM function calling. It renders OpenAI-style function tools into a
short prompt block, and decodes the model's replies back into standard tool calls. It is
pure Rust: no I/O, no environment reads, no tokenizer, no provider code.

The crate is built on two invariants:

* **Fail-closed.** A schema feature the compact form cannot express exactly is never
  approximated. The caller sends native tools instead. A call that does not validate against
  its original schema is an error, never a guessed call.
* **Deterministic.** The same tools always render to the same bytes. No clock, no RNG.

This is the **aggressive** profile: it trims descriptions to what the field line does not already
say. Types, required fields, enum values and defaults are never changed, and the encoder proves
that on every call. See [comparison.md](comparison.md) for how this compares with other
tool-schema compression approaches.

## Contents

- [A2 grammar](#a2-grammar)
- [Call grammar](#call-grammar)
- [Capability matrix](#capability-matrix)
- [Fail-closed and fallback rules](#fail-closed-and-fallback-rules)
- [API](#api)
- [Testing and results](#testing-and-results)
- [Comparison with other approaches](comparison.md)

## A2 grammar

`encode_tools` renders a tool list as one block. The block is line-oriented:

```text
create_event # Create a calendar event
 start:datetime
 title:str
 attendees?:[email]
 limit?:int=10
 options?:{}!
  visibility:public|private|"on hold"
get_time()
Call tools as <<call NAME {JSON}>> (no XML/tags)
```

| Element | Meaning |
|---|---|
| `NAME` | Tool name, `[A-Za-z0-9_.-]+`. |
| `()` after the name | Tool takes no parameters. |
| `!` after the name | Top-level object is closed (`additionalProperties: false`). |
| ` # DESCRIPTION` | Optional description, trimmed (see below). |
| Field line | One space of indent per nesting level, then `NAME`, optional `?`, `:TYPE`, optional `=DEFAULT`. |
| `=DEFAULT` | The field's `default`, as a JSON literal (`=10`, `="A"`, `=false`). |
| Field order | Required fields first, then optional, each alphabetical. |
| `str int num bool datetime date email uri` | Scalar types. `datetime`, `date`, `email`, `uri` are string formats. |
| `[TYPE]` | Array. |
| `{}` / `{}!` | Object (its fields follow one level deeper). `{}!` is closed. |
| `a\|b\|c` | String enum. A value that is not a plain word is written as a JSON string: `Foundation\|"SR Legacy"`. |
| `1\|2\|3` | Integer enum. |
| Last line | The call instruction. Always the last line. |

### Description trimming

Descriptions carry only what the line does not already say:

- Repeated spaces are collapsed, and leading/trailing spaces and trailing periods are dropped.
- On optional fields, a leading "Optional" or "(optional)" is dropped, since `?` already says it.
- A description is left out entirely when every word in it already appears in the tool name,
  the field path, the type (for example "time" and "ISO 8601" for `datetime`), or filler
  ("the", "of", "user", …). For example, "Event title" on `create_event.title` is dropped;
  "Duration in minutes" is kept.

### Self-check

Every `encode_tools` call decodes its own output and fails, so the caller sends native tools,
unless:

- the block decodes to exactly what was meant to be shown, and
- with descriptions removed, the decoded schema is identical to the original.

## Call grammar

The model is asked to emit calls in this form, and nothing else counts as a call:

```text
call  = "<<call" WS+ NAME WS* OBJECT WS* ">>"
NAME  = [A-Za-z0-9_.-]+
OBJECT = a JSON object (strict JSON, duplicate keys rejected)
```

- Text outside calls is free prose.
- Once `<<call` appears, what follows must be a complete, well-formed call. Anything else is
  `Malformed`.
- `>>` or `}` inside a JSON string does not end the call. The object boundary is found by a
  string- and escape-aware scanner, and `serde_json` then parses exactly that span.
- Several calls in one reply are allowed, with or without text between them.

## Capability matrix

### Schema keywords

| Feature | Status |
|---|---|
| `type`: `string`, `integer`, `number`, `boolean`, `array`, `object` | Supported |
| `type` as a list (for example `["string", "null"]`) | **Unsupported** |
| `type: null` | **Unsupported** |
| `format`: `date-time`, `date`, `email`, `uri` | Supported (structural check) |
| Any other `format` (for example `ipv4`) | **Unsupported** |
| `enum` on string or integer, with at least 2 distinct literals | Supported. Any string value, including spaces, `""`, `"1"` and grammar characters, is written as a JSON string when needed. |
| `enum` on number, or `enum` without a `type` | **Unsupported** |
| Single-literal `enum` | **Unsupported** |
| Enum value or default whose JSON form contains ` # ` | **Unsupported** |
| `default` on a string, integer, number, boolean or enum field | Supported, if the default itself passes the field's schema |
| `default` that is `null`, an array or an object, or that fails the field's schema | **Unsupported** |
| `default` on array items | **Unsupported** |
| `properties`, `required` | Supported |
| `additionalProperties: false` | Supported (closed object) |
| `additionalProperties: true` or a schema | **Unsupported** |
| `items` (for arrays) | Supported |
| `description` on tools and fields | Supported, trimmed as described above |
| `description` on the parameters object or on array items | **Unsupported** |
| `description` with a control character (including `\r`, `\n`) | **Unsupported** |
| Top-level `parameters` that is not an object | **Unsupported** |
| `pattern`, `minimum`, `maximum`, `anyOf`, `oneOf`, `allOf`, `$ref`, and any other keyword | **Unsupported** |
| Field name outside `[A-Za-z0-9_.-]` | **Unsupported** |
| Tool name outside `[A-Za-z0-9_.-]`, or duplicate tool names | **Unsupported** |
| Tools with no parameters | Supported (`NAME()`) |
| Nesting | Supported at any depth the grammar expresses |

### Validation of arguments

| Rule | Behavior |
|---|---|
| `integer` | Accepts integers only. `1.0` and `"1"` are rejected. |
| `number` | Any JSON number. |
| `enum` | Exact match against the literal set. |
| `default` | Not applied by the decoder. An omitted field stays omitted; the client applies its default. |
| Optional field (`?`) | Means absent, never `null`. |
| Required field missing | Rejected. |
| Unknown key in a closed object | Rejected. |
| Unknown key in an open object | Accepted. |
| `format` checks | Structural only: dates have real month and day ranges, times have range-checked fields, emails and URIs have basic shape. |

### Output decoding

| Rule | Behavior |
|---|---|
| Plain text, no marker | Returned as text. No calls. |
| One call | Decoded and validated. |
| Multiple calls | All decoded, in order. |
| `>>`, `}`, or `"` inside JSON strings | Handled correctly. |
| Duplicate keys in a call's JSON | Rejected as `malformed`. |
| Single quotes, trailing commas, or other non-JSON | Rejected as `malformed`. |
| Unterminated call | Rejected as `malformed`. |
| Unknown tool name | Rejected as `unknown_tool`. |
| Arguments that fail the schema | Rejected as `invalid_arguments`. |
| Streamed call longer than `MAX_CALL_BYTES` (1 MiB) | Rejected as `malformed` (`StreamDecoder` only). |
| Streaming input in any chunking | Same calls and text as whole-input decoding. |

## Fail-closed and fallback rules

The crate never returns a partial or guessed result. The router, which owns the policy,
applies these rules at the egress seam:

1. **Schema cannot be stated exactly** (`encode_tools` returns `Unsupported`): send native
   `tools`. The compact block is never approximated.
2. **Compact block would not be smaller** than the native tools: send native `tools`.
3. **Request shape the compact path does not cover** (streaming, forced `tool_choice`,
   conversations that already contain tool calls or results, `parallel_tool_calls`,
   non-function tools): send native `tools`.
4. **Reply contains a call that does not decode or validate** (`malformed`, `unknown_tool`,
   `invalid_arguments`): the original request is re-sent natively. Nothing has reached the
   client yet, so the retry is invisible.
5. **Flag off**: the request is returned byte-for-byte unchanged, and the crate is not called.

On the decoder side, a `StreamDecoder` that has failed stays failed. The caller must treat
the whole response as failed. It never resumes after an error.

## API

```rust
use nasiko_tool_compact::{
    ToolDef, encode_tools, decode_tools, decode_calls, validate_arguments,
    StreamDecoder, StreamEvent, render_call, canonical_schema,
};

// Prompt side
let block = encode_tools(&tools)?;          // CompactTools { text }
// block.text goes in the prompt; Err(Error::Unsupported) means send native tools.

// Reply side, whole text
let calls = decode_calls(&reply, &tools)?;  // Vec<ToolCall>, or Err: never a partial result

// Reply side, streamed
let mut d = StreamDecoder::new(&tools);
for chunk in chunks {
    for ev in d.push(chunk)? {
        match ev {
            StreamEvent::Text(t) => { /* show t */ }
            StreamEvent::Call(c) => { /* c is validated */ }
        }
    }
}
d.finish()?;
```

Errors carry a stable label through `Error::as_label()`: `unsupported`, `unknown_tool`,
`invalid_arguments`, or `malformed`.

The crate has `#![forbid(unsafe_code)]` and denies `unwrap`, `expect`, `panic`, and direct
string slicing under clippy.

Dependencies: `serde`, `serde_json`, `thiserror`.

## Testing and results

### Automated tests

| Suite | Passed | Ignored |
|---|---|---|
| `nasiko-tool-compact` (crate unit tests) | 23 | 0 |
| `nasiko-llm-router` (lib unit tests) | 383 | 1 |
| `nasiko-llm-router` `tests/router_e2e.rs` | 5 | 0 |
| `nasiko-llm-router` `tests/provider_translation.rs` | 0 | 6 (live, need API keys) |
| **Total passed (crate + router + e2e)** | **411** | |

Run with:

```sh
cargo test -p nasiko-tool-compact
cargo test -p nasiko-llm-router
```

Key crate tests:

- `renders_a2_exactly`: the rendered block matches the grammar byte for byte.
- `descriptions_keep_only_what_the_line_does_not_say`: description trimming, exactly.
- `defaults_and_quoted_literals_round_trip`: defaults and quoted enum values survive exactly.
- `fuzz_block_mutations_never_panic`: single-character mutations of a compact block never
  panic the parser.
- `i3_round_trip_is_canonically_equal`: encode then decode gives equivalent schemas.
- `i2_unrenderable_schemas_are_rejected_not_approximated`: unsupported schemas fail closed.
- `i7_same_tools_same_bytes`: determinism, independent of key order.
- `i4_invalid_or_malformed_is_never_a_call`: rejection cases.
- `fuzz_mutations_never_yield_bad_calls`: single-character mutations of a valid call never
  yield an invalid call.
- `i6_any_chunking_equals_whole_decode`: every split point and 300 random chunkings give the
  same result as whole-input decoding.

### Token reduction

Measured with `llm-router/examples/compact_tools_eval.rs` (o200k_base, whole JSON request body,
offline and deterministic):

| Tool set | Safe version | This version |
|---|---|---|
| Eval set (3 cases, 1–2 tools each) | 30.5% | **46.4%** |
| 47 real tools from `agents/*/src/tools.rs` | 12.0% (5 of 9 sets fell back to native) | **32.2%** (all 9 compacted) |

On the eval set, the decoded calls and the decoder results are identical to the safe version.
The 46.4% depends on treating "user" as a filler word; without that rule it is 43.1%.

The eval needs an `EVAL_SET` file of cases, which is not checked in.

```sh
EVAL_SET=path/to/cases.json OUT=out.jsonl \
  cargo run --release -p nasiko-llm-router --example compact_tools_eval
```

### Live format adherence

Setting `PROVIDER_BASE_URL` and `MODEL` (with optional `PROVIDER_API_KEY`) makes the harness
send each compact request at temperature 0, and record the raw output and the decoded calls
in the output.

**This version was run live on `anthropic/claude-sonnet-5.5` (via OpenRouter, temperature 0,
`max_tokens` 1024).** All 3 cases followed the format:

| Case | Model output | Result |
|---|---|---|
| ct-001 | One `<<call create_calendar_event {…}>>` | Decoded and valid |
| ct-002 | `<<call send_email {…}>>` then `<<call create_calendar_event {…}>>` | Both decoded and valid, in order |
| ct-003 ("What's the weather?") | Plain text, no call | No calls |

Free-text fields (title, subject, body) differ in wording from the expected answer, as allowed by
the eval's `free_text_fields`. In ct-002 the model dated "tomorrow" 2026-10-03, which is correct
for the eval's reference date (Friday 2026-10-02); the eval's expected `2026-10-04` looks wrong.
This is one model family only.

The behaviors below were reported from the author's earlier live runs of the safe version (the
raw transcripts are not checked in):

- Multiple calls in one reply.
- Plain-text replies with no call.
- `>>` and `}` inside JSON string arguments.
- Invalid schemas and invalid arguments are rejected, not executed.

Live runs need provider keys and are not part of the automated suite.

## Enabling

The router integration is off by default:

```sh
TOKEN_COMPACT_TOOLS=1   # opt in; unset or 0 sends native tools
```

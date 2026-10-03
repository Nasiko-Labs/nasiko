# nasiko-tool-compact

Compact tool schemas for LLM function calling. It renders OpenAI-style function tools into a
short prompt block, and decodes the model's replies back into standard tool calls. It is
pure Rust: no I/O, no environment reads, no tokenizer, no provider code.

The crate is built on two invariants:

* **Fail-closed.** A schema feature the compact form cannot express exactly is never
  approximated. The caller sends native tools instead. A call that does not validate against
  its original schema is an error, never a guessed call.
* **Deterministic.** The same tools always render to the same bytes. No clock, no RNG.

## Contents

- [A2 grammar](#a2-grammar)
- [Call grammar](#call-grammar)
- [Capability matrix](#capability-matrix)
- [Fail-closed and fallback rules](#fail-closed-and-fallback-rules)
- [API](#api)
- [Testing and results](#testing-and-results)

## A2 grammar

`encode_tools` renders a tool list as one block. The block is line-oriented:

```text
Tools (? = optional):
create_event # Create a calendar event
 start:datetime
 title:str # Event title
 attendees?:[email]
 options?:{}!
  visibility:public|private
get_time()
Call tools ONLY as <<call NAME {JSON args}>> (one per call, never XML/tags); else answer normally.
```

| Element | Meaning |
|---|---|
| `Tools (? = optional):` | Header. Always the first line. |
| `NAME` | Tool name, `[A-Za-z0-9_.-]+`. |
| `()` after the name | Tool takes no parameters. |
| `!` after the name | Top-level object is closed (`additionalProperties: false`). |
| ` # DESCRIPTION` | Optional description, kept verbatim. |
| Field line | One space of indent per nesting level, then `NAME`, optional `?`, `:TYPE`. |
| Field order | Required fields first, then optional, each alphabetical. |
| `str int num bool datetime date email uri` | Scalar types. `datetime`, `date`, `email`, `uri` are string formats. |
| `[TYPE]` | Array. |
| `{}` / `{}!` | Object (its fields follow one level deeper). `{}!` is closed. |
| `a\|b\|c` | String enum. |
| `1\|2\|3` | Integer enum. |
| Last line | The call instruction. Always the last line. |

Rendering is checked at encode time: `encode_tools` decodes its own output and fails if it
does not round-trip to the same schemas.

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
| `enum` on string, integer, or number, with at least 2 distinct literals | Supported |
| Single-literal `enum` | **Unsupported** |
| `enum` literals that contain whitespace, control characters, or grammar syntax (`\| # : ? ! [ ] { } ( )`) | **Unsupported** |
| String enum literal that parses as a JSON number | **Unsupported** |
| `properties`, `required` | Supported |
| `additionalProperties: false` | Supported (closed object) |
| `additionalProperties: true` or a schema | **Unsupported** |
| `items` (for arrays) | Supported |
| `description` on fields, tools, and enum-free scalars | Supported |
| `description` on the parameters object or on array items | **Unsupported** |
| `description` with a control character (including `\r`, `\n`) | **Unsupported** |
| Top-level `parameters` that is not an object | **Unsupported** |
| `pattern`, `minimum`, `maximum`, `default`, `anyOf`, `oneOf`, `allOf`, `$ref`, and any other keyword | **Unsupported** |
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
| `nasiko-tool-compact` (crate unit tests) | 20 | 0 |
| `nasiko-llm-router` (lib unit tests) | 383 | 1 |
| `nasiko-llm-router` `tests/router_e2e.rs` | 5 | 0 |
| `nasiko-llm-router` `tests/provider_translation.rs` | 0 | 6 (live, need API keys) |
| **Total passed (crate + router + e2e)** | **408** | |

Run with:

```sh
cargo test -p nasiko-tool-compact
cargo test -p nasiko-llm-router
```

Key crate tests:

- `renders_a2_exactly`: the rendered block matches the grammar byte for byte.
- `i3_round_trip_is_canonically_equal`: encode then decode gives equivalent schemas.
- `i2_unrenderable_schemas_are_rejected_not_approximated`: unsupported schemas fail closed.
- `i7_same_tools_same_bytes`: determinism, independent of key order.
- `i4_invalid_or_malformed_is_never_a_call`: rejection cases.
- `fuzz_mutations_never_yield_bad_calls`: single-character mutations of a valid call never
  yield an invalid call.
- `i6_any_chunking_equals_whole_decode`: every split point and 300 random chunkings give the
  same result as whole-input decoding.

### Token reduction

Commit `15c214af` reports a **30.5% reduction** in prompt tokens for the compact block against
the native tool schema (o200k_base), with the call instruction shortened from about 18 tokens
to 4 per case. The eval harness is `llm-router/examples/compact_tools_eval.rs`. It is
offline and deterministic, and it needs an `EVAL_SET` file of cases, which is not checked in.

```sh
EVAL_SET=path/to/cases.json OUT=out.jsonl \
  cargo run --release -p nasiko-llm-router --example compact_tools_eval
```

### Live format adherence

Setting `PROVIDER_BASE_URL` and `MODEL` (with optional `PROVIDER_API_KEY`) makes the harness
send each compact request at temperature 0, and record the raw output and the decoded calls
in the output. The behaviors below were reported from the author's live runs (the raw
transcripts are not checked in):

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

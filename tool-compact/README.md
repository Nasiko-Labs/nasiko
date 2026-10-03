# nasiko-tool-compact (CompTrust)

*Compact tools. Deterministic trust.*

Tool definitions cost prompt tokens on every request. This crate rewrites tool schemas into a
compact one-line-per-tool form **when that can be done exactly**, keeps the original schema
**when it cannot**, and turns the model's compact calls back into standard tool calls through a
decoder that **rejects anything invalid instead of guessing**.

```text
tools ─encode_tools→ compact signatures + call format ─→ LLM
LLM text ─StreamDecoder / decode_calls→ validated ToolCall(s)   or   an explicit Error
```

Pure library: no IO, no env, no provider code, no dependency on `nasiko-llm-router` (the router
converts its `ToolDef`/`ToolCall` at the seam and assigns call ids).

## Format

```text
create_calendar_event(title:str "Event title",start:datetime "Start time, ISO 8601",attendees?:[str] "Attendee emails",duration_min?:int "Duration in minutes",visibility?:public|private) - Create an event in the user's calendar.
Call a tool: <<call name {json args}>> (several allowed). ?=optional, datetime=RFC 3339 with offset. Only listed tools and args; if none fits, reply in plain text.
```

```text
<<call create_calendar_event {"title":"Design review","start":"2026-10-05T15:00:00+05:30"}>>
```

The full grammar is in the crate docs (`src/lib.rs`). Arguments stay JSON. Required fields come
first in the schema's `required` order, optional ones sorted. Property descriptions are kept
verbatim as JSON string literals so no delimiter in prose can break a line.

## Supported schema subset, and bypass

Compacted: `string` (+ `format: date-time`), `integer`, `number`, `boolean`, string `enum` (≥2
values from `[A-Za-z0-9_.+/@#-]`), `array` with one `items` schema, nested `object`
(`properties`/`required`), `description`; `additionalProperties: false` and `required: []` are
accepted.

**Anything else bypasses compaction for that tool**: it is returned unchanged in
`CompactTools::native` (send it as an ordinary tool) with the reason in `bypassed`. Examples:
`minLength`, `pattern`, `default`, `$ref`, `oneOf`, `title`, `$schema`, other `format`s,
`type: [..]`, free-form objects/arrays, names or descriptions that don't fit on one line.
Nothing is dropped, retyped or loosened to force a fit. A compact call to a bypassed tool is
rejected (`unsupported_schema`).

## Fail-closed decoding

Each call is validated against the original schema. Rejected, never repaired: unknown tool
(`unknown_tool`), missing/unknown argument, wrong type (`30.0` is not an integer), enum miss
(exact match), invalid `date-time`, malformed or duplicate-key JSON (`invalid_arguments`),
malformed/unterminated marker (`malformed_call`). One bad call fails the whole text. Text
outside markers is ignored; `<<call` not followed by whitespace is prose. Stricter than JSON
Schema's default in one respect: undeclared arguments are rejected.

## Streaming

`StreamDecoder::push(chunk)` returns calls as each marker closes; the result is identical for any
chunking (`decode_calls` *is* one chunk through the same decoder). The JSON object's end is found
by brace matching that skips strings and escapes, so `>>` inside an argument is safe. After an
error the decoder stays failed; `finish()` errors if a marker never closed; a marker still open
after 1 MiB is rejected.

## Tests and evaluation

```sh
cargo test -p nasiko-tool-compact
curl -fsSL https://registry.nasiko.dev/r/nasiko/compact-tools-eval -o /tmp/compact-tools-eval.json
EVAL_SET=/tmp/compact-tools-eval.json OUT=/tmp/out.jsonl \
  cargo run --release -p nasiko-llm-router --example compact_tools_eval
```

Offline and deterministic by default. Optional live mode: set `PROVIDER_BASE_URL` and `MODEL`
(and `PROVIDER_API_KEY` if needed). The example prints a local `o200k_base` token estimate to
stderr.

## Measured (public sample set, 3 cases)

Local estimate, `o200k_base` over the full request JSON (minified), native vs compact:
**25.5%** end to end (728 → 542; an independent Python `tiktoken` run gives 731 → 542, 25.9%).

| component (summed over the 3 cases) | tokens |
|---|---|
| native tool schemas only | 572 |
| compact signatures only | 250 (**56.3%** schema-only reduction) |
| fixed call-format instruction | 45 per request, 135 total |
| compact signatures + instruction | 385 (32.7% vs native schemas) |

The instruction is paid once per request, so a one-tool request saves much less than a
many-tool one; savings grow with the number of tools. All descriptions are kept verbatim. The
call-format instruction carries only what the validator depends on (call syntax, multiple calls,
`?`, the datetime offset, "only listed tools and args", plain-text fallback); the array/enum
notation is left to speak for itself.

Measured and **rejected**: eliding descriptions that add no word beyond the field/tool name
would reach about 32%, but it makes the encoding lossy by heuristic (`decode_tools` could no
longer reproduce the schemas) and cannot be validated without live model data. The figure also
depends on how the baseline is built and serialised (e.g. whether the system message is counted,
spaces in JSON). Live format adherence against real models has **not** been measured.

## Router integration (opt-in)

`llm-router/src/compact_tools.rs` wires this crate into `chat_core`, covering the OpenAI,
Anthropic and Gemini chat surfaces (the Responses API path is untouched). It is **off by default**
and switched on fleet-wide with `TOKEN_COMPACT_TOOLS=true` (`GatewayConfig::compact_tools_enabled`);
with it off the request is not modified at all.

When on: compactable tools leave `tools` and are described, with the call format, in one trailing
`system` message; tools without an exact compact form stay in `tools` untouched. The reply is
decoded against the original schemas into ordinary `tool_calls` (router-minted `call_…` ids,
markers removed from the text, `finish_reason: tool_calls`). Streaming uses `StreamDecoder`: text
is forwarded as it arrives and each call is emitted whole once its marker closes. An invalid call
is a 502 (non-streaming) or ends the stream with no tool call (streaming); nothing is repaired.

The request is left alone, even when on, if it has a `tool_choice` other than absent/`auto`, any
earlier tool call or tool result in the transcript, duplicate tool names, or tools that are not
plain functions / carry extra provider fields.

## Known limits

Not covered: the Responses API surface, history that already contains tool calls/results
(compaction is skipped for those turns, so agent loops only save tokens on the first turn of each
loop), and forced `tool_choice` (skipped). Compaction is fleet-wide, not per agent.
`decode_tools` returns compacted tools first, then native ones, and an empty-parameter tool comes
back with `parameters: None`. `FunctionDef` in the router IR already drops unknown `function.*`
fields (e.g. `strict`) at parse time, with or without compaction.

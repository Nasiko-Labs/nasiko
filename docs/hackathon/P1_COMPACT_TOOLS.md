# P1 — Compact tool schemas

Hackathon track for Nasiko’s public repository. One-day slice, scoped to a new
`tool-compact/` crate and `llm-router/`. Existing router behaviour stays the default.
The build order is `P1_COMPACT_TOOLS_PLAN.md`: twenty tasks, each with a test.

Submission is a fork pull request. Title prefix: `[compact-tools]`. No `submissions/`
folder. The organizers run:

```sh
curl -fsSL https://registry.nasiko.dev/r/nasiko/compact-tools-eval -o /tmp/compact-tools-eval.json
EVAL_SET=/tmp/compact-tools-eval.json OUT=/tmp/out.jsonl \
  cargo run --release -p nasiko-llm-router --example compact_tools_eval
```

Exit code 0 means the eval ran. They score `OUT`. Numbers printed in the PR are claims only.

## Problem

Every tool the client sends is a full JSON Schema: name, description, property types,
formats, enums, required fields, nested objects, and arrays. That schema is copied into
the prompt on every turn, including turns that only need one of the tools.

The model is then expected to answer with an OpenAI tool call. The client needs that
shape back: `tool_calls[].function.name` and `arguments` as a JSON string. Shrinking the
schema is useless if the name or the arguments change, or if a bad reply is turned into
a guessed call.

A calendar tool in the brief is the shape of the problem. The native definition is a
nested JSON object. The compact form is one signature line plus a call instruction:

```text
create_calendar_event(title:str, start:datetime, duration_min?:int, attendees?:[str], visibility?:public|private) - Create an event in the user's calendar.
To call a tool, emit: <<call name {json args}>>
```

The model writes `<<call create_calendar_event {...}>>`. The router decodes that text,
checks it against the original schema, and returns a normal tool call. The client never
sees the compact format.

`compress/` and `llm-router/src/brevity.rs` already shrink payloads and answers. They do
not rewrite tool definitions or parse a call grammar. This track is that missing piece.

## Benefits

- **Fewer input tokens.** The organizers measure `1 - compact_tokens / baseline_tokens`
  on the full request body with the `o200k_base` tokenizer. Bypassed cases count as 0%.
  The target is at least 30%.
- **The client contract stays put.** Callers still send and receive OpenAI-shaped tools
  and tool calls. Compaction is an internal transform.
- **Fail closed.** An unknown tool, a missing required field, a wrong type, or an enum
  value that is not in the schema is an error. The decoder does not invent a call or
  drop a field to make one succeed.
- **Streaming-safe.** A call marker may arrive split across chunks (`<<ca` then
  `ll create_...`). `>>` inside a string argument must not end the call.
- **Deterministic and offline.** The library does no I/O and reads no environment
  variables. The default eval mode makes no network call, so the same input always
  writes the same `OUT`.

## Who benefits

| Who | What they get |
| --- | --- |
| Agent authors | Large tool lists (calendar, email, MCP) stop dominating the prompt. The agent code that sends `tools` does not change. |
| People paying for router traffic | Input tokens drop on every request that carries tool schemas. Output stays a normal tool call, so downstream parsing is unchanged. |
| End users of those agents | The same tool runs with the same arguments. They do not see a new call format. |
| Nasiko router maintainers | One opt-in seam, same pattern as payload compression: off means today’s bytes. |

Gateway protocol changes, direct-provider interception, and orchestrator tool-subset
selection are out of scope. Only requests that already pass through `llm-router`.

## High-level solution

A pure library encodes schemas and decodes calls. The eval example drives the library
directly. The same library is wired into the live router behind an opt-in flag that
defaults off. With the flag off, outbound bytes match today.

```text
Client tools (JSON Schema)
        |
        v
encode_tools  --->  compact signatures + call-format instructions
        |
        v
Model text    --->  <<call name {json args}>>   (or plain text, or several calls)
        |
        v
decode_calls / StreamDecoder
        |
        v
Validate against the original schema
        |
        +-- ok --> OpenAI tool call (router assigns id)
        +-- bad -> error (unknown_tool | invalid_arguments), never a guessed call
```

### Grammar

Explicit, tested, and stable across the public and private sets. Illustrative form from
the brief, not a mandated syntax:

```text
signature  := name "(" params ")" " - " description
param      := name ":" type          # required
            | name "?:" type         # optional
type       := str | int | num | bool | datetime | enum alternatives | object | array
call       := "<<call " name " " json-object ">>"
```

Rules the decoder must implement:

- Multiple calls in one reply, and prose before or after a call.
- A reply with no call is a normal answer, not an error.
- `>>` inside a JSON string does not close the marker. The grammar documents the escape.
- A marker split across stream chunks still decodes once the closing marker arrives.
- Required vs optional, types, enums, nested objects, and arrays survive. Descriptions
  may be shortened only when the short form still disambiguates the field.
- Schema features the grammar cannot represent are listed in the PR. Those tools bypass
  compaction (`compacted: false`) instead of being silently simplified.

If the grammar differs from `<<call ...>>`, the eval converts each decoder case
automatically: render the expected call in our grammar and split at the same relative
positions as the published chunks. No hand-written rewrite per case. The private cases
are unseen.

### Library boundary

`tool-compact` does not depend on `nasiko-llm-router`. It defines its own `ToolDef` and
`ToolCall`. The router converts at the seam. Router types today (`llm-router/src/ir/chat.rs`):

- `ToolDef` / `FunctionDef` — name, optional description, `parameters` as JSON Schema.
- `ToolCall` / `FunctionCall` — name plus `arguments` as a JSON string. The router assigns `id`.
- `ToolCallDelta` — streaming.

Public API:

```rust
pub fn encode_tools(tools: &[ToolDef]) -> Result<CompactTools>;
pub fn decode_calls(text: &str, tools: &[ToolDef]) -> Result<Vec<ToolCall>>;
pub struct StreamDecoder { /* push chunks; handles a split marker */ }

// Recommended, so schema survival can be checked without reading the prose.
pub fn decode_tools(compact: &CompactTools) -> Result<Vec<ToolDef>>;
```

Invariants, matching `compress/`: no I/O, no env, no clock, no RNG, fail closed,
deterministic. Decoding validates every call against the original schema.

### Eval example

`llm-router/examples/compact_tools_eval.rs` reads `EVAL_SET` and writes one JSONL line
per case to `OUT`. It calls `tool-compact` directly, so the organizers can score the
crate without turning the router flag on.

- Live cases (`ct-*`): emit `compact_request` (the OpenAI-shaped body that would be
  sent), `compacted`, `rendered_calls` (expected calls written in our grammar), and
  `roundtrip_calls` (that text decoded back).
- Decoder cases (`dc-*`): feed `chunks` to `StreamDecoder` one by one and emit `decoded`
  as `{calls: ...}` or `{error: "unknown_tool" | "invalid_arguments"}`.
- Default mode is offline. When `PROVIDER_BASE_URL` and `MODEL` are set, each
  `compact_request` is sent at temperature 0 and the line gains `raw_output` and
  `live_calls`. Keys stay in the environment.
- Live runs fix the clock in the system message: today is `2026-10-02`, timezone
  `Asia/Kolkata`, so “Monday 3pm” resolves the same way for every submission.
- `tiktoken-rs` (`o200k_base`) may measure tokens locally. It is a dev-dependency used
  only by the example, not by `tool-compact` and not by the router library.

### Router wiring (in scope)

The opt-in flag is part of this slice. It is read in `llm-router/src/config.rs`, never
inside `tool-compact`. Default is off. A test locks that the off path is byte-identical
to today.

When the flag is on, a seam in `llm-router/src/compact_tools.rs` converts router
`ToolDef` / `ToolCall` to the crate’s types and back. `handlers/chat.rs` calls it after
`compress::apply` and before `brevity::apply`, on the already-resolved `ChatRequest`.
Routing has already chosen the model by then, so compaction cannot change which model
is pinned. Brevity then sees the bytes that will actually be sent.

On the way out the seam encodes the tools, injects the compact text and the call-format
instructions, and drops the native `tools` array. On the way back it decodes model text
into `ToolCall`s and the router assigns `id`. Covered path for this slice: **OpenAI,
non-streaming**. The PR states that limit.

Bypass compaction, and send the native tools, when it cannot be guaranteed:

- a schema feature the grammar does not represent
- a forced `tool_choice` the compact format cannot express
- any provider or streaming mode this slice does not decode

Streaming through the router, Anthropic and Gemini endpoints, and prior tool-call
history in the conversation stay follow-on work on the same flag. Do not change the
gateway protocol or the default configuration.

## Expected file changes

Library, eval, and the opt-in router flag are all in this slice. The flag defaults off.

| Path | Change |
| --- | --- |
| `tool-compact/Cargo.toml` | New package `nasiko-tool-compact`. Depends on `serde`, `serde_json`, `thiserror` only. No dependency on `nasiko-llm-router`. |
| `tool-compact/src/lib.rs` | Crate root. Public `encode_tools`, `decode_calls`, `StreamDecoder`, and `decode_tools`. |
| `tool-compact/src/types.rs` | Own `ToolDef`, `ToolCall`, `CompactTools`, and the error enum (`unknown_tool`, `invalid_arguments`, unsupported schema). |
| `tool-compact/src/grammar.rs` | The written grammar: signature rendering and the call-marker scanner, including escapes. |
| `tool-compact/src/encode.rs` | JSON Schema to compact signatures plus call-format instructions. Bypass list for unsupported features. |
| `tool-compact/src/decode.rs` | Text to calls. Schema validation. Prose around calls. Multiple calls. |
| `tool-compact/src/stream.rs` | Incremental decoder. A marker split across chunks yields one call, not a partial guess. |
| `tool-compact/src/schema.rs` | Required vs optional, types, enums, nested objects, arrays. Fail closed. |
| `tool-compact` tests | Round trip, escaping, malformed output, split markers, enum violations, missing required fields, unknown tools. Property tests for “decode never returns a call that fails the schema”. |
| `Cargo.toml` | Add `"tool-compact"` to `[workspace] members` and `nasiko-tool-compact` to `[workspace.dependencies]`. |
| `llm-router/Cargo.toml` | Depend on `nasiko-tool-compact` so the router seam can call it. `tiktoken-rs` pinned as a dev-dependency, referenced only from the example. |
| `llm-router/examples/compact_tools_eval.rs` | `EVAL_SET` / `OUT` contract. Offline by default. Optional live mode. |
| `llm-router/src/config.rs` | Opt-in flag, default off. Read here, never in `tool-compact`. |
| `llm-router/src/compact_tools.rs` | Seam: convert router `ToolDef` / `ToolCall` to the crate’s types, apply or skip, record why. Same shape as `compress.rs` and `brevity.rs`. |
| `llm-router/src/handlers/chat.rs` | Call the seam on the `ChatRequest` after `compress::apply` and before `brevity::apply`. Decode on the OpenAI non-streaming response path. |
| `llm-router/src/lib.rs` | `mod compact_tools;` |
| `llm-router` tests | Flag off ⇒ request bytes identical to today. Flag on ⇒ OpenAI non-streaming request is compacted and the response decodes back to a standard tool call. |

Not in this change: `server/`, the gateway protocol, orchestrator tool selection, and
`compress/` itself. Read those before adding a second compression stack; do not fold
tool compaction into payload compression.

## Done when

1. `encode_tools` + `decode_calls` round-trip the public cases, including two calls,
   prose around a call, and a reply with no call.
2. `StreamDecoder` passes the published decoder cases, including a split marker, and
   every error case returns an error rather than a guessed call.
3. `cargo run --release -p nasiko-llm-router --example compact_tools_eval` with
   `EVAL_SET` and `OUT` exits 0, is deterministic across two runs, and finishes offline
   with no API key.
4. With the flag off, a router test shows the outbound request is byte-identical to today.
   With the flag on, OpenAI non-streaming encodes tools on the way out and decodes calls
   on the way back. Unsupported schemas and a forced `tool_choice` bypass compaction.
5. The PR states the grammar, the unsupported schema features, the flag and its default,
   which provider path is wired, how to run the example, and any measured token reduction.
   Live model ids only if live mode was run.

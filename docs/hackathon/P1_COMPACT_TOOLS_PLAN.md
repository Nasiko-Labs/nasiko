# P1 implementation plan

Breaks `P1_COMPACT_TOOLS.md` into small tasks. Do them in order. A task is done only when
its test command passes. Later tasks assume the earlier tests still pass.

Rules for every task: `tool-compact` stays free of I/O, environment reads, and any
dependency on `nasiko-llm-router`. Follow `tool-compact/AGENTS.md`.

How to build each task, and the test cases to write, are one file per task in
`lld/`. The checklist is `TODO.md`. Tasks 1, 19, and 20 are scaffolding and have no test cases.

## How to use this plan

1. Implement one task.
2. Run the test named on that task.
3. Leave the previous tasks' tests green before starting the next one.

Shared checks, once the crate exists:

```sh
cargo test -p nasiko-tool-compact
cargo fmt
cargo clippy -p nasiko-tool-compact -- -D warnings
```

## Task 1 — Crate skeleton

Create `nasiko-tool-compact` and register it in the workspace. Empty public API is enough:
the four entry points can return `UnsupportedSchema` until later tasks fill them in.

**Done when:** `cargo test -p nasiko-tool-compact` compiles and passes, and
`cargo tree -p nasiko-tool-compact -i nasiko-llm-router` reports that the router is not
a dependency. `#![forbid(unsafe_code)]` is on the crate root.

## Task 2 — Types and errors

Add `ToolDef`, `ToolCall`, `CompactTools`, and `CompactError` (`UnknownTool`,
`InvalidArguments`, `UnsupportedSchema`). `ToolCall.arguments` is a JSON string.

**Done when:** a unit test builds a calendar `ToolDef` from the brief's schema, and a
second test matches each error variant without comparing error strings.

## Task 3 — Decide what the grammar can represent

Walk a JSON Schema and accept only: object root, `string` / `integer` / `number` /
`boolean`, `format: date-time`, `enum` of strings, arrays of those scalars, nested
objects of those scalars, and `required`. Anything else (oneOf, additionalProperties
schemas, `$ref`, tuple arrays) is unsupported.

**Done when:** tests accept the brief's `create_calendar_event` schema and reject one
schema that uses `$ref`. The reject path is `UnsupportedSchema`, not a shortened schema.

## Task 4 — Encode one tool

Render one supported tool as a signature plus the call-format instruction.

```text
create_calendar_event(title:str, start:datetime, duration_min?:int, attendees?:[str], visibility?:public|private) - Create an event in the user's calendar.
To call a tool, emit: <<call name {json args}>>
```

Required parameters have no `?`. Optional ones do. Descriptions stay, shortened only when
the short text still separates two fields.

**Done when:** a test encodes the calendar tool and asserts the signature text, including
`title:str`, `duration_min?:int`, and `visibility?:public|private`.

## Task 5 — Schema survives the compact form

`decode_tools` rebuilds required versus optional, types, enums, arrays, nested objects,
and descriptions from `CompactTools`.

**Done when:** encode then `decode_tools` on the calendar tool equals the original schema
facts. A test with two tools keeps both names.

## Task 6 — Decode one well-formed call

Scan `<<call name {json}>>`, parse the JSON object, and return one `ToolCall` whose
`arguments` are the same JSON string.

**Done when:** the brief's design-review call decodes to `create_calendar_event` with
`title`, `start`, and `attendees`. Key order in the JSON object does not matter.

## Task 7 — `>>` inside a string is not the end of the call

The closer is `>>` outside a JSON string. `>>` inside a quoted argument stays in the
argument.

**Done when:** a call whose title is `meet >> review` decodes with that title intact,
and the characters after the real closer are not part of the arguments.

## Task 8 — Fail closed on a bad call

Validate the decoded object against the original schema.

**Done when:** separate tests assert all of these, and each `Err` carries no call:

- tool name not in the provided list → `UnknownTool`
- missing `title` → `InvalidArguments`
- `duration_min` as a string → `InvalidArguments`
- `visibility: "secret"` → `InvalidArguments`
- truncated JSON or a missing closer → `InvalidArguments`

## Task 9 — Prose and more than one call

A reply may contain text before a call, text after a call, several calls, or no call.

**Done when:**

- prose around one call still yields that call
- two markers yield two calls in order
- `What's the weather?` yields `Ok` with an empty list, not an error

## Task 10 — Stream decoder

`StreamDecoder` accepts chunks and emits a call only when the marker is complete.

**Done when:** pushing `<<ca`, `ll create_calendar_event {"title":"Ret`, `ro",...}>`, `>`
yields one call and no earlier partial call. The same bytes pushed as one chunk match
`decode_calls`. An error chunk sequence yields the error and no guessed call.

## Task 11 — Decoder never returns an invalid call

Property or table test: every `Ok(calls)` from `decode_calls` and `StreamDecoder`
validates against the schema that was passed in.

**Done when:** `cargo test -p nasiko-tool-compact schema_valid_calls_only` passes, using
generated or table-driven bad strings (unknown name, missing field, bad enum, broken
marker). None of those return `Ok` with a call.

## Task 12 — Offline eval example

`llm-router/examples/compact_tools_eval.rs` reads `EVAL_SET` and writes one JSONL line
per case to `OUT`. No network in the default mode. Missing env vars exit non-zero with
a message. `tiktoken-rs` is a dev-dependency of the example's package only.

**Done when:** a tiny local JSON fixture with one `ct` case and one `dc` case produces
two lines containing `compact_request`, `compacted`, `rendered_calls`, `roundtrip_calls`,
and `decoded`. `cargo run -p nasiko-llm-router --example compact_tools_eval` exits 0.

## Task 13 — Public sample round trip

Point the example at the published sample.

```sh
curl -fsSL https://registry.nasiko.dev/r/nasiko/compact-tools-eval -o /tmp/compact-tools-eval.json
EVAL_SET=/tmp/compact-tools-eval.json OUT=/tmp/out.jsonl \
  cargo run --release -p nasiko-llm-router --example compact_tools_eval
```

**Done when:** exit code is 0, every sample case has a line, `ct` round trips match
`expected` (free-text fields checked for presence and type only), and `dc` lines match
the expected calls or `unknown_tool` / `invalid_arguments`. Decoder chunks are split by
code at the same relative positions, not hand-edited per case id.

## Task 14 — Eval is deterministic

Run task 13 twice to two files.

**Done when:** `diff` of the two `OUT` files is empty. No API key is set.

## Task 15 — Token reduction is measurable

The example, or a small local helper used only by it, counts `o200k_base` tokens on
`compact_request` versus the native request built from `tools` and `messages`.

**Done when:** a printed local summary shows a percentage, bypassed cases contribute 0,
and the public sample is at or above the 30% target. Record the number for the PR as a
local claim.

## Task 16 — Router flag defaults off

Add the flag in `llm-router/src/config.rs`. Default is off. The library still does not
read the environment.

**Done when:** a router test with the flag unset compares the outbound OpenAI request to
the same request built without the seam, and the bytes match.

## Task 17 — Compact on the OpenAI non-streaming path

When the flag is on, `compact_tools.rs` runs after `compress::apply` and before
`brevity::apply`. It replaces native `tools` with the compact text. The non-streaming
OpenAI response path decodes model text into `tool_calls` and the router assigns `id`.

**Done when:** a router test sends a calendar tool and a canned `<<call ...>>` reply and
the client-facing response is a standard tool call with the right name and arguments.
Streaming, Anthropic, and Gemini requests are unchanged.

## Task 18 — Bypass when compaction is not safe

Skip compaction for an unsupported schema, a forced `tool_choice` the format cannot
express, and any provider or mode task 17 does not decode.

**Done when:** three router tests show the native `tools` array still present on the
outbound request for those three inputs. The skip reason is recorded on the seam result
so the test can see it.

## Task 19 — Workspace test entry

Add `-p nasiko-tool-compact` to `test-unit` in the root `justfile`.

**Done when:** `just test-unit` runs this crate's tests without Postgres, Redis, or a
model key.

## Task 20 — Pull request text

Write the PR from the measured run. This task changes no behavior.

**Done when:** the description has, in order: track, how to run (including the flag and
its default), model ids or "live mode not run", local token result, and known limits
(unsupported schemas, OpenAI non-streaming only). Title starts with `[compact-tools]`.
The diff contains no API key, `.env`, or `OUT` file.

## Leave for later

Do not start these until tasks 1–20 are green:

- streaming decode inside the router
- Anthropic and Gemini
- prior tool calls in conversation history
- live mode against `PROVIDER_BASE_URL` (optional evidence for the PR, not a gate)

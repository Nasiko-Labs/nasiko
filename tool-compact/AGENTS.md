# tool-compact — agent guide

Instructions for anyone changing `nasiko-tool-compact`. Read this before writing code.

Also read:

- `docs/hackathon/P1_COMPACT_TOOLS.md` — problem, solution, and the file list for this slice
- `docs/CLEAN_CODE_GUIDE.md` — repo-wide code standards
- `CONTRIBUTING.md` — setup, tests, and pull-request flow
- `compress/src/lib.rs` — the crate this one should feel like: pure, deterministic, fail-closed, no environment reads

The package name is `nasiko-tool-compact`. The directory is `tool-compact/`. The hackathon track slug is `compact-tools`.

## What this crate is

A pure library that turns OpenAI function-tool JSON Schemas into a short text form, and turns a model's compact reply back into validated tool calls. The router and the eval example call it. It does not call them.

Public surface:

```rust
pub fn encode_tools(tools: &[ToolDef]) -> Result<CompactTools, CompactError>;
pub fn decode_calls(text: &str, tools: &[ToolDef]) -> Result<Vec<ToolCall>, CompactError>;
pub fn decode_tools(compact: &CompactTools) -> Result<Vec<ToolDef>, CompactError>;
pub struct StreamDecoder { /* push chunks; emit calls or a terminal error */ }
```

`ToolDef` and `ToolCall` are defined here. Router types in `llm-router/src/ir/chat.rs` stay on the router side of the seam. The router assigns call `id`.

## Invariants

These are compiler-and-review gates, not suggestions. `compress/` enforces the same kind of list at the crate root.

- **No I/O.** No filesystem, network, clock, or random number generator.
- **No environment reads.** Flags, model names, and `EVAL_SET` / `OUT` belong in `llm-router`, never here.
- **No dependency on `nasiko-llm-router`.** The router depends on this crate. The other direction is a cycle.
- **Fail closed.** An unknown tool, a missing required field, a wrong type, or an enum value outside the schema is `Err`. The decoder never invents a call, drops a field, or coerces a value to make a call succeed.
- **Deterministic.** The same tools and the same text always produce the same `Result`. No sampling.
- **Schema meaning stays.** Required versus optional, types, enums, nested objects, and arrays round-trip through `decode_tools`. A description may be shortened only when the short form still tells two fields apart. A schema feature the grammar cannot represent is an explicit bypass, not a silent simplification.
- **UTF-8 safe.** Slice only on char boundaries. `>>` inside a JSON string does not end a call marker.
- **Streaming is incremental.** `StreamDecoder` holds a split marker (`<<ca` then `ll create_...`) until the call is complete. A partial chunk is not a guessed call.
- **`unsafe` is forbidden.** `#![forbid(unsafe_code)]` at the crate root.

`encode_tools` may return `Err` for a schema it refuses. It must not emit a compact form that `decode_calls` would accept with weaker checks than the original schema.

## Scope

In this slice:

- This crate, its tests, the workspace membership, and the eval example `llm-router/examples/compact_tools_eval.rs`.
- Opt-in router wiring. The flag is read in `llm-router/src/config.rs`, default off. Off means the outbound request is byte-identical to today. On covers OpenAI, non-streaming: compact tools on the way out, decode on the way back. Unsupported schemas and a forced `tool_choice` the format cannot express bypass compaction and send native tools.

Out of this slice:

- Streaming through the router, Anthropic, and Gemini. The stream decoder still exists in this crate; the router does not call it yet.
- Gateway protocol changes, orchestrator tool-subset selection, and edits to `compress/`.
- Putting token counting inside this crate. `tiktoken-rs` (`o200k_base`) is a dev-dependency of `nasiko-llm-router`, used only by the example.

## Coding guidelines

Match `compress/` and `docs/CLEAN_CODE_GUIDE.md`.

- Edition `2024`, version from the workspace. Dependencies are `serde`, `serde_json`, and `thiserror`, each as `dep.workspace = true`. Add a dependency in the root `Cargo.toml` first, and only when this crate needs it.
- One module, one job. Planned layout:

  | Module | Owns |
  | --- | --- |
  | `types.rs` | `ToolDef`, `ToolCall`, `CompactTools`, `CompactError` |
  | `grammar.rs` | Signature rendering and the call-marker scanner, including escapes |
  | `encode.rs` | JSON Schema to signatures plus the call-format instructions |
  | `decode.rs` | Text to calls, including prose around calls and several calls |
  | `stream.rs` | `StreamDecoder` |
  | `schema.rs` | Required versus optional, types, enums, nested objects, arrays |

- `lib.rs` is the public entry. Helpers stay private. Order the file public items first, private helpers below.
- Errors are a `thiserror` enum whose variants the caller can match: `UnknownTool`, `InvalidArguments`, `UnsupportedSchema`. Do not match on error strings. `InvalidArguments` carries the tool name and the reason (missing field, type, enum).
- Library paths return `Result`. `unwrap`, `expect`, and `panic` are denied outside tests (`#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]`, allowed under `cfg(test)` the way `compress/` does it).
- Deny `clippy::string_slice`. A byte index into a `&str` is a bug when the marker is split or the argument contains non-ASCII text.
- Doc comments on every public item state the invariant, not the implementation. Comments in the body explain why a rule exists (for example why `>>` inside a string is not the closer).
- No boolean flag arguments. A bypass is its own function or an explicit `UnsupportedSchema` error the router turns into "send native tools".
- No feature flags. This repository extends by traits and seams, not `#[cfg(feature = ...)]`.
- Grammar lives in one place. The eval example renders expected calls by calling this crate. It does not reimplement the marker format, and it does not special-case decoder fixtures by id.

## Coding best practices

- Names say what the value means: `required_fields`, `call_marker`, `enum_values`. Types are nouns, functions are verbs.
- A function does one thing at one level of abstraction. Parsing a marker and validating a schema are different functions.
- Three or more related parameters become a struct. `decode_calls(text, tools)` is already the right width.
- Pass `&str` and `&[ToolDef]` for reads. Take ownership only when the value is stored on `CompactTools` or `ToolCall`.
- Define the call marker, the type names (`str`, `int`, `datetime`, …), and the escape rule once. The encoder, the decoder, and the stream decoder share them.
- Make illegal states hard to represent. A compact call under construction is an enum (`Outside`, `InsideString`, `SawGreater`), not a pile of booleans.
- Measure before optimizing. Correctness and a stable grammar come first. Token savings are measured on the eval request body, not by micro-optimizing this crate.
- Leave a file cleaner than you found it, and do not refactor unrelated router code while doing it.

## Testing guidelines

Tests are hermetic: no network, no database, no Docker, no API key, no clock. They call the public functions.

```sh
cargo test -p nasiko-tool-compact
```

Once the crate exists, add `-p nasiko-tool-compact` to the `test-unit` recipe in the root `justfile`, next to `nasiko-compress`.

Name a test for the behavior: `missing_required_title_is_invalid_arguments`, `marker_split_across_chunks_decodes_once`, `greater_than_inside_string_does_not_end_call`.

Cover at least:

- Round trip of a tool with required and optional fields, enums, arrays, and one nested object. `decode_tools` agrees with the input schema on those facts.
- Several calls in one reply, prose before and after a call, and a reply with no call (success, empty list).
- Unknown tool name → `UnknownTool`, and the `Ok` value contains no call.
- Missing required field, wrong JSON type, and an enum value not in the schema → `InvalidArguments`, and no call.
- `>>` inside a JSON string.
- `StreamDecoder` fed the published split (`<<ca` | `ll create_...` | rest) yields one call.
- A schema feature on the unsupported list returns `UnsupportedSchema` and does not emit a compact form that drops that feature.
- Property: every `Ok(calls)` from `decode_calls` validates against the original schema. A generated or table-driven malformed string never comes back as `Ok` with a guessed call.

Router tests live in `llm-router`, not here:

```sh
cargo test -p nasiko-llm-router
```

- Flag off: outbound request bytes match the router without this feature.
- Flag on, OpenAI, non-streaming: native `tools` are replaced by the compact text, and the decoded reply is a standard tool call.
- Unsupported schema and a forced `tool_choice` leave native tools in place.

The organizers' check is the example, run twice. Exit 0 and a clean `diff` mean the eval is deterministic. The example writes outputs, not scores.

```sh
curl -fsSL https://registry.nasiko.dev/r/nasiko/compact-tools-eval -o /tmp/compact-tools-eval.json
EVAL_SET=/tmp/compact-tools-eval.json OUT=/tmp/out-a.jsonl \
  cargo run --release -p nasiko-llm-router --example compact_tools_eval
EVAL_SET=/tmp/compact-tools-eval.json OUT=/tmp/out-b.jsonl \
  cargo run --release -p nasiko-llm-router --example compact_tools_eval
diff /tmp/out-a.jsonl /tmp/out-b.jsonl
```

Read `OUT` yourself. A `ct-*` line is good when `roundtrip_calls` matches the sample's `expected` (key order ignored; `subject`, `body`, and `title` only need to be present and the right type). A `dc-*` error line is good when `decoded.error` is `unknown_tool` or `invalid_arguments` and there is no call. `compacted: false` is allowed only for a documented bypass, and those cases count as 0% savings.

Do not vendor the private scoring set, and do not hard-code answers by case id. The private cases use the same schema and are unseen. If the grammar is not `<<call ...>>`, convert each decoder case by rendering the expected call and splitting at the same relative positions. That conversion is code, not a hand edit per case.

Live mode is optional and stays out of unit tests. It runs only when `PROVIDER_BASE_URL` and `MODEL` are set. The system message fixes today as `2026-10-02`, timezone `Asia/Kolkata`.

## Security guidelines

- Never commit API keys, `.env` files, private prompts, or real user content. Fixtures use the public sample's fictional addresses (`riya@example.com`) or smaller synthetic schemas.
- This crate does not see provider credentials. Do not add an HTTP client "for convenience".
- Treat model text as untrusted input. A huge payload, a nested marker, or a string full of `>>` returns `Err` or a bounded parse. It does not panic and it does not loop without a size cap. Pick one documented input ceiling and test it.
- Do not log tool arguments. They can contain emails, titles, and other caller data. Tests assert on values; production code in this crate does not print them.
- Decoding must not widen what the schema allows. Extra fields the schema does not list are `InvalidArguments`, not silently kept, unless the PR documents a different closed policy and tests it.
- The router flag defaults off. A change that turns compaction on for every request is a behavior change and fails the byte-identical test.
- Do not copy schema text from a proprietary source into the repo. The grammar and the implementation are original. `compress/` documents its prior art the same way; follow that if you mention an external idea.

## How to run

Library tests and a workspace check, from the repo root:

```sh
cargo test -p nasiko-tool-compact
cargo fmt
cargo check -p nasiko-tool-compact
cargo clippy -p nasiko-tool-compact -- -D warnings
```

Before a pull request, the workspace gate still applies:

```sh
cargo fmt
cargo check --workspace
cargo clippy --workspace
```

Zero warnings. CI fails on warnings. `cargo fmt` is the only layout. Do not silence a lint without a comment that says why.

Offline eval (no key):

```sh
curl -fsSL https://registry.nasiko.dev/r/nasiko/compact-tools-eval -o /tmp/compact-tools-eval.json
EVAL_SET=/tmp/compact-tools-eval.json OUT=/tmp/out.jsonl \
  cargo run --release -p nasiko-llm-router --example compact_tools_eval
```

`cargo fetch` before a network-restricted run. The offline example does not contact a model.

Live eval, only on a machine that already has a key in the environment:

```sh
EVAL_SET=/tmp/compact-tools-eval.json OUT=/tmp/live.jsonl \
  PROVIDER_BASE_URL="$PROVIDER_BASE_URL" MODEL="$MODEL" \
  cargo run --release -p nasiko-llm-router --example compact_tools_eval
```

Router flag tests: `cargo test -p nasiko-llm-router`. The flag's environment name is defined in `llm-router/src/config.rs` and documented in the PR. This crate does not read it.

## How to raise a PR

One fork pull request to `Nasiko-Labs/nasiko`. No `submissions/` directory. The PR is the submission.

- Branch from a fresh `main`. One logical change: compact tool schemas, the eval example, and the opt-in router flag.
- Title prefix exactly `[compact-tools]`. Use the slug, not `P1`. Example: `[compact-tools] Compact function-tool schemas and decode them fail-closed`.
- Description, in this order:
  1. **Track** — compact tool schemas.
  2. **How to run** — the `EVAL_SET` / `OUT` command, the router flag name, and that the default is off.
  3. **Model IDs** — only if live mode was run. Otherwise say live mode was not run.
  4. **Measured results** — token reduction you computed locally with `o200k_base`, and what the public sample's round trip did. Label them as local claims. The organizers recompute on a private set.
  5. **Known limits** — unsupported schema features, the OpenAI non-streaming limit, and any case that bypasses compaction.
- State the grammar in the description, including the escape for `>>` inside a string. If decoder cases are rewritten into that grammar, say the rewrite is automatic and position-based.
- Confirm the flag-off path is covered by a test.
- Do not paste API keys, private prompts, or the private scoring set.
- `cargo fmt`, `cargo check --workspace`, and `cargo clippy --workspace` are clean before opening the PR. Unit tests for this crate and the router flag tests pass.

You do not merge. Reviewers and the organizers' harness decide.

## How to write git commit messages

Write a short, natural-language subject that tells a reviewer what changed. Conventional
prefixes such as `feat:`, `fix:`, and `chore:` are not required.

```text
Validate compact tool calls against their original schemas

The router can shrink tool definitions only if a bad model reply cannot
become a guessed call. Unknown tools and invalid arguments now return
typed errors, and the stream decoder waits for a marker split across chunks.
```

- Write the subject as a clear action or outcome: `Add streaming-safe compact call decoding`,
  `Keep native tools for unsupported schemas`, or `Document the compact-tools evaluation`.
- Avoid robotic labels, ticket-only subjects, and vague messages such as `updates`, `changes`,
  `WIP`, or `fix stuff`.
- Keep the subject concise, preferably about 50–72 characters, with no trailing period.
- Add a body when the reason is not obvious. Explain why the change exists, the trade-off, or
  the invariant it protects. Wrap near 72 characters.
- One concern per commit when you can: grammar and decoder, then the eval example, then the router flag. A single commit is fine when the slice is still small.
- Do not commit secrets, `OUT` files, or generated eval output.
- Do not use `git commit --amend` after a hook failure or after the commit is pushed. Add a new commit.
- Do not use `--no-verify`. If fmt or clippy fails, fix the code and commit again.
- Describe docs and test commits naturally too: `Explain how to run the offline evaluation` or
  `Cover split call markers in the stream decoder`.

## Review checklist

- [ ] Crate has no I/O, no env reads, no router dependency, no `unsafe`.
- [ ] Unknown tool, missing required field, bad type, and bad enum are errors with no call attached.
- [ ] `decode_tools` preserves required versus optional, types, enums, nested objects, and arrays.
- [ ] Unsupported schemas are an explicit bypass.
- [ ] Stream decoder handles a marker split across chunks and `>>` inside a string.
- [ ] Eval example writes the JSONL contract, is deterministic, and does not hard-code case ids.
- [ ] Router flag defaults off and the off path is byte-identical.
- [ ] OpenAI non-streaming on path is tested; other providers are documented as not wired.
- [ ] `cargo fmt` clean; `cargo clippy --workspace` has zero warnings.
- [ ] No keys, private prompts, or real user data in the diff.

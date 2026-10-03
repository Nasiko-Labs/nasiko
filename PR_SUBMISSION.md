# PR: Compact tool schemas (`nasiko-tool-compact`), opt-in behind `TOKEN_COMPACT_TOOLS`

## Summary

This PR adds `nasiko-tool-compact`, a pure crate that renders function-tool schemas into a
compact prompt block and decodes the model's replies back into standard tool calls. The router
integration is opt-in and off by default, so existing behavior is unchanged unless
`TOKEN_COMPACT_TOOLS` is set.

On the eval set, the compact block uses **30.5% fewer tokens** than the native tool schema,
with the call instruction shortened from about 18 tokens to 4 per case.

## The "Safe" profile

The design favors correctness over coverage. Four properties define it:

1. **Deterministic per-tool compaction.** The same tools always render to the same bytes. The
   output does not depend on key order, `required` order, or any clock or random source. A
   self-check decodes the rendered block and fails if it does not round-trip to the same
   schemas.
2. **Standard JSON argument parsing.** Call arguments are parsed with `serde_json`, using
   strict JSON that rejects duplicate keys. The crate does not use regex or ad hoc parsing for
   arguments. The call boundary is found by a string- and escape-aware scanner, so `>>` or `}`
   inside a JSON string never ends a call.
3. **Strict fail-closed streaming decoder.** `StreamDecoder` holds back at most a marker prefix
   or an unfinished call. Any chunking of the input gives the same calls and text as decoding
   the whole input. After an error the decoder stays failed, and the caller treats the whole
   response as failed.
4. **Fail-closed on schemas.** A schema feature the compact form cannot state exactly is
   rejected, never approximated. The caller then sends native tools.

This is the design posture of the profile. It is not a runtime switch: there is no
configuration flag that selects a "Safe" mode.

## What changed

- **`tool-compact/`** (new crate, `nasiko-tool-compact`):
  - `compact.rs`: the A2 compact form, `encode_tools` and `decode_tools`, the call instruction,
    and the encode-time round-trip self-check.
  - `schema.rs`: the supported JSON Schema subset as a typed tree. Unsupported keywords are
    rejected, never dropped.
  - `validate.rs`: argument validation against the original schema.
  - `calls.rs`: the call grammar and `decode_calls`.
  - `stream.rs`: `StreamDecoder` with bounded hold-back.
  - `README.md`: grammar, capability matrix, fallback rules, and test results.
- **`llm-router/src/compact_tools.rs`** (new): the egress seam. It decides whether a request is
  eligible, builds the compacted request, and decodes the reply. It re-sends the request
  natively if the reply contains an invalid call.
- **`llm-router/src/handlers/chat.rs`**: calls the seam on the chat path, with the native retry
  on an invalid reply.
- **`llm-router/src/config.rs`**: `TOKEN_COMPACT_TOOLS` flag, off by default.
- **`llm-router/src/lib.rs`, `llm-router/Cargo.toml`, root `Cargo.toml`**: module and
  workspace wiring for the new crate.
- **`llm-router/examples/compact_tools_eval.rs`**: offline, deterministic eval harness, with an
  optional live mode for format-adherence checks.
- **`llm-router/tests/router_e2e.rs`**: formatting only in this PR.

## Behavior when the flag is off

With `TOKEN_COMPACT_TOOLS` unset, the router returns before looking at the request. The request
is sent byte-for-byte as received, and the crate is not called.

## Scope

Compaction applies to non-streaming chat requests with function tools and `tool_choice` left at
`auto`. Everything else goes out natively: streaming, forced `tool_choice`, conversations that
already contain tool calls or results, `parallel_tool_calls`, non-function tools, schemas the
crate cannot state exactly, and blocks that would not be smaller.

## Testing

| Suite | Passed | Ignored |
|---|---|---|
| `nasiko-tool-compact` | 20 | 0 |
| `nasiko-llm-router` (lib) | 383 | 1 |
| `nasiko-llm-router` `router_e2e` | 5 | 0 |
| **Total** | **408** | |

The 6 ignored tests in `provider_translation.rs` are live tests that need provider API keys.

Lint: `cargo fmt --all -- --check` is clean. `cargo clippy --workspace --all-targets -- -D warnings`
passes.

Format adherence (live, reported by the author, not part of the automated suite): the model
emitted multiple calls in one reply, plain-text replies with no call, and `>>` inside JSON
string arguments. Invalid schemas and invalid arguments were rejected and not executed.

## Review notes

- The 30.5% figure comes from commit `15c214af`. The eval input set is not checked in, so the
  number is not reproduced by this PR. Reviewers can run the eval with their own `EVAL_SET`.
- The live adherence results need provider keys and are not reproducible from CI.
- `Cargo.lock` has a pre-existing local modification that is not part of this change.

🤖 Generated with [Claude Code](https://claude.com/claude-code)

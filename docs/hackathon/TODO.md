# Compact tools — task list

Status is `Not started`, `In progress`, or `Done`. Update this file when a task moves.

| Task | Description | Low-level design | Status |
| --- | --- | --- | --- |
| T01 | Create the `nasiko-tool-compact` crate and register it in the workspace | [lld/01-crate-skeleton.md](lld/01-crate-skeleton.md) | Done |
| T02 | Add tool, call, and error types | [lld/02-types-and-errors.md](lld/02-types-and-errors.md) | Done |
| T03 | Accept only the schema features the grammar can represent | [lld/03-supported-schema.md](lld/03-supported-schema.md) | Done |
| T04 | Encode one tool as a compact signature | [lld/04-encode-one-tool.md](lld/04-encode-one-tool.md) | Done |
| T05 | Rebuild the schema from the compact form | [lld/05-schema-round-trip.md](lld/05-schema-round-trip.md) | Done |
| T06 | Decode one well-formed call | [lld/06-decode-one-call.md](lld/06-decode-one-call.md) | Done |
| T07 | Keep `>>` inside a string argument | [lld/07-closer-inside-string.md](lld/07-closer-inside-string.md) | Done |
| T08 | Reject unknown tools and invalid arguments | [lld/08-fail-closed.md](lld/08-fail-closed.md) | Done |
| T09 | Decode prose, several calls, and a reply with no call | [lld/09-prose-and-several-calls.md](lld/09-prose-and-several-calls.md) | Done |
| T10 | Decode a call split across stream chunks | [lld/10-stream-decoder.md](lld/10-stream-decoder.md) | Done |
| T11 | Prove a successful decode always matches the schema | [lld/11-no-invalid-success.md](lld/11-no-invalid-success.md) | Done |
| T12 | Write the offline eval example | [lld/12-offline-eval.md](lld/12-offline-eval.md) | Done |
| T13 | Round-trip the public sample | [lld/13-public-sample.md](lld/13-public-sample.md) | Done |
| T14 | Show two eval runs write the same output | [lld/14-deterministic-eval.md](lld/14-deterministic-eval.md) | Done |
| T15 | Measure token reduction on the public sample | [lld/15-token-count.md](lld/15-token-count.md) | Done |
| T16 | Add the router flag and keep it off by default | [lld/16-flag-defaults-off.md](lld/16-flag-defaults-off.md) | Not started |
| T17 | Compact and decode OpenAI non-streaming requests | [lld/17-openai-non-streaming.md](lld/17-openai-non-streaming.md) | Not started |
| T18 | Bypass compaction when it is not safe | [lld/18-bypass.md](lld/18-bypass.md) | Not started |
| T19 | Run the crate from `just test-unit` | [lld/19-workspace-test-entry.md](lld/19-workspace-test-entry.md) | Not started |
| T20 | Write the pull request | [lld/20-pull-request-text.md](lld/20-pull-request-text.md) | Not started |

Shared fixtures and the grammar: [lld/00-shared.md](lld/00-shared.md).

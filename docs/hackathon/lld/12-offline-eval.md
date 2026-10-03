# Task 12 — Offline eval example

Tracked in [`../TODO.md`](../TODO.md).

Fixtures and the grammar live in [`00-shared.md`](00-shared.md).

`llm-router/examples/compact_tools_eval.rs` is a `fn main`. Read `EVAL_SET` and `OUT`
from the environment. Missing either prints to stderr and exits `1`. Read the JSON file.
For each case in `cases`, build the compact request and write one line. For each case in
`decoder_cases`, render the expected call with `encode` helpers, split the rendered text
at the same relative indexes as `chunks`, push those pieces into `StreamDecoder`, and
write `decoded`. Do not branch on case id.

`compact_request` is an OpenAI chat body: a system message with the compact text, the
user messages from the case, and no native `tools` field when compaction ran. When
`encode_tools` returns `UnsupportedSchema`, write `compacted: false` and put the native
tools on the body.

`llm-router/Cargo.toml`: `nasiko-tool-compact` as a normal dependency later for the
seam; for this task a dependency is already required so the example can call the crate.
`tiktoken-rs` is not added yet.

No unit-test crate. The test is a fixture run.

### Test case

Fixture `/tmp/compact-tools-fixture.json`: one `ct` case whose expected call is the
design-review call, and one `dc` case whose chunks are `<<ca`, `ll create_...`, `>`.
Run:

```sh
EVAL_SET=/tmp/compact-tools-fixture.json OUT=/tmp/fixture-out.jsonl \
  cargo run -p nasiko-llm-router --example compact_tools_eval
```

Assert exit 0 and exactly two lines. Line `ct` has `compacted: true`, a `compact_request`
without a `tools` key, `rendered_calls` containing `<<call create_calendar_event`, and
`roundtrip_calls[0].name == "create_calendar_event"`. Line `dc` has `decoded.calls`
length 1. Unset `EVAL_SET` and assert a non-zero exit.

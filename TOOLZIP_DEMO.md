# ToolZip: two-minute demo

Run the synthetic demo (no keys or network):

```sh
cargo run -p nasiko-llm-router --example toolzip_demo
```

**0:00–0:20 — Cost.** Show the native body count: 48 synthetic tools, 5345 tokens.
Say: “ToolScope reduces what the model sees. ToolZip reduces how much it costs to
describe it. SchemaGuard ensures optimization never bypasses correctness.”

**0:20–0:40 — SCOPE.** The explicit `mail_send and calendar_create` query retains
those two tools. No-signal and broad `record` queries retain all 48. Explain that
lexical selection is optional and disabled in official P1 evaluation.

**0:40–1:00 — ZIP.** Show `compact_grammar`. Required fields, optional `?`, enums,
nested arrays/objects, closed `!` objects and useful descriptions remain visible.
ZIP alone measures 2365 tokens, a 55.75% reduction on this synthetic fixture.
SCOPE+ZIP measures 157 tokens, a 97.06% reduction; label these as custom results.

**1:00–1:20 — Valid call.** Show `rendered_call` with `Build >> deployed` inside the
JSON string, then `standard_tool_calls`. Arguments remain a JSON string in the
OpenAI-shaped reconstruction. Demo IDs are assigned at the seam, not by the library.

**1:20–1:40 — GUARD.** `unknown_tool_rejected`, `invalid_enum_rejected` and
`unsupported_schema_native_fallback` are true. Show stream emissions `[0,0,0,1]`:
the split marker produces a call only after completion. Calls remain provisional
until successful `finish()`.

**1:40–2:00 — Evidence and limits.** `schema_roundtrip_equal` is true because
decode_tools parses the actual rendered grammar. Public P1 results are separate:
3/3 round trips, 5/5 decoder cases, byte-identical offline runs, 656 native versus
451 compact tokens (31.25% reduction). All description text is preserved;
parenthesized annotations and one call instruction reduce overhead.
Real-model adherence not measured locally; organizer live evaluation remains
authoritative. The available local credential returned HTTP 401.
Router integration is skipped, keeping default provider behavior unchanged.

To reproduce public results:

```sh
curl -fsSL https://registry.nasiko.dev/r/nasiko/compact-tools-eval -o /tmp/compact-tools-eval.json
EVAL_SET=/tmp/compact-tools-eval.json OUT=/tmp/out.jsonl \
  cargo run --release -p nasiko-llm-router --example compact_tools_eval
EVAL_SET=/tmp/compact-tools-eval.json \
  cargo run -p nasiko-llm-router --example compact_tools_tokens
```

PowerShell uses `$env:EVAL_SET` and `$env:OUT` instead of inline assignments.
See [the README](tool-compact/README.md) for the schema subset, native fallback,
stream finalization, limits and optional live-provider environment variables.

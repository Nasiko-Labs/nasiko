# Demo (90 seconds or less)

Hook: "A format that saves tokens but that models cannot write back is a discount on a broken product."

All numbers below come from the commands shown; they were measured on 3 Oct 2026 and are repeated in `README.md`.

1. **Before vs after, offline** (15 s). Show one tool's native schema next to its signature, then the token table.
   ```sh
   EVAL_SET=tool-compact/tests/fixtures/extra_cases.json OUT=/tmp/x.jsonl cargo run -q --release -p nasiko-llm-router --example compact_tools_eval
   EVAL_SET=tool-compact/tests/fixtures/extra_cases.json OUT=/tmp/x.jsonl cargo run -q --release -p nasiko-llm-router --example compact_tools_report
   ```
   Public sample (2 tools): 656 to 596 tokens, 9.1%. Our own 12 cases: 1803 to 1635, 9.3%, with 5 of 12 sent natively because compaction would not help.
2. **It depends on the number of tools** (12 s): `cargo run -q --release -p nasiko-llm-router --example compact_tools_scaling`. One tool costs 45.9% more, two tools 6.1% more, five tools save 24.6%, twenty-five save 39.8%, fifty save 41.3%. Small requests go out unchanged.
3. **Live, real models** (25 s). Set `PROVIDER_BASE_URL`, `MODEL` and `PROVIDER_API_KEY` in your shell, then run the eval with `LIVE_NATIVE_BASELINE=1`; each line gets `live_prompt_tokens` and `native_live_prompt_tokens`. Our runs through a local Nasiko router with a Bedrock provider: Mistral Large 3, 2521 to 1993 billed prompt tokens (20.9% fewer) with 10/12 correct for both arms; gpt-oss-120b, 2377 to 2424 (2.0% more) because that provider already renders native tools compactly. Say that the sample is small and outputs vary between runs.
4. **Invalid is rejected, never repaired** (15 s): `cargo test -p nasiko-tool-compact --test robustness -- --nocapture` prints 64 grammar deviations (none repaired) and 16,000 random mutations (no panic, no schema-invalid call).
5. **Unsupported schema and skipped requests** (8 s): a `$ref`, `oneOf`, numeric bound or colon-containing enum gives `compacted:false` with a reason code and the whole request goes native. In the router, streaming, forced `tool_choice`, tool history, `parallel_tool_calls: false` and legacy `functions` are skipped.
6. **In the router** (10 s): `TOKEN_TOOL_COMPACT=true` (off by default, byte-identical when off); the decoded reply is a standard `tool_calls` entry, and provider `prompt_tokens` are logged before decoding.
7. **Close** (5 s): limits, said plainly. Offline reduction on the 2-tool sample is 9.1%, below the 30% target; adherence varies by model (an independent reviewer's live run on gpt-4o-mini and Gemini 2.5 Flash saw more decoder rejections on one and fewer on the other); cost and latency are not measured.

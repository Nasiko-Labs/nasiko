# Demo (about 2 minutes)

Hook: "A format that saves tokens but that models cannot write back is a discount on a broken product."

1. **Before vs after** (20 s): `bash` the eval, then the report.
   ```sh
   EVAL_SET=tool-compact/tests/fixtures/extra_cases.json OUT=/tmp/x.jsonl cargo run -q --release -p nasiko-llm-router --example compact_tools_eval
   EVAL_SET=tool-compact/tests/fixtures/extra_cases.json OUT=/tmp/x.jsonl cargo run -q --release -p nasiko-llm-router --example compact_tools_report
   ```
   Show one native tools block next to its signature; read the per-case token table.
2. **It depends on the number of tools** (20 s): `cargo run -q --release -p nasiko-llm-router --example compact_tools_scaling`. Say plainly: 2 tools is break-even; 5 tools about 30%; 25 tools about 40%; small sets go out unchanged.
3. **Valid decode, split across chunks** (20 s): `cargo test -p nasiko-tool-compact --test decoder_cases -- --nocapture`; point at the `>>` inside a string case.
4. **Invalid is rejected, never repaired** (20 s): unknown tool, enum violation, trailing comma, duplicate key each return a typed error; `cargo test -p nasiko-tool-compact --test robustness -- --nocapture` prints the table (64 deviations, none repaired; 16,000 mutations, no invalid call).
5. **Unsupported schema** (10 s): a `$ref`/`oneOf`/bound tool gives `compacted:false` with a reason code and the whole request goes native.
6. **In the router** (20 s): `TOKEN_TOOL_COMPACT=true`; same request, provider-billed `prompt_tokens` off vs on (see submission/EVIDENCE.md section C).
7. **Close** (10 s): the comparison table with live correctness (PENDING) and the limits: 16% on the 2-tool public sample, not measured on the private set, cost and latency not claimed.

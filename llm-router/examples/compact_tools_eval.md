# Compact tools evaluation

The evaluator is deterministic and offline by default. It reads a compact-tools data set
from `EVAL_SET` and writes one JSON object per normal and decoder case to `OUT`.
The fixed reference-time system message is included only in live requests, so offline token
comparison measures compacted and native request bodies on the same basis.

```powershell
curl.exe -fsSL https://registry.nasiko.dev/r/nasiko/compact-tools-eval -o $env:TEMP\compact-tools-eval.json
$env:EVAL_SET = "$env:TEMP\compact-tools-eval.json"
$env:OUT = "$env:TEMP\compact-tools-out.jsonl"
cargo run --release -p nasiko-llm-router --example compact_tools_eval
```

`MEASURE_TOKENS=1` prints a local `o200k_base` request-token comparison to stderr; it
does not add scores to `OUT`. The included fixture can be used without a network download:

```powershell
$env:EVAL_SET = "$PWD\llm-router\examples\fixtures\compact-tools-sample.json"
$env:OUT = "$PWD\target\compact-tools-local.out.jsonl"
cargo run --release -p nasiko-llm-router --example compact_tools_eval
```

Schemas outside the crate's documented JSON Schema subset are sent in the native `tools`
form and reported as `compacted: false`; the evaluator continues with the remaining cases.

For an optional live adherence check, set `PROVIDER_BASE_URL` and `MODEL`. Set
`PROVIDER_API_KEY` only when the configured endpoint needs bearer authentication. The
evaluator adds `raw_output` and a decoded `live_calls` object to each normal case.

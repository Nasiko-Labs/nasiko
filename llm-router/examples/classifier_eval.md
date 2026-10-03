# Request classifier evaluation

The evaluator loads one classifier instance, then writes a deterministic JSONL row per
input. `latency_us` is measured per decision after backend loading.

```powershell
curl.exe -fsSL https://registry.nasiko.dev/r/nasiko/classifier-eval -o $env:TEMP\classifier-eval.json
$env:EVAL_SET = "$env:TEMP\classifier-eval.json"
$env:OUT = "$env:TEMP\classifier-out.jsonl"
$env:CLASSIFIER_BACKEND = "local"
cargo run --release -p nasiko-llm-router --example classifier_eval
```

Backends:

- `regex` is the default and exactly wraps the historic request-type classifier. It reports
  complexity `3` and confidence `0.60` because the legacy rules have no calibrated margin.
- `local` is deterministic, offline, and context-aware. It combines lexical request-type
  evidence with query/context complexity signals. It has no model download or network cost.
- `hosted` is reserved for an explicit protocol implementation; this build falls back to regex
  instead of making an unbounded network request.

`CLASSIFIER_TIMEOUT_MS` defaults to `50`; errors, timeouts, unknown backends, and local/hosted
confidence below `CLASSIFIER_MIN_CONFIDENCE` (default `0.55`) fall back to regex. The output's
`fallback` field makes these cases visible.

`CLASSIFIER_MODEL_PATH` and `CLASSIFIER_ENDPOINT` are parsed by the binary configuration for
future local-model and hosted adapters. The built-in `local` backend has no model file, while
the currently unsupported `hosted` selection intentionally exercises the safe regex fallback.

## Labelling data and rubric

The checked-in sample fixture is a smoke set, not a training claim. The local scorer was
designed against a separate hand-labelled development set using these rules:

The checked-in `examples/classifier-data/` train and validation JSONL files contain that small
development set and remain separate from the public evaluator fixture.

- Label the primary requested work, not incidental nouns or pasted code.
- For mixed requests, choose the first action that must succeed; label low-confidence ties as
  `general` during validation rather than forcing a class.
- Complexity 1 is a direct mechanical edit or lookup; 2 is a short single-step response; 3
  requires interpretation or moderate context; 4 requires multiple coordinated steps; 5 has
  architecture, safety, migration, or substantial ambiguity.
- Near duplicates remain in one split. Paraphrases and padded/noisy variants are held out.

The local backend is a deterministic rules-and-features baseline, not a claim that it improves
downstream routing quality. Before production adoption, compare downstream answer quality,
model usage, tokens, and cost against regex on the same workloads.

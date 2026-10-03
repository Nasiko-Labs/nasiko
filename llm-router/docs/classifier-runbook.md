# Run the P2 classifier checks

Use the repository root for these commands. Fetch the public data before an offline run.

```sh
curl -fsSL https://registry.nasiko.dev/r/nasiko/classifier-eval -o /tmp/classifier-public.json
CLASSIFIER_BACKEND=regex EVAL_SET=/tmp/classifier-public.json OUT=/tmp/regex.jsonl cargo run -p nasiko-llm-router --example classifier_eval
python3 llm-router/examples/classifier_metrics.py /tmp/classifier-public.json /tmp/regex.jsonl
```

To use JEV, set `OPENROUTER_API_KEY` in your local environment. Do not put the key in a command, document, or committed file. Set `CLASSIFIER_BACKEND=jev`. Use `CLASSIFIER_ENDPOINT=https://openrouter.ai/api/alpha/decisions`. Organizers can allow this endpoint through their evaluation proxy and supply their own key.

Use `CLASSIFIER_MODEL=typesafe/jev-1.13-20260917` to select the revision verified in the live API check. Set `CLASSIFIER_TIMEOUT_MS=3000` and `CLASSIFIER_MIN_CONFIDENCE=0.6`. The cutoff is an initial conservative setting, not a calibrated result. Record the settings with each run. Freeze them before running validation.

```sh
CLASSIFIER_BACKEND=jev CLASSIFIER_MODEL=typesafe/jev-1.13-20260917 EVAL_SET=/tmp/classifier-public.json OUT=/tmp/jev.jsonl cargo run -p nasiko-llm-router --example classifier_eval
python3 llm-router/examples/classifier_metrics.py /tmp/classifier-public.json /tmp/jev.jsonl
```

Repeat the same commands with `llm-router/tests/data/classifier-validation.json` as `EVAL_SET`. Keep validation results separate from public smoke results. Run twice and compare predicted type, difficulty, confidence, and fallback reason. Elapsed time can differ. A fixed model revision does not guarantee identical hosted probabilities.

The metric script reports category accuracy, difficulty error, confidence calibration, and decision latency. Unknown decision cost remains unknown. Report downstream answer quality and total workload cost before claiming routing savings.

For the demo, show one context-dependent classification and one failure fallback. Run `cargo run -p nasiko-llm-router --example cache_switch_demo` with `DATABASE_URL` pointing at your migrated local database. The example creates PostgreSQL temporary tables on one connection. It runs the real cost policy with synthetic rates and usage. The tables disappear on exit. Existing application rows remain intact. It demonstrates keep, switch, and missing-evidence cases. Describe these costs as estimates. They do not show measured production savings.

## Repeatable local evaluation

Use `CLASSIFIER_BACKEND=local CLASSIFIER_MODEL=local-training-v1`. No key or network is required. Keep the confidence cutoff at the documented 0.6 for the guarded results. Run twice in separate processes and compare category, difficulty, confidence, and fallback fields. Exclude latency from prediction comparison. The current small model falls back on all evaluated cases at this cutoff and does not improve accuracy. JEV remains the more accurate measured backend, with hosted repeatability unsupported.

The classifier environment loader lives at `src/bin/llm-router/config.rs`. Library consumers pass a resolved `GatewayConfig` to `LlmRouterCtx::from_shared_with_config`; legacy `from_shared` uses regex defaults for classifier settings.

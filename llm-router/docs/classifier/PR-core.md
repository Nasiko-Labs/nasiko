# [classifier] Pluggable request classifier with Jev (hosted) and Laya (local) backends and regex fallback

**Track:** P2 — Request classifier for cost-aware routing.

## What this adds

- `RequestClassifier` trait (`routing/classifier.rs`), `RegexClassifier` wrapping the
  existing `classify_request_type` unchanged, and a shared `ClassifierService` that owns
  validation, timing, the overall deadline, counted regex fallback and a configurable
  low-confidence abstention. The router holds it once (`LlmRouterCtx.classifier`); the eval
  example and routing use the same path.
- A Jev (typesafe.ai) hosted backend (`routing/jev.rs`) using the documented HTTP API:
  one POST with the query and bounded context as `state`, a Choice over the seven request
  types and a Score over the five complexity levels. Full response validation, bounded
  input/response/concurrency, no redirects, credential redaction, opt-in bounded retry.
- A Laya (Convai Innovations, Apache-2.0) local backend (`routing/laya.rs`): the published
  ONNX export run in-process through ONNX Runtime (`ort`, dynamically loaded) with the
  checkpoint's tokenizer; same rubric as Jev (`routing/rubric.rs`), model loaded once and only
  when selected, bounded blocking queue, token-level truncation reported. Setup script
  `scripts/laya-setup.sh` (pinned revision, sha256-verified, ~1.7 GB, no key, no Python).
- Bounded, role-labelled classifier context from both wire formats (`routing/context.rs`,
  Responses adapter), excluding system prompts, tool results and non-text parts.
- Opt-in deterministic tier sampling (`CLASSIFIER_ROUTING_SEED`); legacy entropy RNG
  otherwise.
- `examples/classifier_eval.rs` extended to the trait path with the exact harness contract,
  plus a diagnostics sidecar; new `examples/classifier_report.rs` scorer.
- Labelled dev/calibration/held-out data with a split manifest and a leakage test;
  docs in `llm-router/docs/classifier/`.

Default behaviour is unchanged: `CLASSIFIER_BACKEND=regex` is the default, nothing is
contacted, downloaded or required, and existing routing tests pass unmodified. **Dependency
note:** `ort` 2.0.0-rc.13 (`load-dynamic`, no binary download) and `tokenizers` 0.23 are
added to the workspace for Laya; they build offline after `cargo fetch`.

## How to run

```sh
curl -fsSL https://registry.nasiko.dev/r/nasiko/classifier-eval -o /tmp/classifier-eval.json
EVAL_SET=/tmp/classifier-eval.json OUT=/tmp/classifier-out.jsonl \
cargo run --release -p nasiko-llm-router --example classifier_eval
```

Env vars (all optional): `CLASSIFIER_BACKEND=regex|jev|laya` (default `regex`),
`CLASSIFIER_ENDPOINT` (default `https://api.typesafe.ai/v1/systemone`),
`CLASSIFIER_MODEL` (default `jev-1.13.0`), `TYPESAFE_API_KEY` (required for `jev`),
`CLASSIFIER_TIMEOUT_MS` (3000), `CLASSIFIER_MIN_CONFIDENCE` (0.0), `CLASSIFIER_ROUTING_SEED`,
`CLASSIFIER_MAX_CONCURRENCY` (8), `CLASSIFIER_RETRIES` (0); Laya: `CLASSIFIER_MODEL_PATH`,
`CLASSIFIER_ORT_DYLIB`, `CLASSIFIER_THREADS` (see `docs/classifier/LAYA.md`). Diagnostics go to
`<OUT>.diagnostics.jsonl` (`DIAG_OUT`); `OUT` carries only the harness fields.

## Model IDs and hosted endpoint

- Jev: `jev-1.13.0` (versioned; the response's `model` field is recorded per call). Endpoint
  to allow on the egress proxy: `https://api.typesafe.ai/v1/systemone` (POST). The key is
  sent only there; redirects are not followed.
- Laya: `receptron/laya-onnx@68f27dfe…` (ONNX export of `convaiinnovations/laya@55cf4c4e…`,
  ModernBERT-large 421M, Apache-2.0), fetched once by `scripts/laya-setup.sh`; no network at
  run time.

## Measured results

Same data, same scorer (`docs/classifier/RESULTS.md` has every table):

| | regex | laya (local, M4 CPU) | jev |
|---|---|---|---|
| held-out (48) accuracy / macro F1 / ECE | 27.1 % / 0.258 / 0.138 | **56.2 % / 0.542 / 0.159** | not run — credentials/billing unavailable |
| held-out complexity exact / ±1 / MAE | 12.5 % / 43.8 % / 1.44 | 33.3 % / 77.1 % / 0.98 | not run |
| public sample (10) accuracy | 30.0 % | 50.0 % | not run |
| p50 / p95 decision latency | 4 µs / 67 µs | 843 ms / 1.98 s | not run |
| init / memory | 12 ms / — | 1.1–5.2 s / ~1.75 GB RSS | — |
| fallback rate | 0 | 2.1 % (one 3 s timeout) | not run |
| repeatability (2 runs, semantic fields) | identical | identical | not run |
| cost per decision | $0 | no API fee; ~0.85 s CPU + 2 GB RAM | $0.042/M input tokens, not measured |

Laya's wins and losses are both real: it is twice as accurate as regex on held-out but reads
technical writing as code (4/8) and reasoning as explanation (1/5 right), is over-confident
in places, and sees only ~200 tokens of the request. Jev was not run; its row is empty, not
estimated.

## Known limits and unsupported cases

- Live Jev accuracy, latency, cost and repeatability are unverified ("not run"); Jev
  documents no seed/temperature, so repeatability is measured, not assumed.
- Laya: English checkpoint only; ~200-token effective window; fp32 only; 3 s default
  timeout is tight on CPU (set ~6000 ms for a Laya deployment); needs ~2 GB RAM.
- Only text is classified; multimodal parts contribute nothing.
- Regex complexity (3) and confidence (0.5/0.3) are uncalibrated placeholders.
- Complexity is predicted and reported but does not change tier selection; the bandit key
  is unchanged. No downstream routing-quality or cost-saving claim is made.
- Abstention is off by default (`CLASSIFIER_MIN_CONFIDENCE=0`) until a floor is chosen on
  the calibration split with live data.
- Labels in the own data are reviewed synthetic labels, not independently adjudicated.
- Harness note: `latency_us` is real timing and will differ between runs; the scorer diffs
  the semantic fields (`PRED2=`).

## Checks run

`cargo fmt --check`, `cargo clippy -p nasiko-llm-router --all-targets` (zero warnings),
`cargo test -p nasiko-llm-router` (all passing; live Jev and local Laya tests `#[ignore]`d,
the Laya ones run here against the real bundle: parity 5e-5, repeatability, timeout fallback),
`cargo check -p nasiko-server` (host call sites compile), the release evaluator twice on
the public sample and once per split, and `classifier_report` on each.

🤖 Generated with [Claude Code](https://claude.com/claude-code)

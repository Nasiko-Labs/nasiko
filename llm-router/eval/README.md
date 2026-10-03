# Request-classifier evaluation

`requests.jsonl` holds 100 hand-labelled requests: request type (the router's seven
`RequestType` wire names) and complexity 1–5 (trivial → expert). They are split into 30 `dev`
rows (used while building) and 70 `test` rows (scored once at the end). The set deliberately
includes cases the regex classifier has no way to get right: Hinglish, Hindi and Spanish,
negations ("don't write any code…"), keyword collisions ("refactor my resume") and
context-dependent follow-ups ("make it funnier and shorter").

```sh
# 1. a local laya-serve (the compose service publishes no host port)
python -m pip install 'laya[serve]==0.3.24'
LAYA_MODELS=multilingual laya-serve

# 2. the report (regex vs. Laya through the router's own code paths)
cargo run -p nasiko-llm-router --example classifier_report -- test
```

There are two stages:

1. **Classification**: type accuracy, complexity MAE and ±1 accuracy, confidence
   calibration and latency for each backend.
2. **Routing replay**: each classification goes to the router's `pick_tier` under 50 fixed
   seeds with no learned cells, which is the cold-start case where the classifier alone
   decides. The report gives the mean tier cost (15 / 3 / 0.8), the share of hard requests
   (4–5) sent to Tier3, and the share of easy ones (1–2) sent to Tier1.

It also runs a third stage, an online-learning simulation of the Thompson loop with
feedback. Laya's labels are deterministic for a given checkpoint revision; the latency figures
are not, and they depend on the hardware. The maintained, gated numbers are in
`benchmark-report.md` (from `tests/classifier_benchmark.rs`).

## In-process local classifier (`REQUEST_CLASSIFIER=local`)

`train_local.py` trains the hashed n-gram model embedded at `assets/local_classifier.json` from
`train-a.jsonl` through `train-e.jsonl` (1,806 rows after leakage cleanup). Features are computed by Rust
(`examples/local_classifier_features.rs`), so training and serving cannot disagree. The trainer
refuses to run if any `h4-validation.json` query near-duplicates a training query.

The benchmark is a cargo integration test, `tests/classifier_benchmark.rs`, so it runs with
the rest of the suite and in CI (`.github/workflows/llm-router-classifier.yml`). It scores
regex, local, (when `LAYA_URL` reaches a `laya-serve`) Laya, and
(when `STRANDS_URL` reaches a ready sidecar) raw/optimized Strands through the router's own code on
`h4-validation.json` (154 held-out, tagged by failure mode), `h4-public-sample.json` (the
hackathon's 10 public examples, byte-identical to
https://registry.nasiko.dev/r/nasiko/classifier-eval) and `requests.jsonl` (classified from real chat history via
`classify_input`). It reports accuracy, macro-F1, per-type F1, complexity exact/±1/MAE/hard
recall, ECE/Brier, latency p50/p95/p99, the routing outcome through `pick_tier` (mean tier
cost, hard/easy → cheapest tier), per-tag accuracy, and the requests each backend fixes or
breaks. Quality gates fail the build if the local model regresses.

```sh
cargo test --release -p nasiko-llm-router --test classifier_benchmark -- --nocapture
LAYA_URL=http://localhost:8000 BENCH_REPORT=eval/benchmark-report.md   cargo test --release -p nasiko-llm-router --test classifier_benchmark -- --nocapture
```

`benchmark-report.md` records the available backends for the latest run. Set `LAYA_URL`
only when the sidecar is running. Set `STRANDS_URL` to include both Strands modes;
its benchmark requests use `STRANDS_TIMEOUT_MS` (default 1500). The committed
Strands quality comparison uses an explicit 30000 ms offline timeout and fails on
any unexpected source/fallback. `BENCH_PREDICTIONS` saves predictions and their SHA
manifest for rescoring. Without endpoint variables the benchmark compares regex
and local.

### Retraining

Run `python -u eval/train_local.py` from `llm-router/` with NumPy, SciPy,
scikit-learn and Cargo available. The trainer selects regularization and temperature
using five-fold cross-validation grouped by training family; augmentation stays in
its original family. Validation labels are not used for fitting.

The October 2026 retraining audit found two validation overlaps in the newly added
data: `trd-0085` and `tre-0403`. Their complete families (`trd-f073` and
`tre-f135`, three rows each) were removed from training instead of changing the
held-out evaluation. The remaining 1,806 originals pass the unchanged 0.7 Jaccard
leakage guard (maximum similarity 0.64) and generate 1,762 robustness copies.


Latest training environment: macOS arm64, Python 3.13, NumPy 2.2.6, SciPy 1.17.1,
scikit-learn 1.8.0. `OPENBLAS_NUM_THREADS`, `OMP_NUM_THREADS`, and
`VECLIB_MAXIMUM_THREADS` were set to 1. The retained confidence floor is 0.4; see
`benchmark-report.md` for coverage and the public-sample regression.

## Optimized Strands

`REQUEST_CLASSIFIER=strands` combines one remote Strands type question with the
embedded retrained complexity head. `strands_raw` retains the native two-question
baseline. The optimized backend reports selected-label probability confidence,
while preserving the native 0.4 concentration admission policy with the equivalent
probability minimum `(1+6*0.4)/7`. Uncertain requests still serve the configured
model without caching, and failures return regex. The bundled server warms short
and long client requests before readiness, pins checkpoint/base revisions, and
skips expired queued work. It is a serial Apple-silicon evaluation runner.

See [Strands reproduction and confidence audit](strands/README.md) and the
[frozen real-user evaluation](real-world/README.md). Calibration uses only training
families; its sharpening candidate is rejected because accepted errors increase
in grouped CV. `ROUTING_SEED=<u64>` enables reproducible tier sampling for the same
classification and learned state; unset preserves exploration.

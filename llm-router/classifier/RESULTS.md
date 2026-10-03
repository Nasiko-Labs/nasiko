# Measured P2 results

These are development measurements, not private-set or downstream routing claims.
Full metrics, error IDs, dataset hashes and model provenance are in `results.json`.
Predictions were produced by the release Rust `classifier_eval` executable through
the same runtime as live routing. Hardware: Windows, Intel Core i7-13650HX,
ONNX Runtime 1.30.0 CPU, two inference threads, one persistent worker.

| Authored test set (28 cases) | Type accuracy | Complexity MAE | p50 ms | p95 ms |
|---|---:|---:|---:|---:|
| Original regex | 28.6% (8/28) | 1.321 | 0.004 | 0.061 |
| Trained MiniLM, raw, threshold 0 | 96.4% (27/28) | 0.179 | 2.371 | 3.075 |
| Trained MiniLM, default threshold 0.35 | 96.4% (27/28) | 0.179 | 2.273 | 2.955 |

The default model had zero low-confidence fallbacks. Raw-model ECE with ten bins
was 0.071; calibration remains uncertain on this small set. Regex confidence is a
fixed zero placeholder, so comparing its numerical ECE as calibration would be
misleading. Confidence gating is useful but does not catch confident errors.

The splits are 79 train / 45 validation / 28 test. The validation set selected
epoch 5 and fitted type temperature 0.5 and effort temperature 0.5874.
Validation type accuracy was 95.6% (43/45). Training data was expanded with
rubric-authored boundary examples for config edits, follow-up writing, API facts,
proofs, debugging versus design, parsers and diagnostics. Exact-query/family
overlap checks pass. Test examples remain unchanged and were not directly read
by training, checkpoint selection or calibration. However, test/public error
analysis informed the added examples: these are development results, not fresh
blind evaluations. This small synthetic dataset cannot establish real-world
generalization.

On the official ten-case public **smoke** set, raw-model type accuracy was 90%
versus regex 30%; complexity MAE 0.4 versus 1.2. Model p50/p95 was 3.924/11.140 ms.
This smoke set is not evidence of final hackathon performance. No public case
was added to training, and no private labels were available.

## Efficiency, determinism and recovery

- ONNX weights: 58,617,569 bytes (55.9 MiB), plus tokenizer/manifest. Dynamic INT8
  quantization applies to MatMul; embedding weights remain FP32. The base encoder
  has approximately 22.7 million parameters. Training/export took about 22 seconds
  after downloading dependencies/base weights on this machine.
- Four cold loads were 561, 503, 515 and 632 ms, excluded from per-call timing.
- Two fresh release runs produced exactly identical type/complexity/confidence
  predictions for all 28 test cases. `latency_us` varies as expected. Opt-in seeded
  tier selection is covered by Rust tests; default routing remains unchanged.
- Ordinary raw local evaluation had zero timeout/error fallbacks. Missing model
  path produced 28/28 startup fallbacks, exactly matching the regex predictions.
- A deliberately aggressive 1 ms deadline produced 21/28 timeout fallbacks, exited
  successfully and emitted all rows. This exercises real subprocess cancellation;
  remaining requests finished before the timeout timer was observed. Tokio
  deadlines include queueing and IPC and are not hard real-time interrupts.
- API fee per local decision: $0. Hardware billing depends on deployment. Median
  active elapsed compute is about 0.0024 seconds on two CPU threads; marginal
  compute cost is `hourly machine price * 0.0024 / 3600`, excluding idle capacity,
  memory, loading and hosting overhead. No measured dollar savings are claimed.
- Rust regression suite: 396 passed, 7 ignored (live provider/DB prerequisites).
  Five additional offline Python artifact tests passed. Tests cover context,
  contract validation, missing/corrupt weights, timeout/error/low-confidence
  fallback, provider overrides, repeatable tier choice and sticky continuation.

## Negative results and next work

Unadapted Laya was tried first with the same rubric and validation cases through
the Python development transport. Type accuracy was 38.1%, complexity MAE 0.810,
p50/p95 1911.7/2429.0 ms and load about 8.4 seconds. Its root weights were about
842 MB. This was an exploratory probe, not the final Rust timeout-controlled
backend comparison. It confused code changes with design, so it was retained
as an optional comparison adapter rather than selected for deployment. Jev was
not benchmarked because no authorized hosted credentials were supplied.

The selected compact model still labels an authored word problem as factual
lookup (confidence 0.617) and a public session-loss diagnostic as code generation
instead of analytical reasoning (confidence 0.542). These
are visible counterexamples to any claim that confidence guarantees correctness.

Before a competitive final submission, expand independently labelled scenario
families, especially ambiguous/multi-intent/noisy/OOD cases; evaluate calibration
on a larger validation set; obtain a fresh blind test after any further tuning;
measure simultaneous request load and memory; and validate answer quality,
tokens and costs to validate the complexity-to-tier policy. Semantic routing now
adds a fixed tier-score adjustment of weight 0.12: low complexity favors T3,
medium T2, and high T1. Existing quality/cost scores and feedback can outweigh
this bias. Existing provider-specific bandit keys are preserved; this is not
complexity-specific feedback storage. No downstream quality or cost improvement
has been measured.

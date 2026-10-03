# [classifier] Add calibrated subword classification with ordinal complexity

## Track

`classifier` / P2 Request classifier.

## How to run

```sh
python llm-router/scripts/classifier_train.py

curl -fsSL https://registry.nasiko.dev/r/nasiko/classifier-eval -o /tmp/classifier-eval.json

CLASSIFIER_BACKEND=regex EVAL_SET=/tmp/classifier-eval.json OUT=/tmp/regex.jsonl \
  cargo run --release -p nasiko-llm-router --example classifier_eval

CLASSIFIER_BACKEND=local EVAL_SET=/tmp/classifier-eval.json OUT=/tmp/local.jsonl \
  cargo run --release -p nasiko-llm-router --example classifier_eval

cargo test -p nasiko-llm-router
```

## Model IDs used

None. The opt-in local backend is deterministic, offline, and does not make a network/model call per routing decision.

## What changed

This PR keeps the existing regex classifier as the default/safety path and adds an opt-in `RequestClassifier` implementation with:

- query + prior-turn context as separate classifier inputs
- word uni/bi-grams
- character 3–5 gram features
- structural / intent features
- a calibrated seven-way request-type head
- four cumulative ordinal heads for complexity 1–5
- conservative high-precision rubric tie-breakers for explicit intents
- timeout, load-error, inference-error, invalid-output and low-confidence fallback
- sticky continuation/tool-loop routing unchanged
- classification only at safe cold-start/switch cache-miss boundaries
- numeric sparse runtime weights (no per-feature string allocation in the inference hot path)

The cumulative complexity decoder enforces a monotone threshold prefix, so inconsistent independent heads cannot produce an impossible ordinal pattern.

## Production context integration

The evaluator already supported `query + context`; this PR now closes the production/evaluator mismatch as well.

For Chat Completions, the router passes:

- the latest user turn as `query`
- up to the three most recent prior user turns as bounded `context`
- a 4096-character cap on context

Continuation/tool-loop routing stays sticky and does not trigger reclassification.

The Responses surface explicitly keeps `context: None` until an equivalent canonical prior-turn extraction seam is available there.

## Training and generalization

The local model is generated deterministically by:

```sh
python llm-router/scripts/classifier_train.py
```

Training uses scenario-family isolation so paraphrases/variants from the same semantic family never cross train/validation boundaries.

The corpus was expanded with additional genuinely distinct scenario families across all seven classes, including:

- code generation vs code understanding near-misses
- technical design vs analytical reasoning
- prose rewriting/editing
- factual lookup vs reasoning
- noisy/padded formulations
- production/reliability context
- ambiguous and multi-step technical requests

The public smoke cases are not copied or paraphrased into training data.

## Validation evidence

A full hardened-branch validation run completed successfully with:

- `cargo check --workspace` ✅
- `cargo clippy --workspace --all-targets -- -D warnings` ✅
- `cargo test -p nasiko-llm-router` ✅
- 384 passed, 0 failed, 1 ignored in the main router test target
- additional router test targets passed
- official public evaluator executed successfully for both regex and local backends ✅

On that run, local classification latency after model load was generally sub-millisecond to ~1.2 ms per public row, while preserving deterministic type/complexity/confidence outputs.

The corpus/model artifact has since been regenerated from the broader family-isolated dataset; current metrics are reproducible from the commands above rather than hard-coded as unsupported claims.

## Safety / compatibility

Existing behavior remains default:

```text
CLASSIFIER_BACKEND=regex
```

The local backend is opt-in.

Any model load failure, timeout, invalid output, inference error, or confidence below the configured threshold falls back safely.

The existing Thompson-sampling tier learner is preserved. Complexity is returned and evaluated but does not replace the existing adaptive bandit.

## Why this design

The goal is to improve the exact failure modes of a regex-only classifier without paying for another LLM call before every routed LLM request.

Character subwords improve robustness to typos, morphology and unseen technical terms. Word features preserve semantic intent. Explicit context features let prior turns influence ambiguous requests. Ordinal complexity models the fact that 4 vs 5 is a smaller error than 1 vs 5. Calibration and fallback prevent the local model from becoming a brittle single point of failure.

## Known limitations

- Labels are still synthetic / single-author and should not be treated as a production benchmark.
- Confidence calibration is only as representative as the authored validation distribution.
- The Responses API surface does not yet have equivalent prior-turn context extraction.
- No downstream answer-quality or dollar-cost-saving claim is made without downstream evaluation.
- Complexity currently informs classifier output/evaluation only; it does not override the existing Thompson bandit.

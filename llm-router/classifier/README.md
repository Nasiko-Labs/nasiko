# P2: context-aware request classification

The local backend is a trained MiniLM transformer with two heads: seven request
types and five ordered complexity levels. It attends to the current query and
recent conversation context, then pools query tokens so a long conversation does
not dominate the requested action. The final two encoder layers and heads are
adapted; inference uses a partially INT8-quantized ONNX model on CPU. No paid API
or network is needed during local inference.

`RequestClassifier` is the model-independent async seam. Both live routing and
`classifier_eval` use `ClassifierRuntime::run`, including the same timeout,
validation, confidence threshold and regex fallback. Regex remains the default.
The optional hosted backend speaks the System One probability-distribution
protocol; it is not an arbitrary OpenAI chat-completions endpoint.

## Prepare the model before evaluation

Run from the repository root. Use Python 3.13 and a virtual environment. Training
needs PyTorch, Transformers, ONNX and scikit-learn through `laya[onnx]`; deployment
needs only the three packages in `requirements-local.txt`. No Rust dependencies
were added. Downloading happens explicitly during preparation, never at runtime.

```powershell
python -m venv .venv
$py = "$PWD\.venv\Scripts\python.exe"
& $py -m pip install -r llm-router/classifier/requirements.txt
& $py llm-router/classifier/prepare_data.py
& $py llm-router/classifier/fetch_model.py --repo sentence-transformers/all-MiniLM-L6-v2 --revision 1110a243fdf4706b3f48f1d95db1a4f5529b4d41 --output .classifier-work/minilm
& $py llm-router/classifier/train_semantic.py --base .classifier-work/minilm --output .classifier-work/semantic
```

On Unix, use `.venv/bin/python` instead. Base MiniLM is
[Apache-2.0 licensed](https://huggingface.co/sentence-transformers/all-MiniLM-L6-v2).
`provenance.json` records the immutable base revision and file SHA256s;
`semantic_model.json` records train/validation hashes, seed, selected epoch,
temperatures and ONNX SHA256. Generated weights are local artifacts, excluded
from Git. Rebuild them with the commands above; they are not downloaded from a
submission-specific hosting account.

## Run the real evaluation path

```powershell
$env:CLASSIFIER_BACKEND = 'local'
$env:CLASSIFIER_PYTHON = "$PWD\.venv\Scripts\python.exe"
$env:CLASSIFIER_MODEL_PATH = "$PWD\.classifier-work\semantic"
$env:EVAL_SET = "$PWD\llm-router\classifier\data\test.json"
$env:OUT = "$PWD\.classifier-work\local-out.jsonl"
cargo run --release -p nasiko-llm-router --example classifier_eval
& $py llm-router/classifier/metrics.py --dataset $env:EVAL_SET --outputs $env:OUT

$env:CLASSIFIER_BACKEND = 'regex'
$env:OUT = "$PWD\.classifier-work\regex-out.jsonl"
cargo run --release -p nasiko-llm-router --example classifier_eval
```

The output contains exactly `id`, `request_type`, `complexity`, `confidence`,
`latency_us`. Model loading happens once before the loop. Fallback reasons/counts
go to stderr. Timing naturally varies; compare repeated predictions excluding
`latency_us` using `metrics.py --repeat SECOND_OUTPUT`. Evaluation reads only
query/context, never reference labels or `tier_hypothesis`.

The same environment applies to the standalone `llm-router` binary. Embedded
users (including `server`) explicitly inject an `Arc<ClassifierRuntime>` into
`LlmRouterCtx.request_classifier` and prepare it before serving; library code
does not read configuration from the environment.

| Variable | Default / meaning |
|---|---|
| `CLASSIFIER_BACKEND` | `regex`; `local` or `hosted` opt in |
| `CLASSIFIER_MODEL_PATH` | Required local directory |
| `CLASSIFIER_PYTHON` | `python3`; set explicitly on Windows |
| `CLASSIFIER_WORKER` | Included `model_worker.py` |
| `CLASSIFIER_THREADS` | `2` CPU threads |
| `CLASSIFIER_TIMEOUT_MS` | `2000`, including queueing and IPC |
| `CLASSIFIER_LOAD_TIMEOUT_MS` | `180000`, excluded from inference latency |
| `CLASSIFIER_MIN_CONFIDENCE` | `0.35`; `0` disables abstention for raw comparison |
| `CLASSIFIER_TEMPERATURE` | `1`; optional additional type-probability rescaling |
| `CLASSIFIER_SEED` | `42`; deterministic opt-in tier sampling |
| `CLASSIFIER_ENDPOINT` | Required full hosted POST URL |
| `CLASSIFIER_API_KEY` | Optional separate bearer key; never routed provider keys |
| `CLASSIFIER_MODEL` | Optional hosted model name |

Hosted responses must provide `answers.request_type.probabilities` with exactly
the seven labels and `answers.complexity.probabilities` indexed `0..4` (levels
1..5). Every distribution is checked for valid mass. Redirects are disabled.
No hosted production endpoint was benchmarked or implicitly enabled.

## Labels, confidence and routing

Label by the requested deliverable: edits, implementations and code fixes are
`code_generation`; explaining/reviewing existing code is `code_understanding`;
architecture/tradeoffs are `technical_design`; derivations and analysis are
`analytical_reasoning`; prose/translation is `writing`; direct definitions and
known facts are `factual_lookup`; greetings or unresolved intent are `general`.
Context resolves references, but does not override the latest requested action.
For multiple intents, use the main requested deliverable. The complete rubric is
in `assets/classifier_questions.json`.

Complexity measures expected effort, not length: 1 is trivial, 2 a small bounded
task, 3 several steps/constraints, 4 substantial interaction/reasoning, 5 difficult
work with multiple interacting uncertainties. Confidence is the winning type's
probability after validation-fitted temperature scaling. It is an estimate, not
a correctness guarantee. Regex returns complexity `3`, confidence `0`: both are
documented placeholders, not model estimates.

Low confidence, invalid output, timeout, inference/network failure or load failure
returns the original query-only regex result and increments a fallback counter.
The 0.35 threshold was selected during initial validation and retained after
expanding the data. Raw-model results use threshold 0 and are reported separately.
Regex can itself be wrong;
this fallback preserves existing behavior rather than guaranteeing quality.

Classification occurs at `cold_start`/`switch` only. Cached decisions remain
sticky on `continue`, including tool loops. Pinned models, per-provider tier
registry/overrides and existing quality/cost weights are preserved. The existing
salience gate remains active and receives context in semantic mode. Opt-in tier
sampling uses a seeded RNG mixed with provider/query/context, giving repeatable
selection for identical inputs and learned cells. Default routing keeps its
existing randomness. In semantic mode, complexity adds a bounded tier-score bias
with weight 0.12: level 1 favors T3, level 3 favors T2, and level 5 favors T1.
Quality feedback and cost remain part of the score; complexity does not force a
tier. Persisted bandit keys remain `(provider, tier, request_type)`. Routing
quality and cost benefits still need downstream validation.

The local worker is persistent and serializes inference. Cancellation kills a
worker with an outstanding response, preventing response mixups; subsequent
requests can restart it. Context is bounded to recent user/assistant messages
and 4096 characters; system/developer/tool messages are excluded. The model has
a 256-token joint budget. Excess context is dropped from the left; an oversized
query fails to regex instead of silently discarding its instructions.

## Evidence and limits

See [RESULTS.md](RESULTS.md) for measured results and known errors. The authored
dataset contains 79 training, 45 validation and 28 test cases; it is small,
synthetic and English-only. `prepare_data.py` rejects family/exact-query leakage;
scenario families were authored separately, with no generated paraphrase splits.
Validation selects epoch/temperatures. Training never directly reads test or
public examples, but development error analysis informed new boundary examples.
The test examples remain unchanged; both evaluations are development evidence,
not blind generalization estimates. The private hackathon set is unavailable. More independently
labelled data, ambiguity/OOD calibration, multilingual coverage, concurrent-load
testing and downstream answer-quality/cost evaluation remain before production.

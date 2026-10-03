# Request classifier (`[classifier]`)

A pluggable `RequestClassifier` for cost-aware routing. **The regex classifier stays the default**; every
other backend is opt-in and falls back to regex on any failure.

## Interface (`llm-router/src/routing/request_classifier.rs`)

```rust
#[async_trait]
pub trait RequestClassifier: Send + Sync {
    fn name(&self) -> &str;
    fn fallback_count(&self) -> u64;                       // answers served by the regex fallback
    async fn classify(&self, query: &str, context: Option<&str>) -> Result<Classification, String>;
}
pub struct Classification { pub request_type: RequestType, pub complexity: u8 /*1-5*/, pub confidence: f64 /*0-1*/ }
```

Equivalent to the brief's `ClassifyInput`/`ClassifyError` shape (query/context as arguments, `String`
errors, `f64` confidence). The router holds `Arc<dyn RequestClassifier>` (`LlmRouterCtx.request_classifier`).

| semantics | |
|---|---|
| `request_type` | one of the router's 7 `RequestType` values |
| `complexity` | 1–5 per the rubric in `DATA.md`. **Reported only; it is not part of the bandit key** (the tier bandit stays keyed on `RequestType`), so it cannot change routing today. |
| `confidence` | regex: `(best+1)/(best+second+2)` vote margin, uncalibrated, deterministic. local/http: calibrated max-softmax. llm: the model's self-report (uncalibrated; see results). |
| low confidence | `FallbackRequestClassifier` answers with regex when `confidence < REQUEST_CLASSIFIER_MIN_CONFIDENCE` and counts a fallback. This is the safe default. |

Routing integration (`routing::route_model`): classification still happens only at fireable boundaries
(`cold_start`, `switch`) after the salience gate, and only on a decision-cache miss. `continue` steps and
cache hits never call the classifier, so the selected tier stays sticky through tool loops
(tests: `classifier_is_never_called_on_continue_steps`,
`classifier_runs_once_at_cold_start_and_switch_but_not_on_cache_hit`). Classification is not moved into the
orchestrator. A classifier error never fails routing (`failing_classifier_falls_back_to_regex…`).

**Determinism.** Every classifier here is deterministic for identical input (checked: two eval runs diff
equal apart from `latency_us`). Tier *selection* is still Thompson-sampled by the existing bandit
(`pick_model_thompson`, entropy RNG) — that is unchanged pre-existing behaviour and is not made
deterministic by this work.

## Backends — configuration (read in `llm-router/src/config.rs`)

| env var | default | meaning |
|---|---|---|
| `REQUEST_CLASSIFIER_BACKEND` | `regex` | `regex` \| `local` \| `http` \| `llm`. Unknown/incomplete config → regex. |
| `REQUEST_CLASSIFIER_ENDPOINT` | — | `http`: full URL of a `/classify` endpoint. `llm`: OpenAI-compatible base URL ending in `/v1`. |
| `REQUEST_CLASSIFIER_MODEL` | — | `llm`: chat model id. |
| `REQUEST_CLASSIFIER_MODEL_PATH` | bundled | `local`: weights JSON; empty uses the model compiled into the binary. |
| `REQUEST_CLASSIFIER_API_KEY` | — | optional bearer token (environment only, never committed). |
| `REQUEST_CLASSIFIER_TIMEOUT_MS` | `500` | hard timeout; timeout → regex fallback. |
| `REQUEST_CLASSIFIER_MIN_CONFIDENCE` | `0.5` | below this → regex fallback. |

* **`local`** — offline lexical model (word 1–2-grams + char 3–5-grams, TF-IDF, multinomial logistic
  regression, temperature-calibrated), pure Rust, 549 KiB JSON bundled in the binary. No network, no
  native deps. `train_lexical.py` exports it; golden vectors in the file are re-checked on every load, so
  the Rust featurizer cannot drift from `lexical.py`.
* **`http`** — any service returning `{request_type, complexity, confidence}`. `serve.py` is a reference:
  `bge-small-en-v1.5` embeddings (via `fastembed`/ONNX, CPU) + logistic regression + ridge for complexity.
* **`llm`** — OpenAI-compatible `/chat/completions`, temperature 0, JSON reply, `max_tokens` 1500 because
  reasoning models spend tokens before answering (null content → fallback).

## How to run

```sh
curl -fsSL https://registry.nasiko.dev/r/nasiko/classifier-eval -o /tmp/classifier-eval.json

# default: regex baseline
EVAL_SET=/tmp/classifier-eval.json OUT=/tmp/out.jsonl \
  cargo run --release -p nasiko-llm-router --example classifier_eval

# offline local model
REQUEST_CLASSIFIER_BACKEND=local EVAL_SET=/tmp/classifier-eval.json OUT=/tmp/out.jsonl \
  cargo run --release -p nasiko-llm-router --example classifier_eval

# embedding service
cd llm-router/classifier && python -m venv .venv && . .venv/bin/activate && pip install -r requirements.txt
python build_dataset.py && python train.py --public /tmp/classifier-eval.json   # writes model.joblib
uvicorn serve:app --port 8099 &
REQUEST_CLASSIFIER_BACKEND=http REQUEST_CLASSIFIER_ENDPOINT=http://127.0.0.1:8099/classify \
REQUEST_CLASSIFIER_MIN_CONFIDENCE=0.7 EVAL_SET=/tmp/classifier-eval.json OUT=/tmp/out.jsonl \
  cargo run --release -p nasiko-llm-router --example classifier_eval

# score any OUT
python score.py /tmp/classifier-eval.json /tmp/out.jsonl --min-confidence 0.7

# retrain the offline model (rewrites llm-router/models/request-classifier-lexical-v1.json)
python train_lexical.py --public /tmp/classifier-eval.json
```

`classifier_eval` reads `EVAL_SET` (`examples` array) and writes one line per case to `OUT`:
`{id, request_type, complexity, confidence, latency_us}`. The classifier is built once before the loop; a
backend error never aborts the run (that case is answered by regex). stderr reports
`fallbacks (backend-reported: N)`.

Hosted backend used for the comparison: endpoint host `bedrock-mantle.us-east-1.api.aws`, model
`openai.gpt-oss-120b`, temperature 0. Keys come from the environment only.

## Measured results

Own labelled set: 220 seeds (`DATA.md`). Trained models are scored by grouped 5-fold cross-validation
(seeds never straddle folds, only originals scored); regex and the hosted model need no training so they
run directly on the same 220 cases. Public sample = the 10-case smoke set, not used for any training
choice; with n=10, one case is 10 points.

| backend | type acc. (220) | ECE | complexity MAE | p50 / p95 latency | public sample |
|---|---|---|---|---|---|
| regex (default) | 0.364 | 0.212 | 0.67 | ~0 µs | 3/10 |
| `local` lexical LR (CV) | 0.609 | 0.054 | 0.60 | 0.03 / 0.06 ms | 6/10 |
| `http` bge-small + LR (CV) | 0.682 | 0.067 | 0.51 | 7.9 / 13.2 ms (incl. HTTP) | 8/10 |
| `llm` gpt-oss-120b | 0.814 | 0.165 | 0.31 | 779 / 2134 ms | 8/10 |
| *TF-IDF LR (sklearn, CV)* | 0.514 | 0.079 | – | – | – |

Notes on honesty:
* **ECE for the trained rows is after temperature scaling fitted on the same out-of-fold predictions**, so
  it is mildly optimistic. Raw (uncalibrated) ECE was 0.109 (bge) / 0.063 (lexical).
* **The hosted model's confidence is uninformative**: it reports 0.97–0.99 on nearly every answer, right
  or wrong, so `MIN_CONFIDENCE` barely filters it; its ECE (0.165) is the worst of the three ML options
  despite the best accuracy.
* **Low-confidence handling**: for `local` at `MIN_CONFIDENCE=0.7`, 6/10 public cases fell back to regex.
  The CV-recommended threshold for `local` is 0.79 (answers ~23% of cases at ≥90% accuracy); for `http`
  0.73 (~50% coverage at ~91%). The lexical model is the least useful as a *standalone* classifier; its
  value is zero-dependency offline operation.
* Public-sample failures: factual lookups about Rust APIs are read as code understanding (pub-02), and
  multi-step diagnosis prompts with lots of code context are read as code understanding (pub-06).

Costs: `local` $0, 549 KiB, load time not separately measured (JSON parse + golden self-check at startup); `http` CPU only, `bge-small`
33M params, ~3 s cold start for the service, 26 KiB head; `llm` per-decision price not measured (shared
key; roughly 300 prompt + up to 1500 completion tokens per call).

## Not measured / not claimed
* **No downstream routing-quality, cost or token comparison.** Accuracy here does not establish routing
  savings; that requires answer-quality runs on the same workloads, which I did not do.
* Complexity does not influence routing, so complexity quality is reported but unused.
* The private set (ambiguous, multi-intent, paraphrased, OOD) may behave differently from my seeds.
* Hosted-backend fallback rate on timeouts was 0/220 in this run (60 s timeout); not stress-tested.
* No bandit-feedback simulation: complexity is not in the bandit key.

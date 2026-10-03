# P2 — Request classifier for cost-aware routing

A model-agnostic `RequestClassifier` seam for the Level 3 routing decision, with the existing
regex classifier as the default and two opt-in backends (an embedded local model and an
OpenAI-compatible hosted endpoint). Neither opt-in changes default behaviour; the regex path
is byte-for-byte what it was.

## The interface

Added to `src/routing/classifier.rs`:

```rust
pub struct ClassifyInput<'a> { pub query: &'a str, pub context: Option<&'a str> }
pub struct Classification { pub request_type: RequestType, pub complexity: u8, pub confidence: f32 }

#[async_trait]
pub trait RequestClassifier: Send + Sync {
    fn name(&self) -> &str;
    async fn classify(&self, input: &ClassifyInput<'_>) -> Result<Classification, ClassifyError>;
    fn fallback_stats(&self) -> Option<(u64, u64)> { None }  // (fallbacks, calls)
}
```

`Router` integration: `LlmRouterCtx` holds an `Arc<dyn RequestClassifier>`; `route_model`
takes it as a parameter and uses it **at Level 3 only** — the safe routing boundary
(`cold_start`/`switch` in `free_flowing` mode, cache miss). It replaces only the
*request-type* step; tier selection is still `pick_model_thompson` over the provider's learned
cells. Because the default `RegexClassifier` returns exactly `classify_request_type(query)`,
the RNG draw count and order are unchanged and the default is behaviour-preserving.

Boundaries are preserved exactly as before (see `src/routing/boundary.rs`): `continue`
tool-loop turns and `pinned_flow` conversations never classify, so the selected tier stays
sticky mid-loop.

## Backends

| `CLASSIFIER_BACKEND` | Type | Network | Notes |
| --- | --- | --- | --- |
| `regex` *(default)* | `RegexClassifier` | no | Wraps the existing `classify_request_type`. Fixed complexity 3, confidence 0.5. No wrapper, no timeout, no counter — behaviour identical to before the trait existed. |
| `local` | `LocalRequestClassifier` | no | Embedded hashed n-gram + multinomial logistic regression. Deterministic; ~µs per decision; model loaded once at startup. |
| `hosted` | `HostedClassifier` | yes | OpenAI-compatible chat-completions endpoint, `temperature: 0`, strict JSON reply. |

`local` and `hosted` are wrapped in a `ResilientClassifier` that applies a timeout and a
confidence floor, falls back to the regex verdict on any failure, and counts the fallback.

### Configuration (read in `config.rs` via `GatewayConfig::from_env`, never in the library)

| Env var | Default | Meaning |
| --- | --- | --- |
| `CLASSIFIER_BACKEND` | `regex` | `regex` \| `local` \| `hosted` |
| `CLASSIFIER_MODEL_PATH` | *(empty)* | `local`: override the embedded weights; empty ⇒ embedded |
| `CLASSIFIER_ENDPOINT` | *(empty)* | `hosted`: full chat-completions URL |
| `CLASSIFIER_MODEL` | *(empty)* | `hosted`: model id |
| `CLASSIFIER_API_KEY` | *(empty)* | `hosted`: bearer token (never logged; no fallback to platform keys) |
| `CLASSIFIER_TIMEOUT_MS` | `500` | per-decision timeout before falling back |
| `CLASSIFIER_MIN_CONFIDENCE` | `0.35` | below this top-label probability ⇒ regex fallback (counted) |

## Semantics

- **Confidence** is the backend's top-label probability in `[0, 1]`. The local model's
  probabilities are temperature-scaled (T fit on the val split). It is a confidence, not a
  calibrated guarantee; the reported ECE below quantifies the gap.
- **Complexity** is 1–5 per the public rubric. The regex backend has no complexity signal, so
  it reports the neutral midpoint 3 for every input (documented, not a claim).
- **Safe low-confidence behaviour**: a model verdict below `CLASSIFIER_MIN_CONFIDENCE` is
  discarded in favour of the regex verdict and counted as a fallback — never acted on. The
  eval reports the fallback rate. The default `0.35` is deliberately low because the regex
  fallback is itself weak; raising it trades accuracy for conservatism.
- **Fail closed**: a missing/corrupt model, an unbuildable endpoint, an inference error or a
  timeout all degrade to `RegexClassifier` (and increment the fallback counter where the
  resilient wrapper is in play). Routing never fails because the classifier did.

## Complexity rubric (shared with the eval)

1 trivial single operation · 2 straightforward single artifact · 3 multi-step with limited
constraints · 4 substantial reasoning or design · 5 intricate cross-component reasoning.

## Measured results

All numbers were produced by the code in this PR; the scorers recompute from `OUT`.
Comparisons use the same eval files and the same `classifier_eval` binary
(`CLASSIFIER_BACKEND=regex` vs `local`).

Request-type accuracy (higher is better):

| Set | n | regex baseline | local model | Δ |
| --- | --- | --- | --- | --- |
| Public sample | 10 | 0.300 | **0.800** | +0.500 |
| Contributor val | 54 | 0.426 | **0.759** | +0.333 |
| Contributor test (held out) | 63 | 0.476 | **0.762** | +0.286 |
| Train-only 5-fold CV (model selection) | 263 | — | 0.822 | — |

Complexity (local model, vs 1–5 labels): test MAE **0.365**, exact **0.667**, within-1
**0.968**. The regex baseline's complexity MAE is 1.206 on the same set (it always predicts 3).

Calibration: top-1 ECE 0.115 (val) / 0.168 (test) after temperature scaling (T = 0.75).
Fallback rate on the held-out sets: 1/54 (val), 1/63 (test).

Latency (`--release`, Apple Silicon / arm64, model load excluded — loaded once before the
loop; numbers printed to stderr by the example). Per-decision wall time over the held-out
sets:

| Backend | p50 | p95 | max |
| --- | --- | --- | --- |
| local (val, n=54) | 43 µs | 179 µs | 10.1 ms |
| local (test, n=63) | 39 µs | 123 µs | 10.5 ms |
| regex (test, n=63) | 3 µs | 41 µs | 10.3 ms |

The `max` values are per-process scheduling/page-fault outliers on the first case, not
per-decision cost. The regex backend's first call also includes one-time `LazyLock` pattern
compilation (~9 ms); the local model's weights are parsed once at startup, outside the timer.

### Honest caveats

- The contributor val/test sets are self-authored and synthetic. They share the labelling
  convention with the public set but not its exact items; the public-sample column is the
  closest proxy to unseen data.
- The local model is a linear bag-of-ngrams model. It separates the seven classes well when
  the class cue is lexical, and is weakest on genuinely ambiguous or multi-intent requests
  (e.g. a "design …" request phrased with heavy diagnosis language can read as
  `analytical_reasoning`).
- `latency_us` in `OUT` is the only field that varies between runs; every decision field
  (`id`, `request_type`, `complexity`, `confidence`) is byte-identical across runs for the
  regex and local backends.
- Router-level `context` is currently `None`: the inbound transcript is not split into a
  query/context pair at the routing seam. The trait and the eval support context; wiring it
  into the router is deferred (see limits).

## How to run

```sh
# Public sample
curl -fsSL https://registry.nasiko.dev/r/nasiko/classifier-eval -o /tmp/classifier-eval.json

# Baseline
EVAL_SET=/tmp/classifier-eval.json OUT=/tmp/regex.jsonl CLASSIFIER_BACKEND=regex \
  cargo run --release -p nasiko-llm-router --example classifier_eval

# Local model
EVAL_SET=/tmp/classifier-eval.json OUT=/tmp/local.jsonl CLASSIFIER_BACKEND=local \
  cargo run --release -p nasiko-llm-router --example classifier_eval
```

`OUT` receives one JSONL line per case: `{"id","request_type","complexity","confidence","latency_us"}`.
The aggregate summary goes to stderr. `EVAL_SET` may point at any file in the public schema
(the contributor `training/eval_sets/{val,test}.json` follow it).

## Files

| Path | Role |
| --- | --- |
| `src/routing/classifier.rs` | Trait, types, `RegexClassifier`, `ResilientClassifier`. |
| `src/routing/request_classifier.rs` | Local model: features, loader with validation, scoring. |
| `src/routing/classifier_hosted.rs` | Hosted OpenAI-compatible backend. |
| `src/routing/features.rs` | Shared deterministic featurization (also used by the salience gate). |
| `assets/request_classifier_weights.json` | Embedded trained weights (582 KiB). |
| `examples/classifier_eval.rs` | The eval harness (contract above). |
| `training/` | Labelled data, labelling criteria, training pipeline, metrics. |
| `src/config.rs`, `src/lib.rs`, `src/routing/mod.rs`, `src/handlers/chat.rs` | Config + router wiring. |

## Known limits / not production-ready yet

- Router context is always `None` at Level 3; only the eval exercises `context`.
- The hosted backend is unit-tested against a mock endpoint; it has not been run against the
  hackathon egress proxy in this environment.
- Complexity is trained on synthetic labels only; treat it as a soft signal, not ground truth.
- No provider-specific tier remapping change: this PR keeps the existing Thompson bandit and
  its provider registry exactly as-is.
- Local model retraining is manual (`training/train_request_classifier.py`); there is no CI
  job producing the asset.

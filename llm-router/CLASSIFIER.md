# Request classifier (`[classifier]`)

`routing::RequestClassifier` decides the `RequestType` used at Level 3 of `route_model`. The local
model is the default, with the regex classifier as its fallback; nothing needs a network unless you
opt in to `hosted`.

```
classify(query, context?) -> { request_type, complexity 1-5, confidence 0-1 }
```

| env | default | meaning |
|---|---|---|
| `CLASSIFIER_BACKEND` | `local` | `local` \| `regex` \| `hosted` (unknown -> regex) |
| `CLASSIFIER_MODEL_PATH` | embedded | `local`: weights JSON to load instead (load failure -> regex, logged) |
| `CLASSIFIER_ENDPOINT` | - | `hosted`: full chat-completions URL (only this host gets the key) |
| `CLASSIFIER_MODEL` / `CLASSIFIER_API_KEY` | - | `hosted` |
| `CLASSIFIER_TIMEOUT_MS` | 250 | per-decision budget; past it -> regex answer, counted |
| `CLASSIFIER_MIN_CONFIDENCE` | 0 (off) | below it -> `general` (the safe default), counted |
| `ROUTER_TIER_SEED` | unset (entropy) | seeds the Thompson tier draw from `(seed, provider, query, request_type)` |

## Semantics
* **Regex**: fixed `complexity=3`; `confidence=0.5` if a pattern matched else `0.25` (a vote count is not a probability).
* **Local**: hashed word/char n-grams (plus the query's words crossed with the context kind) -> two-head multinomial logistic regression (request type + complexity), weights embedded from `assets/request_classifier_weights.json`. Confidence = temperature-calibrated softmax; complexity comes from the model's own head. Data, labelling and training: [`classifier/README.md`](classifier/README.md).
* **Hosted**: OpenAI-compatible `/chat/completions`, temperature 0, one JSON object back; any invalid field is an error.
* **Complexity**: regex reports a fixed 3, local predicts it with its own head, hosted reports the model's own. Reported with the decision; not yet a routing input.
* **Failure**: backend `Err`/timeout -> regex answer (counted). **Low confidence** -> `general` (counted as fallback, not error).
* **Sticky routing**: the classifier is only called where Level 3 already ran (fireable `cold_start`/`switch`, cache miss). `continue` steps and cache hits never call it. Tier selection is still Thompson sampling keyed on `(provider, tier, request_type)`; complexity does not enter the bandit key. With `ROUTER_TIER_SEED` set the draw is deterministic for identical inputs and learned state.
* **Determinism**: regex and local are pure functions of the input.

## Eval
```
EVAL_SET=llm-router/classifier/data/val.json OUT=/tmp/out.jsonl \
CLASSIFIER_BACKEND=local cargo run --release -p nasiko-llm-router --example classifier_eval
```

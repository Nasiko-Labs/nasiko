# Request classifier (`[classifier]`)

`routing::RequestClassifier` decides the `RequestType` used at Level 3 of `route_model`. The regex
classifier stays the default; nothing needs a network unless you opt in.

```
classify(query, context?) -> { request_type, complexity 1-5, confidence 0-1 }
```

| env | default | meaning |
|---|---|---|
| `CLASSIFIER_BACKEND` | `regex` | `regex` \| `local` \| `hosted` (unknown -> regex) |
| `CLASSIFIER_MODEL_PATH` | embedded | `local`: JSONL training set to train on instead |
| `CLASSIFIER_ENDPOINT` | - | `hosted`: full chat-completions URL (only this host gets the key) |
| `CLASSIFIER_MODEL` / `CLASSIFIER_API_KEY` | - | `hosted` |
| `CLASSIFIER_TIMEOUT_MS` | 250 | per-decision budget; past it -> regex answer, counted |
| `CLASSIFIER_MIN_CONFIDENCE` | 0 (off) | below it -> `general` (the safe default), counted |

## Semantics
* **Regex**: fixed `complexity=3`; `confidence=0.5` if a pattern matched else `0.25` (a vote count is not a probability).
* **Local**: multinomial Naive Bayes (word unigrams/bigrams, 5-char stems, shape tokens, regex votes as features) trained at load from `data/classifier/train.jsonl`. Confidence = temperature-scaled posterior (T chosen by 5-fold CV on train only).
* **Hosted**: OpenAI-compatible `/chat/completions`, temperature 0, one JSON object back; any invalid field is an error.
* **Complexity**: `estimate_complexity`, a deterministic rubric (length, context size, sequencing, constraint/scale words).
* **Failure**: backend `Err`/timeout -> regex answer (counted). **Low confidence** -> `general` (counted as fallback, not error).
* **Sticky routing**: the classifier is only called where Level 3 already ran (fireable `cold_start`/`switch`, cache miss). `continue` steps and cache hits never call it. Tier selection is still Thompson sampling keyed on `(provider, tier, request_type)`; complexity does not enter the bandit key.
* **Determinism**: regex and local are pure functions of the input.

## Eval
```
EVAL_SET=llm-router/data/classifier/val_eval.json OUT=/tmp/out.jsonl \
CLASSIFIER_BACKEND=local cargo run --release -p nasiko-llm-router --example classifier_eval
```

# Calibrated request classification

Nasiko's regex classifier remains the default. Setting `CLASSIFIER_BACKEND=local` selects
an offline, deterministic sparse linear classifier. It consumes separately-prefixed query
and context word unigrams, word bigrams, character 3–5 grams, and structural signals. A
seven-way softmax head predicts request type; four cumulative binary heads independently
predict the ordinal complexity thresholds. Validation-family temperature scaling makes
the published confidence the estimated probability that the selected type is correct.

`GuardedClassifier` validates every result and falls back to the legacy regex classifier
on load/inference failure, timeout, invalid values, or confidence below the configured
floor. The router invokes it only at its existing cold-start/switch Level 3 boundary after
a cache miss. Cached and normal continuation turns remain sticky and do not classify.

| Variable | Default | Meaning |
| --- | --- | --- |
| `CLASSIFIER_BACKEND` | `regex` | `regex` or `local` |
| `CLASSIFIER_MODEL_PATH` | empty | Optional local model override |
| `CLASSIFIER_TIMEOUT_MS` | `50` | Per-decision deadline |
| `CLASSIFIER_MIN_CONFIDENCE` | `0.30` | Regex fallback floor |

The model is trained only from the family-isolated records under `data/classifier`. It is
single-author synthetic data; private-distribution performance and downstream answer cost
or quality are not claimed. See `LABELLING.md` for the rubric and split rules.

```sh
python llm-router/scripts/classifier_train.py
CLASSIFIER_BACKEND=local EVAL_SET=/tmp/classifier-eval.json OUT=/tmp/local.jsonl \
  cargo run --release -p nasiko-llm-router --example classifier_eval
cargo test -p nasiko-llm-router
```

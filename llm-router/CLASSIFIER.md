# P2 request classifier

The router's request classifier produces a request type, a complexity score from
1 to 5, and confidence. It runs only at the existing fireable routing boundary;
pinned routes, cached continuation turns, and non-fireable turns keep their
existing behavior.

## Backend selection

The default backend remains `regex`, preserving existing routing behavior.
Set `CLASSIFIER_BACKEND=local` to opt into the embedded linear model. The local
backend uses deterministic lexical and structural features and does not require
an inference service or network access.

| Environment variable | Default | Meaning |
| --- | --- | --- |
| `CLASSIFIER_BACKEND` | `regex` | `regex` or `local` |
| `CLASSIFIER_MODEL_PATH` | empty | Optional v1 JSON weights override for `local` |
| `CLASSIFIER_TIMEOUT_MS` | `50` | Maximum inference time before regex fallback |
| `CLASSIFIER_MIN_CONFIDENCE` | `0.50` | Lower-confidence predictions use regex |
| `CLASSIFIER_SEED` | `42` | Seed for repeatable tier selection on the same input and learned state |

An unavailable or invalid local model, an inference error, a timeout, an invalid
output, or a prediction below the configured confidence threshold is logged and
falls back to regex. The router also records the selected classification and
fallback count. Complexity is returned by the classifier; existing provider
tier mapping continues to use request type.

## Model and evaluation data

`assets/request_classifier_v1.json` is the versioned, embedded model. The
family-held-out train and validation splits, labeling guide, and provenance are
in [`tests/data`](./tests/data/). The approved team dataset combines curated
examples, hard negatives, and team-reviewed synthetic expansions. The split
keeps each request family in only one partition.

These files are development data, not a blind test set. Synthetic examples
include related task templates, so validation results must not be presented as
proof of generalization to independently authored requests or the private
evaluation set.

## Evaluating

The example uses the same configured classifier factory and `EVAL_SET`/`OUT`
JSONL contract as the harness:

```sh
EVAL_SET=llm-router/tests/data/request-classifier-validation.json \
OUT=/tmp/request-classifier-output.jsonl \
CLASSIFIER_BACKEND=local \
cargo run --release -p nasiko-llm-router --example classifier_eval
```

Set `CLASSIFIER_BACKEND=regex` to produce the baseline predictions. Each output
line contains the example ID, predicted request type, complexity, confidence,
elapsed microseconds, and whether regex fallback occurred. Keep timing and
accuracy comparisons on the same machine and data split.

# Request-classifier training

This directory holds the labelled data and the offline training pipeline for the Level 3
local request classifier. The Rust build never runs Python: the pipeline exports a weights
JSON that is committed under `llm-router/assets/` and embedded in the binary with
`include_str!`.

## Files

| File | Role |
| --- | --- |
| `make_dataset.py` | Authored corpus + labelling criteria + group-disjoint split. Writes `dataset.jsonl` and `eval_sets/{val,test}.json`. |
| `dataset.jsonl` | 387 labelled examples, one JSON object per line, with a `split` field. |
| `eval_sets/{val,test}.json` | The val/test splits in the public eval-file shape, so the Rust example can score them (`EVAL_SET=…`). |
| `train_request_classifier.py` | Featurizes exactly as the Rust engine does, trains and calibrates, exports the asset + `metrics.json`. |
| `metrics.json` | Held-out metrics produced by the last training run. |

## Labelling criteria

The full criteria live at the top of `make_dataset.py`; the short version:

1. **code_generation** — the deliverable is new/changed code or config (write, implement, fix,
   refactor, add tests, edit a line). Explicit constraints win: "do not redesign anything, just
   change TODO to NOTE" is still code_generation, complexity 1.
2. **code_understanding** — explain existing code, config, or a stack trace; no artifact.
3. **technical_design** — a design, architecture, migration plan, schema, or trade-off.
4. **analytical_reasoning** — a derived answer from multi-step reasoning: calculate, estimate,
   prove, diagnose, reconcile, statistical reasoning.
5. **writing** — human-facing prose: draft, rewrite, summarize, tone-edit, announce.
6. **factual_lookup** — a single fact/definition/date/number/name. Live-data requests
   ("weather now") are **general**, because the model cannot know them.
7. **general** — the safe default: greetings, meta/chat, vague requests with no referent,
   requests outside the six categories, and anything genuinely ambiguous.

Complexity 1–5 mirrors the public rubric: 1 trivial · 2 straightforward · 3 multi-step with
limited constraints · 4 substantial reasoning/design · 5 intricate cross-component reasoning.

## Splits and near-duplicate control

Every example belongs to a **group**; all variants of a group (paraphrases and near-duplicates)
share one split, so nothing paraphrased leaks across splits. Groups are assigned
deterministically:

- a small set of **anchor** groups is pinned so every class has at least one learnable,
  class-defining exemplar in train and at least one group held out in val and test;
- the rest are dealt round-robin (2/7 to val+test, 5/7 to train).

`make_dataset.py` asserts that no exact query appears in two splits and that every class is
present in val and test.

## Reproducing the model

```sh
cd llm-router
python3 training/make_dataset.py            # regenerate data (deterministic)
python3 training/train_request_classifier.py # retrain + export assets/…weights.json + metrics.json
```

Requires Python 3 with `numpy`, `scipy`, `scikit-learn` (the runs in this PR used
numpy 2.5.0 / scikit-learn 1.9.0). `trained_at_utc` is pinned so re-running with the same
library versions produces the same bytes.

## Feature compatibility is a contract

`train_request_classifier.py` reimplements the Rust feature engine
(`src/routing/features.rs` + `src/routing/request_classifier.rs`) byte-for-byte: FNV-1a
hashing, word 1–2 grams, char 3–5 grams, the same six dense features, and the same
query/context weighting. Any change to those constants invalidates the weights, so the Rust
loader validates `num_buckets`, `num_dense_features`, the n-gram ranges and `context_weight`
against the compiled-in constants and **refuses** a mismatched file rather than scoring with
the wrong mapping.

## Model and results

Multinomial logistic regression (7-way) + ridge complexity regressor over 4096 hashed buckets
+ 6 dense features; temperature-scaled on the val split. Model selection is 5-fold CV on the
train split only — the val split stays clean for calibration and the test split is never
consulted during development.

See `metrics.json` and the "Measured results" section of `../REQUEST_CLASSIFIER.md` for the
held-out numbers and the regex comparison.

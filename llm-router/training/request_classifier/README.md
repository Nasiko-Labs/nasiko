# Request classifier: data and training

This directory produces `llm-router/assets/request_classifier.json`, the weights for the `local` request-classifier backend (`CLASSIFIER_BACKEND=local`). It is offline tooling only. Nothing here is part of the cargo build.

| File | What |
|---|---|
| [`LABELING.md`](LABELING.md) | Labelling criteria: type definitions, the complexity rubric, tie-breaks, worked examples |
| [`GENERATION.md`](GENERATION.md) | How the synthetic data was written, and the batch prompts |
| [`DATASHEET.md`](DATASHEET.md) | Counts, composition, process, labelling quality, known biases |
| `data/base/*.jsonl` | Authored scenarios (compact keys) |
| `data/labelled.jsonl` | The full dataset, with splits |
| `data/eval_val.json`, `data/eval_test.json` | Held-out splits in the public eval schema, so `classifier_eval` runs on them unchanged |
| `rc.py` | The pipeline: `build`, `split`, `train`, `eval`, `export`, `report`, `kappa` |
| `reports/` | Measured results: the test report, the val report, the public-10 smoke test, and the first one-shot test run |

## Design
- **Rust is the only feature engine.** `examples/dump_classifier_features.rs` writes the exact vectors `routing::request_features::extract` computes, and Python only fits weights on them. A Rust test asserts that the dump equals the inference features.
- **Model:**
  - The type head is a 7-way multinomial logistic regression, with temperature scaling for calibrated confidence.
  - The complexity head is Frank–Hall ordinal: four binary "complexity > k" logistic regressions, decoded monotonically.
- **Selection without leakage:**
  - The type head's C, the complexity heads' C and the temperature T are chosen by **group 5-fold cross-validation on train only**, using out-of-fold predictions, with all variants of a scenario in one fold.
  - Val is never used for selection.
  - Test was evaluated once before the calibration fix; the rerun is labelled post-hoc (see "Results").
- **Export:**
  - One row per hashed bucket (7 type weights + 4 complexity weights), pruned by max |w| and rounded to 5 significant digits.
  - The file is capped at about 1.9 MB.
  - Provenance is embedded: dataset sha256, git sha, C values, regex constants.

## Reproduce
```bash
python3 -m venv .venv && .venv/bin/pip install -r llm-router/training/request_classifier/requirements.txt
R=llm-router/training/request_classifier
curl -fsSL https://registry.nasiko.dev/r/nasiko/classifier-eval -o /tmp/classifier-eval.json   # for the public near-dup guard
.venv/bin/python $R/rc.py build
.venv/bin/python $R/rc.py split --public /tmp/classifier-eval.json
DATASET=$R/data/labelled.jsonl DUMP_OUT=$R/out/features.jsonl \
  cargo run --release -p nasiko-llm-router --example dump_classifier_features
.venv/bin/python $R/rc.py train          # ~5 min on a laptop CPU (grid × 5 folds)
.venv/bin/python $R/rc.py eval           # val report -> out/val_report.md
SOURCE_DATE_EPOCH=1790000000 .venv/bin/python $R/rc.py export   # fixed timestamp => byte-identical artifact
```
Running the whole chain twice gives the same artifact sha256: everything is seeded (13) and the trained-at time comes from `SOURCE_DATE_EPOCH`. `export` also prints the regex confidence constants to copy into `routing/classifier.rs` (`REGEX_CONFIDENCE_*`).

To evaluate with the official harness on our splits:
```bash
EVAL_SET=$R/data/eval_test.json OUT=/tmp/test-local.jsonl CLASSIFIER_BACKEND=local \
  cargo run --release -p nasiko-llm-router --example classifier_eval
EVAL_SET=$R/data/eval_test.json OUT=/tmp/test-regex.jsonl CLASSIFIER_BACKEND=regex \
  cargo run --release -p nasiko-llm-router --example classifier_eval
.venv/bin/python $R/rc.py report --eval $R/data/eval_test.json --out /tmp/test-local.jsonl --out-b /tmp/test-regex.jsonl
```

## Results
Measured on an Apple M4 (10 cores), with a release build.

| System | Split | Type acc | Macro-F1 | ECE (10) | Cx exact / ±1 / MAE | p50 / p95 µs |
|---|---|---|---|---|---|---|
| regex | test (219) | 0.224 | 0.216 | 0.062 | 0.228 / 0.699 / 1.073 | 5 / 21 |
| local v1 (T from val), **one-shot** | test (219) | 0.763 | 0.769 | 0.117 | 0.685 / 0.986 / 0.329 | 103 / 236 |
| **local v2 (CV T), shipped**, post-hoc rerun | test (219) | **0.758** | 0.764 | **0.072** | 0.685 / 0.986 / 0.329 | 103 / 246 |
| local v2 | val (213, optimistic) | 0.854 | 0.855 | 0.077 | 0.690 / 0.977 / 0.333 | 104 / 197 |
| local v2 | train out-of-fold (CV) | 0.778 | — | 0.039 | — | — |
| regex / local v2 | public 10 (smoke test, not evidence) | 0.3 / 1.0 | — | — | 0.2 / 0.5 exact | — |

Notes:
- **v1 vs v2.** v1 fitted its temperature on val, which had already informed one error-analysis round. On test it was overconfident (135 items averaged 0.978 confidence at 0.881 accuracy). v2 picks C and T by cross-validation on train only. We report both because v2 was evaluated on test after we had seen v1's test result.
- **Slices where local is weak on test:** boundary 0.43 (regex 0.57, the one slice where regex wins), negation 0.33, non_english 0.43, trap 0.50, misleading_keyword 0.57. Details are in `reports/test_report.md`.
- **Recommended `CLASSIFIER_MIN_CONFIDENCE` is 0.6**, chosen on train out-of-fold predictions: 0.871 accuracy at 74% coverage. On test, τ = 0.6 gives 0.827 at 79% coverage. The router default stays 0.0 (off).
- **Artifact and cost:**
  - The artifact is 1.40 MB embedded (12,643 buckets), loads in about 27 ms including the warm-up, and costs $0 per decision on CPU.
  - The 1.9 MB cap forced pruning at |w| < 0.29. That cost about one val item (0.5 points) against the unpruned model.

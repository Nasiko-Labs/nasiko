# Request classifier — data, training, evaluation

The router picks a model tier per request from its **request type** (see
`src/routing/classifier.rs`). This directory holds everything behind the `local` backend: the labelled
data, the labelling criteria, and the scripts that train and score the model embedded at
`assets/request_classifier_weights.json`.

## Backends

Selected with `CLASSIFIER_BACKEND` (read in `src/config.rs`); every backend runs behind
`FallbackClassifier`, so an error or timeout falls back to the regex result and is counted.

| Backend | What it is | Network | Notes |
|---|---|---|---|
| `regex` (default) | The original keyword vote count | No | Fixed complexity 3, confidence 0.5 |
| `local` | Hashed word/char n-grams (plus the query's words crossed with the context kind: none, code, prose) → two-head multinomial logistic regression (type + complexity), temperature-calibrated | No | Weights embedded in the binary; override with `CLASSIFIER_MODEL_PATH` |
| `hosted` | Any OpenAI-compatible chat-completions endpoint returning JSON | Yes | `CLASSIFIER_ENDPOINT`, `CLASSIFIER_API_KEY`, `CLASSIFIER_MODEL`; self-reported confidence |

Other settings: `CLASSIFIER_TIMEOUT_MS` (default 1000), `CLASSIFIER_MIN_CONFIDENCE` (default 0.5; below
it the agent's configured model is served and nothing is pinned; never applied to regex),
`ROUTER_TIER_SEED` (deterministic Thompson tier sampling).

## Data

- `data/train.json` (3,212) and `data/val.json` (439), eval-set schema, so `classifier_eval` runs on
  them directly. Every split covers the slices the brief names: paraphrase, ambiguous, multi-intent,
  noisy, padded, out-of-distribution, regex near-miss, with_context, negation.
- Synthetic requests, authored with LLM assistance (Claude) for this classifier and labelled per
  [`LABELLING.md`](LABELLING.md); no user data. The committed splits are the output of
  `build_splits.py`; the raw authoring files are not included. Two batches:
  1. 2,943 requests written one label at a time (2,591 train / 352 validation);
  2. 716 "new-style" requests (708 kept after exact-duplicate removal) written to imitate many different writers (terse, rambling, non-native
     English, voice-to-text, pasted tickets and email chains, vague queries over evidence that is only in
     the context), added with `build_splits.py --extend` so batch 1's splits and ids stay unchanged
     (621 train / 87 validation; 8 near-duplicate groups merged).
- Grouped split: paraphrase families and near-duplicates (char 3–5-gram TF-IDF cosine ≥ 0.85) never span
  train and validation; the closest cross-split pair is 0.77.
- The published eval sample is excluded: `build_splits.py` drops anything within cosine 0.45 of a public
  case and aborts the build if any survives. The closest example to any public case is 0.32; two
  generated examples that read like public cases (0.52 and 0.29) were removed by hand.
- Label audits (independent annotator, blind to the existing labels): 280 stratified examples from
  batch 1 agreed on 99.3% of request types; 140 from batch 2 agreed on 100% (8 flagged as genuinely
  ambiguous). Complexity agreed exactly on 69% / 70% and within one point on 99% / 99%.

## Training the `local` model

The feature engine exists only in Rust (`src/routing/classifier/linear.rs`); training consumes its
output, so training and inference cannot drift apart.

```sh
# 1. splits (needs the public sample only to exclude it); --extend keeps an existing split and ids
python scripts/build_splits.py --raw <dir of *.jsonl> --public classifier-eval.json --out data \
  [--extend <previous data dir>]

# 2. features from the router's own feature engine
DATASET=llm-router/classifier/data/train.json OUT=/tmp/feat/train.features.jsonl \
  cargo run --release -p nasiko-llm-router --example classifier_features
DATASET=llm-router/classifier/data/val.json OUT=/tmp/feat/val.features.jsonl \
  cargo run --release -p nasiko-llm-router --example classifier_features

# 3. pack the features, then run scripts/train_linear_gpu.ipynb (this produced the shipped weights)
python scripts/make_features_npz.py --data data --features /tmp/feat --out linear_features.npz
```

`train_linear_gpu.ipynb` was run on Kaggle (GPU T4 x2: upload `linear_features.npz` as a dataset and
attach it as input); it also runs on CPU (slower) with `linear_features.npz` in the working directory.
It writes `request_classifier_weights.json`, which replaces `../assets/request_classifier_weights.json`.

Model selection uses only `train`: C by grouped 5-fold CV (the chosen type-head C, 256, is the top of
the searched grid), temperature on out-of-fold predictions, weight pruning by ≥ 99.5% agreement with
the unpruned model on train. `val` is only reported. The
weights file records C, temperature, pruning thresholds, the training-data SHA-256 and validation
metrics in `provenance`.

## Evaluating

```sh
CLASSIFIER_BACKEND=local EVAL_SET=llm-router/classifier/data/val.json OUT=/tmp/local.jsonl \
  cargo run --release -p nasiko-llm-router --example classifier_eval
python llm-router/classifier/scripts/score.py --gold llm-router/classifier/data/val.json \
  --pred /tmp/local.jsonl --min-confidence 0.5
```

## Results

CPU latency is per decision, batch 1, single thread, excluding the one-time load.

| | regex | **local** |
|---|---|---|
| Validation accuracy (439) | 20.5% | **88.6%** |
| Validation ECE | 0.295 | **0.022** |
| Confident errors (conf ≥ 0.5) | 349 | **40** (5.5% below 0.5 → safe default) |
| Accuracy when answered (conf ≥ 0.5) | 20.5% | **90.4%** |
| Public sample (10, not trained on) | 3/10 | **10/10** |
| Complexity exact / MAE (val) | 21.4% / 1.19 | 54.2% / 0.55 |
| p50 / p95 latency | 3 µs / 19 µs | 0.25 ms / 1.2 ms |
| Model size / load time | — / 21 ms | 2.9 MB on disk (~13 MB in memory) / ~55 ms |
| Cost per decision | 0 | 0 (in-process CPU) |

By validation subset (local, with the previous model for reference):

| Subset | n | previous model | **this model** |
|---|---|---|---|
| Original validation | 352 | 89.8% (ECE 0.038, 30 confident errors) | **90.6%** (ECE 0.032, 24) |
| New-style validation | 87 | 75.0% (84-example subset, ECE 0.154) | **80.5%** (ECE 0.064, 16) |

Validation accuracy by slice (local, 439): clean 93%, padded 92%, with_context 88%, regex near-miss
87%, paraphrase 87%, negation 86%, noisy 85%, multi-intent 83%, OOD 83%, ambiguous 75%. Recall by label:
general 93%, technical_design 91%, writing 91%, code_understanding 88%, factual_lookup 88%,
analytical_reasoning 87%, code_generation 81%.

Known limits:
- The data is synthetic, so validation accuracy is likely optimistic for real traffic. The new-style
  batch was written to expose that gap: the previous model scored 71.5% on it before it was trained on
  (611 requests it had never seen), against 89.8% on its own-style validation set. The 10-case public
  sample is the only independent check here, and it is too small to settle anything.
- Ambiguous requests are the weakest slice (75%). Non-Latin scripts are a real gap: "日本の首都はどこですか？"
  is answered `general` at 0.98 confidence, because few such n-grams were seen in training.
- Complexity got worse than the previous model (exact 62% → 55% on the original validation set, MAE
  0.46 → 0.53) while request-type accuracy and calibration improved; complexity is reported but does not
  yet drive routing.
- The router does not yet pass conversation context to the classifier (the eval does), and complexity
  is not yet a bandit input.

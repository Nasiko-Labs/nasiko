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
| `local` | Hashed word/char n-grams → two-head multinomial logistic regression (type + complexity), temperature-calibrated | No | Weights embedded in the binary; override with `CLASSIFIER_MODEL_PATH` |
| `hosted` | Any OpenAI-compatible chat-completions endpoint returning JSON | Yes | `CLASSIFIER_ENDPOINT`, `CLASSIFIER_API_KEY`, `CLASSIFIER_MODEL`; self-reported confidence |

Other settings: `CLASSIFIER_TIMEOUT_MS` (default 1000), `CLASSIFIER_MIN_CONFIDENCE` (default 0.5; below
it the agent's configured model is served and nothing is pinned; never applied to regex),
`ROUTER_TIER_SEED` (deterministic Thompson tier sampling).

## Data

- `data/train.json` (2,591) and `data/val.json` (352), eval-set schema, so `classifier_eval` runs on
  them directly.
- Synthetic requests, authored with LLM assistance (Claude) for this classifier, one label at a time,
  and labelled per [`LABELLING.md`](LABELLING.md); no user data. The committed splits are the output of
  `build_splits.py`; the raw authoring files are not included. Every split covers the slices the brief names: paraphrase, ambiguous, multi-intent, noisy,
  padded, out-of-distribution, regex near-miss, with_context, negation.
- Grouped split: paraphrase families and near-duplicates (char 3–5-gram TF-IDF cosine ≥ 0.85) never span
  train and validation; the closest cross-split pair is 0.51.
- The published eval sample is excluded: `build_splits.py` drops anything within cosine 0.6 of a public
  case and aborts the build if any survives (closest example to any public case: 0.32).
- Label audit: an independent annotator blind-labelled a stratified sample of 280 examples and agreed
  on 99.3% of request types (both disagreements flagged as genuinely ambiguous); complexity agreed
  exactly on 68.6% and within one point on 99.3%.

## Training the `local` model

The feature engine exists only in Rust (`src/routing/classifier/linear.rs`); training consumes its
output, so training and inference cannot drift apart.

```sh
# 1. splits (needs the public sample only to exclude it)
python scripts/build_splits.py --raw <dir of *.jsonl> --public classifier-eval.json --out data

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

Model selection uses only `train`: C by grouped 5-fold CV, temperature on out-of-fold predictions,
weight pruning by ≥ 99.5% agreement with the unpruned model on train. `val` is only reported. The
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
| Validation accuracy (352) | 21.3% | **89.8%** |
| Validation ECE | 0.287 | **0.038** |
| Confident errors (conf ≥ 0.5) | 277 | **30** (4.0% below 0.5 → safe default) |
| Public sample (10, not trained on) | 3/10 | **9/10** |
| Complexity exact / MAE (val) | 22.2% / 1.19 | 61.9% / 0.46 |
| p50 / p95 latency | 4 µs / 32 µs | 0.21 ms / 0.96 ms |
| Model size / load time | — / 16 ms | 2.5 MB on disk (~13 MB in memory) / 44 ms |
| Cost per decision | 0 | 0 (in-process CPU) |

Validation accuracy by slice (local): ambiguous 76%, paraphrase 86%, OOD 85%, noisy 88%, padded 87%,
regex near-miss 90%, with_context 90%, multi-intent 93%.

Known limits:
- The data is synthetic and was written per label, so validation accuracy is likely optimistic for real
  traffic; the public sample is the only independent check here.
- Ambiguous requests and "explain this code" with the code only in the context are the weakest cases
  (the public miss is pub-03: `code_understanding` predicted as `code_generation`, confidence 0.74).
- The router does not yet pass conversation context to the classifier (the eval does), and complexity
  is reported but not yet a bandit input.

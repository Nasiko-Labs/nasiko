# P2 request-classifier datasets

Labelled data for the request classifier (`src/routing/request_model.rs`). Both files are
**JSONL**, one JSON object per line:

```json
{"id":"cg-01","query":"Write a Python function that reverses a string.","request_type":"code_generation","complexity":2}
```

`request_type` is one of the router's `RequestType` values, `complexity` is 1–5. An optional
`"context"` string (pasted code, prior-turn detail) is used by the model as additional
features when present.

## Files

| File | Role |
|---|---|
| `train.jsonl` | Fitting set (7 classes × 34 = 238 cases). |
| `val.jsonl` | **Held-out** validation set (7 classes × 9 = 63 cases), authored after `train.jsonl` and never fitted on. |
| `../../scripts/train_request_classifier.py` | Offline trainer; emits the weights JSON embedded as `assets/classifier_weights.json`. |

## Labelling criteria

Label by **what artifact the user wants produced**, not by surface keywords. The rubric is
deliberately the same taxonomy the router already persists in `router_quality_cells`, so a
learned cell keeps its meaning across backends.

| `request_type` | The user wants… | Typical signals |
|---|---|---|
| `code_generation` | New/changed **code**: a function, script, query, config, migration, test, patch | write/implement/fix/add/refactor/convert/rename, language or file names |
| `code_understanding` | An **explanation of existing code or behaviour**; no code artifact requested | explain/why/what does…do/trace/interpret this snippet |
| `technical_design` | A **design decision**: architecture, schema, trade-offs, a plan; no code expected | design/should we/architecture/trade-offs/approach |
| `analytical_reasoning` | A **derived answer**: calculation, proof, counting, diagnosis, inference from data | calculate/how many/prove/estimate/which explains |
| `writing` | **Prose for humans**: docs, emails, posts, summaries, release notes | write/draft/compose/rephrase/shorten/turn…into a…post |
| `factual_lookup` | A **single short fact** with a known answer | who/what/when/where…(is/was), define X |
| `general` | **Small talk / meta**: greetings, acknowledgements, "can you repeat that", nothing actionable | hi/thanks/cheers/testing |

### Complexity rubric (1–5)

1. trivial single operation · 2. straightforward · 3. multi-step with limited constraints ·
4. substantial reasoning or design · 5. intricate cross-component reasoning and validation.

### Tie-breaks (multi-intent queries)

The held-out set deliberately contains ambiguous, multi-intent and noisy cases so the metric
does not reward keyword memorisation. When a query could carry two labels:

1. The **last artifact-changing action** the user asks for wins (e.g. "summarise this function
   and then fix the bug" → `code_generation`, because the fix is the artifact).
2. Otherwise the **dominant noun phrase** decides.
3. Otherwise the earliest label in the router's precedence order wins:
   `code_generation` › `code_understanding` › `technical_design` › `analytical_reasoning` ›
   `writing` › `factual_lookup` › `general`.

## Split policy (no near-duplicate leakage)

- `val.jsonl` was authored **after** `train.jsonl`, independently, with different phrasings and
  different artifacts.
- A candidate validation case was dropped or reworded if it shared a **prefix of ≥3 content
  words in the same order** with any training case, or if it was a paraphrase differing only by
  punctuation/synonyms.
- Validation also intentionally carries **out-of-distribution and noisy** cases (shop talk,
  a typo-free "Polish this paragraph", "Figure out how many shards…") that the regex baseline
  is expected to get wrong.

## Retraining

The feature engine is a compatibility contract: **changing `src/routing/text_features.rs`, the
feature extraction in `src/routing/request_model.rs`, or the dense standardization invalidates
the committed weights.** Refit and re-embed alongside any such change.

```sh
python3 scripts/train_request_classifier.py \
  --train data/classifier/train.jsonl \
  --val   data/classifier/val.jsonl \
  --out   assets/classifier_weights.json
```

Defaults are `--epochs 600 --lr 0.6 --l2 1e-3` (selected on the fit/dev split); omit `--val`
to fall back to a stratified 80/20 split of `--train`. The weights are **schema v2**, which
carries the fit-set dense-feature `dense_mean`/`dense_std` so the unbounded log-length dense
features are z-scored before the dot product — without this the model degenerates into a
length classifier. `RequestModel::from_wire` rejects any other version rather than silently
mismatching.

Then run `cargo test -p nasiko-llm-router` (the embedded-model tests load the new asset) and
`cargo run --release -p nasiko-llm-router --example classifier_eval` to score it. The weights
file carries a `provenance` block (timestamp, counts, seed, accuracy notes) that the model logs
at load time; the trainer writes it, nothing edits it by hand.
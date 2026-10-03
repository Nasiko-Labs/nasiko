# Labelled data and labelling criteria

`build_dataset.py` holds ~220 hand-written **seed** queries (`SEEDS`) and deterministically expands them
(`random.Random(1234)`) into `data/train.jsonl` and `data/val.jsonl`. `data/seeds_eval.json` is the same
seeds in the public eval format (one case per seed, no augmentation).

Row: `{"seed", "query", "context", "request_type", "complexity"}`. No private prompts or user data;
everything is synthetic and written for this task.

## `request_type` — label by the work the user wants done, not by keywords

| label | the deliverable is… | examples |
|---|---|---|
| `code_generation` | new or changed code, tests, scripts, configs, regexes | "Write a retry decorator", "fix the bug where…" |
| `code_understanding` | an explanation, trace, or diagnosis of *existing* code | "Why does this re-render?", "What does this one-liner do?" |
| `technical_design` | an architecture, schema, API, rollout or technology choice | "Design idempotent webhook handling" |
| `analytical_reasoning` | diagnosis from evidence, estimation, maths, trade-off judgement | "Why did p99 double?", "17 × 23" |
| `writing` | prose: emails, summaries, rewrites, docs, commit messages | "Rewrite this error message…" |
| `factual_lookup` | a short fact, definition or well-known comparison | "What port does Postgres use?" |
| `general` | chit-chat, vague or meta requests, advice outside the above | "ok", "Plan a weekend in Lisbon" |

Tie-breaks used while labelling:
* **Multi-intent → primary deliverable.** "Explain the bug, then give the corrected version" is
  `code_understanding` (the explanation leads); "Summarize this function and write tests" is
  `code_generation`.
* **Keyword near-misses follow intent.** "Rewrite the architecture overview" is `writing`;
  "Define a Rust struct…" is `code_generation`; "What is causing the memory growth?" is
  `analytical_reasoning`; "Draft a design doc outline" is `technical_design`.
* **Numbers/maths with no code → `analytical_reasoning`**, even when trivial.
* **Padding and typos never change the label.**

## `complexity` (1–5) — the public rubric

1 trivial single operation · 2 straightforward · 3 multi-step with limited constraints ·
4 substantial reasoning or design · 5 intricate cross-component reasoning and validation.

## Coverage of hard cases
~30% of seeds are deliberately adversarial for the regex baseline: keyword present but different intent,
multi-intent, noisy/padded text ("pls help!!! my code no work"), and out-of-distribution requests.

## Leakage control
* Splits are **grouped by seed**: a seed and all of its variants are always in the same split.
* Validation rows are un-augmented seeds. Cross-validation (`train.py`, `train_lexical.py`) uses
  `GroupKFold` by seed and scores **only original seeds**, never augmented copies.
* No public-sample query is a seed: the 10 public cases were removed from the seed list after an early run
  showed they inflated the score (9/10 → 6/10 once removed). The public sample is therefore a clean check.

## Augmentation (train only)
Per seed: a lower-cased/punctuation-stripped copy, a filler prefix ("hey, ", "um so "), appended
context padding, and a one-word transposition typo.

## Known limits
* Labels are single-author; I have not measured inter-annotator agreement. Several seeds are genuinely
  ambiguous and a different reviewer might choose differently.
* ~220 seeds is small. Cross-validation numbers carry roughly ±3 points of noise and should be read as
  rough ordering, not precise estimates.
* The hosted-LLM prompt uses the same category definitions as this file, so its score on these seeds is
  likely optimistic relative to an unseen labeller's data.

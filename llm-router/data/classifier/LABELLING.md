# Classifier training data: labelling criteria

`labelled.txt` is `request_type|complexity|query`, one hand-written query per line. `build_splits.py`
turns it into `train.jsonl` (embedded in the binary), `val.jsonl` (held out, never used for tuning)
and `val_eval.json` (same schema as the public `classifier-eval@v1` file).

## request_type (decide by what the user wants *produced*)

| label | the answer is mainly... |
|---|---|
| `code_generation` | new or modified code, scripts, queries, regexes, config/YAML |
| `code_understanding` | an explanation, review, trace or debugging of code/errors the user already has |
| `technical_design` | architecture, schema, tooling or technology trade-offs, plans |
| `analytical_reasoning` | maths, estimation, business/data analysis or logic, no code needed |
| `writing` | prose drafted, rewritten, summarised or translated |
| `factual_lookup` | one short fact |
| `general` | chit-chat, advice, brainstorming, anything else |

Tie-break: the *final deliverable* wins ("explain this code and rewrite it" -> `code_generation`).

## complexity rubric (1-5)

1 trivial one-liner / single fact; 2 short single-step; 3 typical multi-sentence task;
4 several requirements or needs domain depth; 5 many interacting constraints, multi-step or
scale/robustness requirements.

## Splits

Near-duplicates (word-set Jaccard >= 0.6) are dropped before splitting. The split is stratified
per class (every 4th row in FNV-1a hash order -> validation), with no RNG, so it is reproducible.
Regenerate: `cd llm-router/data/classifier && python3 build_splits.py`.

## Known weaknesses of this data

148 rows, one author, mostly clean English. It has no noisy/padded, multi-intent or
out-of-distribution rows, which is exactly what a private set will contain. Treat the held-out
numbers as a smoke signal, not as evidence of generalisation.

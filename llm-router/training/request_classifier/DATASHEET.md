# Datasheet: request-classifier dataset (`data/labelled.jsonl`)

## Summary

| | |
|---|---|
| Items | **1,695**: 682 authored scenarios plus label-preserving variants |
| Splits | train 1,263 · val 213 · test 219. Split by scenario group (`group_id`), so all variants of a scenario land in one split |
| Labels | `request_type` (7 router types) and `complexity` (1–5), following [`LABELING.md`](LABELING.md) |
| Source | Synthetic. Written by the team's LLM assistant (Claude) following `LABELING.md`; see [`GENERATION.md`](GENERATION.md). No real user data, no secrets |
| Public eval set | **Never trained on.** Items with char-5-gram Jaccard ≥ 0.5 to any public query are dropped by `rc.py split` (0 found). Items the author judged structurally close to a public case were rewritten |
| sha256 of `labelled.jsonl` | recorded in the weights file's `provenance.dataset_sha256` |

## Composition (all splits)

**Request type:**

| Type | Items |
|---|---|
| code_generation | 340 |
| analytical_reasoning | 250 |
| writing | 245 |
| factual_lookup | 240 |
| technical_design | 226 |
| code_understanding | 202 |
| general | 192 |

**Complexity:**

| Complexity | 1 | 2 | 3 | 4 | 5 |
|---|---|---|---|---|---|
| Items | 375 | 598 | 439 | 203 | 80 |

Level 5 makes up 4.7% of items, which is below the 8% we aimed for. It is a known gap.

**Slice:**

| Slice | Items |
|---|---|
| clear | 1,040 |
| boundary (near-misses between types) | 310 |
| trap (negation, misleading keywords) | 135 |
| multi_intent | 88 |
| ood (out of domain) | 62 |
| noisy | 60 |

**Tags.** An item can carry several tags.

| Tag | Items | Note |
|---|---|---|
| padded | 431 | variant |
| noisy | 369 | variant |
| very_short | 224 | |
| placeholder_context | 214 | variant |
| non_english | 154 | Spanish, French, German, Portuguese, Italian, Hindi/Hinglish, Japanese, Chinese |
| misleading_keyword | 134 | |
| multi_intent | 88 | |
| negation | 62 | |

**Context:**

| Context | Items |
|---|---|
| Attached material (code, logs, notes, constraints) | 637 (38%) |
| Placeholder ("No codebase context.") | 218 (13%) |
| None | 840 (50%) |

## Process

1. **Authoring.** Scenarios are hand-written per type, slice and complexity in `data/base/*.jsonl` (compact keys: `t`, `c`, `s`, `tags`, `q`, `x`).
2. **Variants.** `rc.py build` adds 1–2 deterministic, label-preserving variants per scenario. Each variant shares its scenario's `group_id`.
   - *padded:* a greeting and a sign-off are added.
   - *noisy:* at least one typo is guaranteed; lowercasing, dropped final punctuation and filler words are applied probabilistically.
   - *placeholder:* a placeholder context is used where the item had none.
3. **Split.** `rc.py split` runs these steps:
   1. Exact dedup on normalised text (9 items dropped).
   2. Group split 70/15/15, stratified by type, with seed 13.
   3. A cross-split near-dup guard: char-5-gram Jaccard ≥ 0.6 moves the val/test group into train (1 item moved). The step then asserts that no near-dup remains.
4. **Error-analysis round.** One round was allowed, on val only. `data/base/iteration1_train_only.jsonl` holds 100 new scenarios targeting the confusions seen on val:
   - "write" + prose deliverables vs code
   - small code edits vs explanations
   - maths word problems
   - "right way to model X" design questions
   - non-English coverage

   They are **train-only**, built last under an `it-` id prefix, and excluded from the split shuffle. Val and test were verified byte-identical before and after.

## Labelling quality

- **Single annotator.** The authoring model labelled every item against `LABELING.md`, then re-read the boundary, trap and multi-intent items against the tie-break rules.
- **Human adjudication is not done yet.** Every item has `adjudicated: false`. A human pass over the `boundary`, `trap` and `multi_intent` slices is recommended before relying on fine-grained numbers.
- **Inter-annotator agreement (κ) has not been measured yet.** `rc.py kappa --a A.jsonl --b B.jsonl` computes Cohen's κ for type and quadratic-weighted κ for complexity once a second, independent labelling of a 100-item subset is available.

## Known biases and limits

- Synthetic, one author, and English-centric. About 9% of items are non-English, and those are mostly short.
- Complexity 5 is under-represented. The distribution of attached context differs from production traffic, where router-side context is recent conversation turns.
- Variants inflate the item count, but not the diversity of scenarios. For an honest picture, read the metrics by slice as well as overall.
- Val was used for one error-analysis round, so **val metrics are optimistic. Test is the honest estimate.**

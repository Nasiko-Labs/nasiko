# Labelled data

Files: `llm-router/tests/data/classifier/{dev,calibration,heldout}.json` plus
`split-manifest.json`. Same schema as the public sample (`examples` array; each case has
`id`, `query`, `context`, `request_type`, `complexity`, `tests`) plus a `family` field.

## Provenance (read this before trusting the labels)

- **Authored for this contribution**, synthetic, no private user data.
- **Model-authored**: the contributor's AI assistant (Claude) wrote queries, contexts and
  labels; the same author reviewed each case once against the criteria below. There was
  no second human adjudicator, so these are *reviewed synthetic labels*, not independently
  validated ground truth. Disagreement on genuinely mixed cases should be expected.
- The public ten-case sample (`pub-*`) is smoke data only and is not in any split.

## Counts (actual)

| split | cases | code_gen | code_und | design | analysis | writing | factual | general | cx 1/2/3/4/5 |
|---|---|---|---|---|---|---|---|---|---|
| dev | 48 | 10 | 7 | 6 | 7 | 6 | 7 | 5 | 18/14/6/5/5 |
| calibration | 24 | 5 | 3 | 2 | 4 | 4 | 3 | 3 | 9/7/3/4/1 |
| heldout | 48 | 9 | 6 | 6 | 5 | 8 | 8 | 6 | 22/10/6/5/5 |

Deliberately small: every case was written and reviewed individually rather than
templated. Level 1 is over-represented because real traffic is, and because keyword traps
and padding are easiest to build on trivial tasks.

## Label criteria

**Request type — dominant intent, i.e. the output the user will actually use.**

| label | criterion | boundary notes |
|---|---|---|
| `code_generation` | produce or change code | any edit counts, even one character; "explain then fix" ⇒ code_generation |
| `code_understanding` | explain/trace/review supplied or referenced code without changing it | "review, don't rewrite" ⇒ code_understanding |
| `technical_design` | propose how a system/API/schema/migration/rollout should be structured | includes trade-off discussions; "design then implement X" ⇒ code_generation if the code is the deliverable |
| `analytical_reasoning` | work out a conclusion: diagnose, calculate, prove, estimate, weigh evidence | includes trivial arithmetic; sentiment/judgement of quoted text is analysis |
| `writing` | produce or rewrite prose for people to read | includes prose *about* code/flags/systems; a judgement about prose quality is writing |
| `factual_lookup` | short settled fact/definition needing no supplied code and no working-out | "what does `?` do in Rust" with no code is factual, not code_understanding |
| `general` | conversation, meta, acknowledgements, or nothing else fits | "make it faster" with no context is general (unresolvable), not code |

**Complexity — the work, not the wording.** 1 trivial single operation; 2 one
well-defined task, known approach, no competing constraints; 3 several coordinated steps
or a couple of constraints within one component; 4 open-ended trade-offs, several
interacting constraints or non-obvious diagnosis; 5 many interacting parts, contradictory
requirements, correctness must be argued and verified. Padding never raises the level;
terseness never lowers it.

## Phenomena covered (tags in `tests`)

`ambiguity`, `mixed_intent`, `paraphrase`, `unseen_topic`, `negation`, `keyword_trap`,
`context_dependent`, `noisy_padded`, `concise_hard`, `verbose_easy`, `spelling_error`,
`missing_context`, `adversarial_quoted`, `unicode`, plus the public sample's own tags.
`tests/classifier_data.rs` asserts the held-out split carries every one of these.

## Split policy and leakage check

- Every case belongs to a `family`; paraphrases and near-duplicates share a family.
- A family never spans splits (`split-manifest.json` lists each split's families).
- `tests/classifier_data.rs` additionally computes token-set Jaccard over all queries and
  fails if any pair ≥ 0.6 crosses a split or sits in one split without sharing a family.
- The held-out split was not inspected while writing the Jev rubric, the conversion rule
  or the threshold. Dev supported rubric wording; calibration is reserved for choosing
  `CLASSIFIER_MIN_CONFIDENCE` and comparing the two complexity rules once live data exists.

## What the public sample is used for

Smoke only. Regex scores 3/10 on it (see RESULTS.md); it is far too small to rank anything.

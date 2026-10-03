# Request classifier data and labelling criteria

`labelled.jsonl` trains the `local` request classifier (`src/routing/linear_classifier.rs`).
`linear-v1.json` is the model `examples/classifier_train.rs` produces from it, byte for byte (a
unit test retrains and compares).

Each line is `{id, query, context, request_type, complexity}`. All 438 examples were written by
hand for this project. They are synthetic, and contain no user data or private prompts.

## request_type: label the main deliverable

Ask "what does the person want handed back?" and label that, not the topic or the keywords.

| label | the deliverable is… | examples |
|---|---|---|
| `code_generation` | new or changed code, config, queries, commands or tests, however small (a typo fix in a comment counts) | write, fix, refactor, port, rename, add tests |
| `code_understanding` | an explanation or review of existing code, without new code as the main output | what does this do, why does it fail, trace it, review this diff |
| `technical_design` | the design of a system: architecture, API or schema design, migration or rollout plan, technology choice | design X, how should we structure, compare and recommend an architecture |
| `analytical_reasoning` | a conclusion reached by reasoning over evidence or numbers: diagnosis, root cause, math, data analysis, a decision with trade-offs | diagnose from logs, is this A/B result valid, estimate capacity |
| `writing` | prose: compose, rewrite, summarize, translate, proofread, change tone | email, release notes, summary for execs, make it friendlier |
| `factual_lookup` | a short answer that is a known fact or definition, with no reasoning chain | capital of X, default port, what does `pass` do |
| `general` | chit-chat, meta questions, personal advice, recommendations, anything else | greetings, thanks, recipes, travel, motivation |

Tie-breaking rules:

- **Multi-intent:** label the part that needs the most capability. "Explain what's wrong, then rewrite it" is `code_generation`.
- **Negation:** label what is actually asked. "Don't redesign anything, just rename X" is `code_generation`.
- **Misleading keywords:** a request that mentions "architecture" but asks for a smoother intro paragraph is `writing`.
- **Padding and noise:** ignore greetings, apologies and backstory.
- **Code question versus fact:** a question about a language feature or API with no code to read is `factual_lookup`. A question about pasted code is `code_understanding`.
- **Debugging:** "why does this pasted snippet fail" is `code_understanding`. Diagnosis from logs, metrics or event sequences is `analytical_reasoning`. "Write the fix" is `code_generation`.

## complexity: 1 to 5

The public eval's rubric:

1. Trivial single operation: rename, one fact, one-line command.
2. Straightforward: one function, one email, one short explanation.
3. Multi-step with limited constraints: a tested utility, a summary with audience rules, a small diagnosis.
4. Substantial reasoning or design: architecture with trade-offs, multi-hypothesis diagnosis.
5. Intricate cross-component reasoning and validation: consensus, concurrency races with interleavings, multi-region consistency.

Context counts. A short query with a long, contradictory context is rated higher than the query alone.

## Composition

| request_type | examples |
|---|---|
| code_generation | 83 |
| factual_lookup | 68 |
| writing | 64 |
| code_understanding | 57 |
| technical_design | 56 |
| analytical_reasoning | 55 |
| general | 55 |

Complexity is skewed toward 1 to 3, with only 8 examples at level 5, which matches expected traffic.

Deliberately hard cases are spread across labels:

- keyword near-misses;
- negation;
- multi-intent requests;
- padded or noisy text;
- out-of-domain phrasings (recipes, home network, bills);
- terse fragments ("capital of canada", "12 * 17?").

**Disclosure:** the 39 `i*` examples were added after reading the error pattern on the public 10-case smoke set. The pattern was requests whose context is prose constraints rather than code or data. None of those examples copies a public case: the maximum query Jaccard similarity against the public set is 0.31. Because of this, public smoke-set accuracy is not an independent measurement.

## Splits

`linear_classifier::split` unions examples whose query token sets have Jaccard similarity ≥ 0.6. Each group goes wholly to validation when the FNV-1a hash of its first id is ≡ 0 (mod 5). That gives 350 train and 88 validation examples. The trainer asserts that no validation query reaches 0.6 similarity with any training query. The maximum observed is 0.50.

The model is trained on the train split only. Its temperatures are fitted on 5-fold out-of-fold predictions within train. Validation numbers are therefore held out end to end.

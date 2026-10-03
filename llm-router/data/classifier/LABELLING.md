# Request classifier dataset

Hand-written, synthetic queries used to train (`train.jsonl`, 654 rows) and evaluate
(`val.jsonl`, 70 rows) the `nb` request classifier
(`src/routing/naive_bayes.rs`). No user data and no rows from the public
`classifier-eval` sample are included.

Each line: `{"id", "query", "context", "request_type", "complexity"}`. `context` is
`""` when the query stands alone.

## Request type: label the deliverable

Label what the user wants **back**, not the words they use.

| label | the answer is… | examples of near-misses kept on the right side |
|---|---|---|
| `code_generation` | new or changed code, config, SQL, shell, tests, however small (a one-line fix or a rename counts) | "Draft a Python script…", "Calculate shipping cost in code: write a function…", "Explain nothing, just give me a makefile target" |
| `code_understanding` | an explanation, trace, review or diagnosis of **given** code, with no rewrite asked for | "What design pattern is this class using?", "Summarize what this module is responsible for" |
| `technical_design` | an architecture, schema, technology choice, migration or rollout plan, or a trade-off decision | "Draft an architecture for…", "REST or gRPC between our services?" |
| `analytical_reasoning` | a computed, inferred or argued conclusion: maths, probability, estimation, logic, root cause from evidence (metrics, logs, timelines) | "how many seconds are in a leap year", "Write up your reasoning: is it statistically sound…" |
| `writing` | prose for a human reader: emails, posts, summaries, rewrites, tone changes, proofreading, stories | "Turn my rough notes into a design doc introduction", "Rewrite this API error message so users understand it" |
| `factual_lookup` | a short, checkable fact or definition that exists independently of the conversation (incl. API and tool semantics with no code to read) | "What's the architecture of the Cortex-M4, Harvard or von Neumann?", "How many bytes are in a UUID?", "What's the system design concept called where you write to a log first?" |
| `general` | conversation, personal advice, recommendations, planning, brainstorming, acknowledgements | "Write me a list of fun weekend activities", "How can I be a better mentor to a junior developer?" |

Tie-breakers:

1. **Multi-intent**: label the part that carries the most work, which is usually the
   final deliverable. "Explain what's wrong, then rewrite it" is `code_generation`;
   "Compare Kafka and Pulsar, then propose our topic layout" is `technical_design`.
2. **Any requested change to code is `code_generation`**, however small: a misspelling,
   a rename, a constant, a log level, a doc/comment edit, a one-line change. This holds
   even when a lot of code is pasted around the change. `code_understanding` is only for
   explaining, tracing or reviewing code **without** changing it. ("Why does this crash?
   Give me a corrected version" is `code_generation`; "Which line leaks? Don't change it"
   is `code_understanding`.)
3. **Negation**: label what is asked, not what is ruled out ("don't rewrite it, just point
   things out" is `code_understanding`).
4. **Diagnosis**: reading given code is `code_understanding`. Reasoning over evidence
   (metrics, logs, timelines, numbers) is `analytical_reasoning`. Proposing the fix
   architecture as the main ask is `technical_design`.
5. **Padding and noise** (greetings, apologies, typos, urgency) never change the label.

## Complexity 1–5

Same scale as the eval file: 1 trivial single operation; 2 straightforward; 3 multi-step
with limited constraints; 4 substantial reasoning or design; 5 intricate cross-component
reasoning and validation. Label the effort a strong engineer or writer would need, not the
length of the text. A long padded request for a one-liner is still 1.

The `nb` backend does not learn complexity. It applies the fixed rubric in
`rubric_complexity` (base by type, plus length, enumerated requirements and rigour words,
minus minimal-edit words), and these labels only measure that rubric.

## Coverage and hygiene

- Each split covers all 7 types (train 85–87 per type, plus 52 extra `code_generation` rows for tiny edits; val 10 per type) and all complexity
  levels, including ambiguous, multi-intent, paraphrased, padded/noisy, and near-miss
  rows where a regex keyword points at the wrong type. The second batch of training rows
  (ids past the original 35 per type) deliberately targets the confusable pairs:
  writing vs analytical_reasoning (writing that carries numbers; analysis phrased as
  "write up / explain"), code_generation vs code_understanding (explain-then-fix, "don't
  fix, just explain"), technical_design vs analytical_reasoning (designing vs diagnosing),
  factual_lookup vs general (everyday facts vs advice), and adds non-software domains
  (finance, health, law, cooking, sport, travel) and noisy/Hinglish-adjacent phrasing.
  A third batch (52 `code_generation` rows) applies tie-breaker 2: small edits (rename,
  constant, log level, flag, docstring, one-line fix) to pasted code in many languages,
  often with much more code pasted than the edit touches.
- **Near-duplicates are excluded**: no two rows in or across splits have a query-token
  Jaccard similarity of 0.6 or more. The unit test
  `no_near_duplicates_within_or_across_splits` enforces this.
- The validation split is never used for fitting. The softmax temperature and the default
  confidence threshold were chosen by 5-fold cross-validation on the training split
  (`cargo test -p nasiko-llm-router tune_temperature -- --ignored --nocapture`).
- Single annotator, synthetic data: treat accuracy on `val.jsonl` as in-distribution and
  optimistic. The private set's phrasing will differ.

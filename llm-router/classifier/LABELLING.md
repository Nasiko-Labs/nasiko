# Request classifier — labelling criteria

These rules define the labels for the classifier's training and validation data
(`data/train.json`, `data/val.json`) and are the same definitions the hosted backend's prompt
uses (`src/routing/classifier/hosted.rs`). They follow the published eval set
(`classifier-eval@v1-sample`) and extend it where it is silent.

Each example has the eval-set schema:

```json
{"id": "tr-00001", "query": "...", "context": "...", "request_type": "writing",
 "complexity": 2, "tests": ["paraphrase", "regex_near_miss"], "group": "writing-0042"}
```

`context` may be empty. `tests` tags the slices the example exercises. `group` ties together
paraphrases and variants of one underlying request, so a whole group lands in one split.

## 1. Request type — label the main deliverable

Ask: *what does the user want handed back?* Ignore which keywords appear.

| Label | The user wants back… | Examples |
|---|---|---|
| `code_generation` | New or changed code: write, implement, fix, refactor, port, add tests, edit a config/SQL/regex/shell command, even a one-character edit to code or a code comment. | "Fix typo in this Python comment"; "change `TODO` to `NOTE` in this line"; "write a bash one-liner that counts lines" |
| `code_understanding` | An explanation or review of **specific code that is given** (in the query or context), without changing it: what it does, why it behaves so, trace its output, review it. | "Explain why this function returns the old value" (+ code); "what does this regex match?" (+ regex) |
| `technical_design` | A plan for a system: architecture, API/schema design, migration strategy, technology trade-offs, scaling, rollout. | "Design migration from sync callbacks to a queue"; "Postgres or DynamoDB for this workload?" |
| `analytical_reasoning` | Reasoning to a conclusion: diagnose from evidence (logs, symptoms, incidents), debug *why* without being handed the fix, calculate, estimate, prove, solve a puzzle, compare options quantitatively. | "Diagnose intermittent 401s from these logs"; "what's the probability of two sixes?" |
| `writing` | Prose for a human audience: compose, rewrite, edit tone, summarize, translate, outline, brainstorm names/taglines, poems and stories, including on technical topics. | "Summarize these release notes for customers"; "write a poem about Python" |
| `factual_lookup` | A short answer recalled, not derived: a fact, definition, date, or what a named API/command/concept does in general, with no user code to analyse. | "What does `Option::take()` do?"; "capital of Australia?" |
| `general` | Everything else: greetings, thanks, chit-chat, opinions, personal advice, recommendations, questions about the assistant, empty or unintelligible input. | "hi!"; "which movie should I watch tonight?" |

### Tie-breakers

1. **Deliverable over keywords.** "Write a poem about Python" is `writing`. "Design a logo
   tagline" is `writing`. "Calculate the Big-O of this code" (+ code) is `code_understanding`.
2. **Multi-intent: label the part that dominates the effort.** "Explain this function, then
   rewrite it in Go" is `code_generation` (the rewrite is the larger deliverable). "Diagnose
   the duplicate charges and propose a repair plan" is `analytical_reasoning` (the
   investigation dominates). If the parts are equal, label the last explicit deliverable.
3. **Negated instructions don't count.** "Do not redesign anything: just change `TODO` to
   `NOTE`" is `code_generation`, complexity 1.
4. **Given code vs. general concept.** A question about code the user supplied is
   `code_understanding`; the same question about a language feature in general is
   `factual_lookup`.
5. **Fix vs. diagnose.** "Fix this bug" (+ code) is `code_generation`. "Why does this
   service drop requests under load?" (+ logs, no code to edit) is `analytical_reasoning`.
6. **Design vs. implement.** If the user wants the plan, it is `technical_design`. If they
   want the code that implements a plan, it is `code_generation`.
7. **Recall vs. derive.** Recalling a fact is `factual_lookup`. Computing or deriving one,
   even simple arithmetic, is `analytical_reasoning`.
8. **Noise and padding don't change the label.** Greetings, apologies, signatures, pasted
   boilerplate and typos around a request are ignored. The request is labelled.
9. **Language doesn't change the label.** A non-English request is labelled by its
   deliverable like any other.
10. **Out of domain still gets a label.** Non-technical requests follow the same rules:
    a recipe request is `general` (advice); "write a toast for my sister's wedding" is
    `writing`; "how many grams in an ounce?" is `factual_lookup`.

## 2. Complexity — 1 to 5

The published rubric, with anchors:

| Score | Definition | Anchors |
|---|---|---|
| 1 | Trivial single operation | Rename a variable, fix a typo, one-line fact, greeting, convert a unit |
| 2 | Straightforward | Explain a short function, rewrite a paragraph in a warmer tone, a small self-contained function |
| 3 | Multi-step with limited constraints | Implement a parser with tests, summarize a document for an audience with rules, compare two options with reasons |
| 4 | Substantial reasoning or design | A migration plan with rollout and rollback, a multi-step incident diagnosis, a design under several constraints |
| 5 | Intricate cross-component reasoning and validation | Concurrency/ordering bugs across services with uncertain evidence, a design that must reconcile contradictory requirements and prove invariants |

Rules:

- Score the **work needed to answer well**, not the length of the query. A long, padded
  query can be complexity 1; a two-line concurrency question can be 4.
- Context raises complexity when it must be used (constraints, logs, code to reason over),
  not when it is merely present.
- Explicit constraints ("keep the date unchanged", "no external crates") add roughly one
  step each, up to the score's ceiling.
- `general` is almost always 1. `factual_lookup` is 1–2.

## 3. Slices (`tests` tags)

Every split covers these slices, because the private scoring set does:

| Tag | Meaning |
|---|---|
| `clean` | A clear, direct request |
| `paraphrase` | A rewording of another example in the same `group` |
| `ambiguous` | Plausibly two labels; the tie-breakers above decide |
| `multi_intent` | Several asks in one request |
| `noisy` | Typos, slang, missing punctuation, casing |
| `padded` | Greetings, apologies, irrelevant preamble or pasted boilerplate around the request |
| `ood` | Out of distribution: non-English, non-technical domains, unusual formats |
| `regex_near_miss` | Keywords that point the regex at the wrong label |
| `with_context` | The context decides or changes the label or complexity |
| `negation` | An instruction that rules out the keyword-suggested task |

## 4. Splits and leakage

- Examples are grouped (`group`); a group never spans train and validation.
- Near-duplicates across groups are merged before splitting: character 3–5-gram TF-IDF
  cosine similarity ≥ 0.85 joins two groups (see `scripts/build_splits.py`).
- Validation is about 12% of groups, stratified by label.
- No example is copied or paraphrased from the published eval sample; the build drops anything within
  cosine 0.45 of a public case and fails if one survives.

# Labelling criteria: request classifier

Every item gets exactly one `request_type` and one `complexity`. The labels must match the router's `RequestType` wire names, because those names are the persisted learning keys.

## Complexity (rubric, verbatim)

> 1 = trivial single operation; 2 = straightforward; 3 = multi-step with limited constraints; 4 = substantial reasoning or design; 5 = intricate cross-component reasoning and validation.

- Judge the **effort a capable model needs**, not the topic and not the length.
- More constraints, or a larger amount of attached material that must be used, can raise complexity.
- Padding, greetings and verbosity never do.

## Request types: label the primary deliverable

The label is whatever most of the answer's effort produces.

| Label | Definition | Typical cues (not rules) |
|---|---|---|
| `code_generation` | The output is new or modified code, config, SQL, regex, a shell command or tests. **This includes trivial edits** (fix a typo in a comment, rename a variable, change `TODO` to `NOTE`) and "explain the bug and fix it". | implement, write a function, fix, add tests, convert this to |
| `code_understanding` | Explain, trace or review **given** code without producing new code as the main output. | what does this code do, walk me through, why does this snippet print, review |
| `technical_design` | Architecture, system, API, schema or migration design; rollout and trade-off planning for something not yet built. | design, architecture, how should we structure, migration plan |
| `analytical_reasoning` | Diagnosis from evidence (logs, metrics, traces), root cause, reconstructing event orderings, maths, logic, quantitative estimation. **Wins over design or code when the request starts by investigating given evidence.** | diagnose, why did X happen given these logs, compute, prove, estimate |
| `writing` | Prose for humans: draft, rewrite, summarise, change tone or adapt for an audience. This holds **even when the source material is technical** (e.g. release notes turned into customer bullets). | email, summarise, rewrite, announcement, make it friendlier |
| `factual_lookup` | A short, known fact, a definition, or documented API/library behaviour, with **no user artefact to analyse** ("What does `HashMap::entry` do?"). | what is, what does X do, which, when was |
| `general` | Chit-chat, meta questions, non-technical advice, gibberish. Use it **only when nothing else fits**. | thanks, hi, life advice |

## Tie-breaks for multi-intent requests

1. Go by the explicit primary verb first.
2. If still tied: evidence diagnosis (`analytical_reasoning`) > `technical_design` > `code_generation` > `writing`.
3. If still tied: whichever deliverable dominates the requested output.

## Special rules

- **Negated scope removes that intent.** For "Do not redesign anything, just rename the field", label the requested action (`code_generation`, complexity 1).
- **Misleading keywords don't decide the label.** Domain words (cache, Redis, auth, architecture, design, calculate, fix) don't make an item a code or design type. Examples:
  - "Fix the tone of this email" is `writing`.
  - "What's the architecture style of the Pantheon?" is `factual_lookup`.
- **API behaviour vs. given code.** "What does `Option::take` do?" with no user code is `factual_lookup`. "What does this function do?" plus a snippet is `code_understanding`.
- **Single snippet vs. evidence.**
  - Explaining why one given snippet behaves as it does is `code_understanding`.
  - Reconstructing what happened from logs, metrics or several interacting components is `analytical_reasoning`.
- **Advice vs. writing.** "How should I handle a difficult coworker?" is `general`. "Write a message to my coworker about…" is `writing`.
- **Ignore padding.** Greetings, sign-offs, apologies, emoji and typos don't change the label or the complexity.
- **Non-English requests** are labelled by the same rules.

## Worked examples
These were written for this guide. None comes from any evaluation set.

| Query (context) | Label | Cx |
|---|---|---|
| Rename `cnt` to `retry_count` in this function. (snippet) | code_generation | 1 |
| Write a Python CLI that tails a log file and alerts on 5 consecutive ERROR lines; include tests. | code_generation | 3 |
| What does this regex match? `^(?=.*\d)[A-Za-z\d]{8,}$` | code_understanding | 2 |
| Review this Go worker pool for races and explain each one you find. (code) | code_understanding | 4 |
| How should we shard the events table as we grow from 1M to 1B rows a day? | technical_design | 4 |
| Design the API, data model and rollout for multi-region active-active sessions with conflict resolution. | technical_design | 5 |
| Given these three log excerpts, reconstruct the order in which the two workers updated the row. | analytical_reasoning | 4 |
| What's the probability of at least one six in four rolls of a die? | analytical_reasoning | 2 |
| Rewrite this outage notice so non-engineers understand it; keep it under 80 words. (notice) | writing | 2 |
| Turn these engineering notes into a quarterly update for the board. (notes) | writing | 3 |
| What port does PostgreSQL listen on by default? | factual_lookup | 1 |
| What does `Array.prototype.flatMap` return? | factual_lookup | 1 |
| thanks, that's all for today! | general | 1 |
| Any tips for staying focused when working from home? | general | 2 |

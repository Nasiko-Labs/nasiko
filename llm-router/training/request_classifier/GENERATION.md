# How the dataset was generated

The data was written by the team's LLM assistant (Claude, in Claude Code), following [`LABELING.md`](LABELING.md). No external API key was used. Scenarios are stored in compact JSONL under `data/base/`. `rc.py build` turns them into the full schema and adds variants deterministically.

## Rules every authoring batch followed
- Follow `LABELING.md`: label the primary deliverable, and apply the tie-breaks and the negation and misleading-keyword rules.
- **Never copy or paraphrase the public evaluation set.** It was read only for its schema, rubric and style. Items the author judged structurally close to a public case were rewritten. `rc.py split` also drops anything with char-5-gram Jaccard ≥ 0.5 to a public query.
- Vary the domain (web, systems, data, infra, mobile, finance, health, legal, retail, everyday life), the phrasing (terse, verbose, imperative, question), the language, and the context form (code snippet, logs, notes, constraints, placeholder, none).
- Keep complexity honest. Judge effort, not length: padding and greetings never raise it.
- Invent all attached material (code, logs, release notes, numbers). Never include real user data or secrets.

## Batch prompts used
One batch per file. Each was run as an instruction to the authoring model with `LABELING.md` in context.

| File | Prompt (abridged) |
|---|---|
| `code_generation.jsonl` | "Write ~80 requests whose primary deliverable is code. Include trivial edits (typos in comments, renames, flag flips), routine functions, multi-step tasks with tests, and a few complexity 4–5 systems tasks. Add ~10 boundary items: 'explain the bug and fix it', 'what does this do? rewrite it', implementing an already agreed design, and misleading words (architecture, probability) inside code tasks. About half carry a code or schema snippet as context." |
| `code_understanding.jsonl` | "Write ~50 requests to explain, trace or review GIVEN code without producing new code: snippets in many languages, configs, shell, SQL, regex. Include 'why does this print X', race and lifetime explanations, and 'don't change anything, just tell me'. Code always goes in context." |
| `technical_design.jsonl` | "Write ~50 requests for architecture, schema, API or migration design and trade-off planning across scales. Include boundary items: 'don't implement yet', 'sketch interfaces only', 'design the schema, I'll write migrations', rollout plans." |
| `analytical_reasoning.jsonl` | "Write ~50 requests needing diagnosis from evidence (logs, metrics, timelines, query plans, bills), maths, probability, logic or estimation. Put the evidence in context. Include evidence-first requests that also ask for a fix (they stay analytical)." |
| `writing.jsonl` | "Write ~50 requests for prose for humans: emails, announcements, rewrites, summaries, translations, speeches. Include technical source material to be rewritten for non-technical readers, and misleading verbs (fix the tone, refactor this paragraph)." |
| `factual_lookup.jsonl` | "Write ~65 short factual questions: documented API or library behaviour with no user code, definitions, defaults, trivia. Include misleading domain words (architecture, design, cache) in purely factual questions." |
| `general.jsonl` | "Write ~50 chit-chat, meta and non-technical advice requests, including misleading technical words used casually ('debug my sourdough starter')." |
| `traps.jsonl` | "Write ~55 items where negation removes an intent ('don't redesign, just rename') or a misleading keyword points to the wrong type. Spread them across all seven types." |
| `multi_intent.jsonl` | "Write ~35 requests with two or more deliverables. Label each by the tie-breaks (primary verb; analytical > design > code > writing; dominant deliverable)." |
| `noisy_ood_multilingual.jsonl` | "Write ~80 items: organically noisy or rambling requests with typos and emoji; out-of-domain requests (legal, medical, finance, retail, spreadsheets, gibberish); and ~35 non-English or code-switched requests across all types." |
| `complex.jsonl` | "Write ~25 genuinely complex (mostly complexity 5) requests across types, with substantial attached material, to fill out the top of the complexity scale." |
| `iteration1_train_only.jsonl` | Error-analysis round (val only). "Write ~100 NEW scenarios (not paraphrases of val items) covering these confusion patterns: prose deliverables introduced by 'write'; small code edits phrased as 'convert/add/change'; maths word problems; 'right way to model X' design questions; idea or advice requests; and more non-English across types." Marked `train_only`. |

## Variants (`rc.py build`, deterministic)
For each scenario, an RNG seeded from `(scenario id, seed 13)` picks the variant types:
- **First variant:** padded or noisy.
- **Second variant (50% of scenarios):** placeholder context if the scenario has no context, otherwise the other of padded/noisy.

The lists of greetings, sign-offs, fillers and placeholders are in `rc.py`.

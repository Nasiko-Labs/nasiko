# Code review evals

Status: proposed, implemented as a thin vertical slice; not yet merged upstream. This document is
the PRD, TRD, grading methodology and limitations list in one, written at hackathon speed — see
"Why one document" below.

## Why

Nasiko's existing Evals ([docs/EVALS.md](../../EVALS.md)) answers "which model best reproduces a
real bug fix" (replay evals: a mature, shipped SWE-bench-Verified-style pipeline). It does not
answer a second, equally common question a team asks before adopting a model: **"which model is
best at catching real problems when it reviews a pull request, and what does that cost?"** No part
of the codebase does PR-level ingestion, reference-finding grading, or review-quality comparison
today (confirmed by reading `evals/`, `server/src/evals/`, `github/src/service.rs` and
`github/src/history.rs` before writing any code).

## Why one document, not four

The full brief this slice was built from asks for a PRD, TRD, implementation plan and methodology
doc as four files, each covering all three categories (code review, code generation, security) at
production scope. Given a same-day time budget, shipping one real, defensible, end-to-end
capability was prioritized over four documents describing a larger scope that could not actually
be built and verified in the time available. Code generation benchmarking already exists and needs
no new work. Security benchmarking is not attempted here — it is real future work, not something to
gesture at with an unimplemented doc section.

## Scope: what this slice does

- Import **one** GitHub pull request at a time, at its exact base/head commit
  (`POST /api/evals/code-review/tasks`).
- Optionally add **human-validated reference findings** to that task
  (`POST /api/evals/code-review/tasks/{id}/reference-findings`) — real ground truth, not every
  GitHub review comment treated as correct.
- Review the PR's diff (plus the touched files' text at head, for context) with one or more of the
  caller's own LLM configs, using an identical system prompt, diff and file context for every
  model in the run (`POST /api/evals/code-review/tasks/{id}/runs`).
- Grade each model's findings against the reference findings with **deterministic matching only**
  (file + category + a small line-distance tolerance), reporting true/false positives, false
  negatives, precision, recall and severity accuracy.
- Report **real router-billed cost and latency** per model, read back from `trace_usage` by the
  attempt's own tag — never estimated client-side, never fabricated when the router hasn't billed
  yet (`GET /api/evals/code-review/runs/{id}`).
- When a task has no reference findings, a run on it is explicitly labelled an **"ungraded
  preview"**: findings are shown, but no precision/recall number is ever computed or displayed —
  this is the brief's own rule ("a single PR without reference findings... must not be treated as a
  trustworthy benchmark result"), enforced by the grader never running at all in that case.
- Every result always carries a sample-size warning, because this slice grades one task at a time —
  it does not claim a statistically meaningful ranking over n=1.

## Scope: what this slice explicitly does not do (follow-up work, not hidden gaps)

- **No multi-PR dataset manager.** No "connect a repo, import 50 PRs" workflow. One task is
  imported at a time, deliberately, to keep every number behind it real and inspectable rather than
  spread thin across an unverified pile of auto-imported PRs.
- **No durable job queue.** Replay evals' `eval_jobs` (harvest/build/screen/run, SKIP LOCKED,
  heartbeat, retry) is the right design for an hours-long harness session. A code review is one
  short completion call, so an attempt is run with `tokio::spawn` directly; a server restart
  mid-run loses that attempt. This is an accepted limitation at this scope, not an oversight — see
  `server/src/code_review/mod.rs`'s doc comment.
- **No semantic or LLM-assisted finding matching.** The brief allows this only as a *supplement* to
  a deterministic method, never a replacement; only the deterministic matcher
  (`server/src/code_review/grader.rs`) ships. A close-but-not-exact match (different file, or a
  category the model phrased differently) is scored as a miss, which understates recall for models
  that write looser categorization — a known, documented conservative bias.
- **No security benchmarking track.** Shares the same underlying primitives (a task, a run, an
  attempt, a grader) but needs its own labelled vulnerability datasets and its own defensible
  checks, which were not available to build and validate in this time budget.
- **Eval Universe** (`/evals/universe`, `ui/shared/features/src/evals/universe/`): a follow-up
  pass built a cross-task 3D view on top of a new `GET /api/evals/code-review/universe` aggregate
  endpoint (`server/src/code_review/api.rs::get_universe`, `store.rs::users_universe_attempts`) —
  one cluster per task, one point per attempt, colored by precision and sized by tokens used
  (`@react-three/fiber` + `@react-three/drei`, the monorepo's first 3D/WebGL dependency). Explicitly
  simple: a fixed circular layout, not force-directed physics; a flat capped list
  (`MAX_UNIVERSE_ATTEMPTS = 500`), not cursor pagination; the caller's own data only, never a
  cross-user view. A floating in-3D label per cluster was attempted and dropped — `@react-three/
  drei`'s `Html` loses its children under this app's `StrictMode` (`shared/app-shell/src/
  mount.tsx`) double-mounting Canvas's own reconciler root on first render; the task identity lives
  in the (reliable) hover tooltip and a plain HTML caption above the canvas instead. This also
  carries forward, rather than fixes, the known `store::attempt_usage` cost/token display bug
  (see that function's doc comment) — a universe point's size/cost can read as 0 even when the
  real `trace_usage` row has real figures, because the attempt's own query ran before the async
  usage backfill landed.
- **No OpenAPI registration.** `server/src/openapi.rs`'s `ApiDoc` macro is an "opt-in, incremental
  rollout" by its own doc comment; the new routes were left unregistered to avoid editing a large,
  unseen macro invocation under this time budget. Follow-up, not a blocker.

## Architecture

New, small, sibling module: `server/src/code_review/` (`mod.rs`, `api.rs`, `store.rs`,
`grader.rs`, `prompt.rs`), mounted the same way `evals` is
(`server/src/lib.rs`: `pub mod code_review;` / `.merge(code_review::router())`). It was **not**
built by extending `eval_tasks`/`eval_runs`/`eval_attempts`: those tables and their Rust types
(`TaskInstance`, `gate`, `link`, `screen` JSONB) are shaped tightly around the SWE-bench-style
harness pipeline, and bending a one-shot PR review into that shape would be exactly the kind of
"duplicate domain model via misuse" the brief warns against.

What *is* reused, deliberately:
- `evals::llm::complete()` — one router-attributed chat completion against a caller's
  `llm_configs.id`, tagged and costed. A review is a single completion, not an agent harness
  session, so this primitive is the right fit; the container/opencode replay machinery is not
  needed and was not touched.
- The `trace_usage.eval_attempt` tag convention and its index (migration `0116`): review attempts
  reuse the same column with a `cr-<run>:<model>` tag (distinct from replay evals' `<run>:<attempt>`
  tags), so costs come from the router's own billing, the same way replay evals' do. The ~15-line
  aggregate query is duplicated into `code_review::store` rather than exposed across modules — the
  `evals::usage` module that has the original is private to `evals`, and replay evals' own
  `ApiError` already carries a `TODO NAS-1098: replace with a shared dto::ApiError` for the same
  reason. A follow-up PR could extract both into a shared crate.
- `evals::forge::Forge` for GitHub access, extended with one new method,
  `Forge::pull_request(number)`, and one new `GitHubService::pull_request` REST call
  (`github/src/history.rs`, mirroring `commit`/`compare_diff`'s existing pattern exactly). Two
  `mod` declarations in `evals/mod.rs` were widened from private to `pub(crate)` (`forge`, `llm`)
  to allow this reuse — an additive, behavior-preserving visibility change, not a rewrite.

Database: one new migration, `migrations/0120_code_review_evals.sql` — `review_tasks`,
`review_runs`, `review_attempts`. Schema and comments mirror `0118_eval_runs.sql`'s conventions
(JSONB snapshots, per-row status CHECKs, `cold: compatible` marker).

## Grading methodology

1. A model's reply is a JSON array of findings: `{file, line, severity, category, description,
   confidence}` (`server/src/code_review/prompt.rs`). Only the outermost `[...]` is parsed, so a
   reply wrapped in prose or a fenced code block still parses; a reply with no array at all is a
   failed attempt, not a silently-empty one.
2. Matching (`server/src/code_review/grader.rs`) is **deterministic and greedy, one-to-one**: a
   model finding matches a reference finding when they name the same file and category and a line
   within 3 of each other; each reference claims at most one model finding (the closest), and a
   claimed model finding cannot match a second reference. This specifically prevents a model that
   repeats the same finding from inflating its own recall.
3. **True positive** = a reference finding with a match. **False negative** = a reference finding
   with none. **False positive** = a model finding nothing claimed it (including a reference's
   age-left duplicate). Precision = TP/(TP+FP), recall = TP/(TP+FN); both read as 0, not a
   misleading 1, when their denominator is 0. **Severity accuracy** = among true positives only,
   the fraction whose severity matched exactly.
4. This method is never run on a task with no reference findings (`review_tasks.graded = false`) —
   the API never computes or returns `metrics` for such a run, and the UI's results table shows a
   dash with a standing "ungraded preview" banner in that case, never a number.
5. Cost and latency are the router's own figures, read back from `trace_usage` by the attempt's own
   tag after the call completes; a model the router cannot price fails to report a cost rather than
   guessing one (same principle as `docs/EVALS.md`'s own cost-attribution rule).

## Fairness across models

Every model in a run gets the exact same system prompt, the same diff, the same touched-file
context, and the router's zero-temperature setting (`evals::llm::complete`'s existing behavior).
Models are only ever a run's `llm_configs`; the agent harness is held constant because this is a
single completion call, not an agent session, so harness-vs-model confounding does not apply here
the way it would for a coding-agent comparison.

## Statistical reliability

One task per run, by design (see "what this slice does"). Every run response always includes
`sample_size_warning` ("Treat any difference between models as directional only, not a
statistically significant ranking."), regardless of how many models were compared, because adding
more models to one task's run never increases the sample size that matters: tasks graded.

## Testing done

- `grader.rs`: exact match, line-tolerance boundary, category mismatch, duplicate-finding handling,
  severity accuracy counted over true positives only, zero-reference and zero-model-output edge
  cases, and the price-source "worst of" ordering — all as unit tests, no I/O.
- `prompt.rs`: empty-array reply, a fenced-code-block-wrapped reply, prose with no array (an
  error), and a malformed entry (an error, not a silent drop).
- `api.rs`: the unified-diff touched-file parser (added-side targets only, `/dev/null` correctly
  excluded since it never matches the `+++ b/` prefix) and the owner/repo path-segment validator.
- `github/src/history.rs`: the new `pull_request` reply shape deserializes its base/head SHAs.
- **SQL verification**: every hand-written query in `code_review::store` (the upsert-on-reimport,
  the grading flip, the zero-row and one-row usage aggregate, attempt finish/fail, and the
  `settle_run` `UPDATE ... FROM` subquery) was run against a real `postgres:17` container with a
  minimal stub schema and representative data, and produced the expected rows in every case.

## Known limitation of this review: unverified by a Rust/TS compiler

This environment had no Rust toolchain installed, and both `cargo`'s usual Docker-based fallback
and `pnpm`'s package fetch were blocked by the sandbox's network restrictions (Docker Hub TLS
verification failure, a blocked `crates.io`, and a corepack/pnpm download that failed silently).
**No part of this Rust or TypeScript code has been compiled, linted, or type-checked.** The SQL
itself was verified directly against Postgres (see above), which de-risks the layer most prone to
silent bugs in a Rust+sqlx codebase, but type-level mistakes (a wrong trait bound, an import path,
a borrow-checker objection) are not ruled out. Before relying on or submitting this work:

```
cargo fmt --check -p nasiko-github -p nasiko-server
cargo check -p nasiko-github -p nasiko-server
cargo clippy -p nasiko-github -p nasiko-server -- -D warnings
cargo test -p nasiko-github -p nasiko-server
cd ui && pnpm install && pnpm build && pnpm vitest run
```

## Upstream process

`CONTRIBUTING.md` asks for an issue opened and maintainer agreement before building a new feature.
Given the hackathon time budget, this slice was built in parallel with that issue rather than
waiting on a reply — see the issue text prepared alongside this doc for the user to review and
post. No fork of `Nasiko-Labs/nasiko` exists yet under this environment's GitHub identity; creating
one was attempted and denied by this session's auto-approval policy (classified as a
destructive/unwanted action), so it is still pending the user's own action or explicit approval
before any branch can be pushed or a PR opened.

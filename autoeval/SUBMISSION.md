# Nasiko AutoEval — buildathon submission

**Scope.** A runnable, standard-library Python CLI extends Nasiko evaluations to
Python/pytest, JavaScript/Jest, TypeScript/Vitest, Go and Rust/Cargo. All adapters
produce one execution-plan schema. Task extraction, Docker execution, validation,
one-shot model attempts, grading and reporting are shared. The existing prebuilt
server and seven-service stack remain running; no backend or frontend rebuild was
required. Claude Code/Codex ingestion is not a prerequisite.

**Measured real work.** Public upstream commits produced three verified tasks:

| Repository / language | Fix | Fail-to-pass | Pass-to-pass |
| --- | --- | ---: | ---: |
| pytest-dev/iniconfig / Python | `d2dcb7289291` | 3 | 51 |
| dustin/go-humanize / Go | `1b638b2c0d9b` | 1 | 68 |
| unjs/pathe / TypeScript | `b52fcacc5971` | 10 | 457 |

Every task failed on old code and passed on the reference fix twice in fresh
offline containers. Pathe additionally has 16 optional skipped/todo tests;
these do not count as passing requirements. Exact parent/fix SHAs, test identities,
commands, images and logs are in [results.json](results/results.json).

Two working Nasiko configurations each attempted all three tasks. Gemini 3.1
Flash-Lite resolved **2/3 (66.7%)**; Gemini 3.5 Flash-Lite resolved **1/3 (33.3%)**.
Both fixed Go; only 3.1 fixed Python; both failed TypeScript. The TypeScript grader
caught existing-suite regressions and an invalid TypeScript replacement. A
separate Gemini 3.8 Flash attempt timed out and is recorded as an infrastructure
failure. Models received identical prompts and budgets per task: 4,096 output
tokens for Python/Go and 8,192 for TypeScript, temperature zero, one generation
request each. These selected tasks support those outcomes, not a general ranking.

**Cost evidence and blocker.** Six completed Lite attempts have attributable
Nasiko trace IDs and response-checked token counts. Nasiko recorded **$0.049993**
across those six calls, using pricing it marks **estimated**. The older server
misses context-suffixed prices and falls back to a static rate. Verified cost per
attempt and per resolution therefore remain **N/A**; no estimate is presented as
verified spend. The absent `/api/evals/*` routes are bridged by read-only,
owner/agent-scoped Postgres snapshots. Two verified-price insertions for the
working models are prepared in [pricing-lite-compat.sql](examples/pricing-lite-compat.sql),
awaiting operator approval. No pricing correction remains applied.

**Grading trust.** Prompts contain parent production source and an issue, with no
reference fix, hidden tests or revealing Git history. Candidate test/config
edits are rejected. Trusted tests run against candidate code with no network,
host credentials, Docker socket or writable checkout, under time/memory/process
limits. Missing/zero reports, omitted or skipped requirements and infrastructure
errors never resolve. Candidate compile/import failures count as unresolved.

**Checks and limits.** 36 focused unit tests, five Docker adapter fixtures,
three real repeatability gates, Ruff lint/format, compilation and editable CLI
installation passed. Native Rust workspace checks could not run because the host
has no Cargo; the Cargo adapter passed in Docker. Rust/Jest have fixture coverage,
not real model-attempt demonstrations. This is one-shot file replacement, with
limited package-manager support and no Rust inline-test extraction, independent
task screening, broad compatibility claim or adversarial result-forgery defense.

**Artifacts.** [README and exact commands](README.md),
[measured report](results/report.md), [three-minute demo](DEMO.md).

# Three-minute demo

Run from the Nasiko repository root. Python 3.11+, Git and running Docker are
required for validation. Recorded evidence can be displayed without model calls.

## Before presenting

Run the five synthetic calculator demos together; allow time for image pulls and
dependency installation on a new machine:

```sh
python3 autoeval/run.py examples all
python3 autoeval/run.py verify --run autoeval/final
```

Expected: each calculator reports two regression tests and one existing test;
the final benchmark verifies three real tasks and six assigned model/task slots.
Individual calculator commands are in [the example guide](examples/calculators/README.md).
The synthetic command calls the ordinary shared validation pipeline and makes
no model requests. It is separate from the headline real benchmark.

## 0:00–0:35 — One pipeline, five toolchains

“AutoEval discovers a repository's test toolchain, separates a reference fix from
its regression tests, verifies the task, then asks models through Nasiko to fix
the parent. These five calculators are synthetic demonstrations.”

```sh
python3 autoeval/run.py --help
cat autoeval/evidence/calculators/report.md
```

Show `autoeval/examples/calculators/python/` and one other language folder.
Point out project manifests, the subtraction bug, tests and `reference.patch`.
The helper creates deterministic `buggy` and `reference-fix` commits; the standard
detector and evaluator handle them without separate language workflows.

## 0:35–1:05 — A verified real task

Open `autoeval/final/runs/iniconfig-bom/validation.json`, then
`artifacts/before-1/stdout.log` and `artifacts/reference-1/stdout.log` in that run.

“Three upstream regression tests fail on old Python code and pass on the reference
fix. All 51 existing tests pass. We repeat both checks in fresh offline containers.
The model receives the issue and parent production code, without these hidden
tests, the reference implementation or Git history.”

## 1:05–1:45 — The frozen comparison

```sh
python3 autoeval/demo.py
```

“This one declared final cohort contains three real public tasks and two models.
Both models fixed Python and Go, and both failed TypeScript: two of three each.
One TypeScript patch regressed existing tests; the other failed compilation or
suite loading. Each assigned slot had one generation request. Synthetic fixtures
and development reruns do not enter these scores.”

## 1:45–2:15 — Exact recorded cost

Open `autoeval/final/runs/iniconfig-bom/model-1.json`; show `usage.trace_id`, token
counts and `usage.pricing` (`source=Exact`, `estimated=false`).

“The approved exact prices are applied and source-verified. Generation cost totals
are $0.016999 for 3.1 Flash-Lite and $0.025345 for 3.5 Flash-Lite. Preflight overhead
is separately $0.000081. Amounts come from Nasiko's attributable ledger records.”

## 2:15–2:45 — Show the grading limit honestly

Open `autoeval/evidence/security/before/security.json` and `after/security.json`.

“We reproduced a candidate replacing the old writable report and log, then forcing
exit zero. It incorrectly resolved before hardening. Reports now stream to the
host, and the same attack is rejected with a phase/exit mismatch. Framework and
stdout manipulation remain possible: grading is not tamper-proof.”

## 2:45–3:00 — Verify and reproduce

```sh
python3 autoeval/run.py verify --run autoeval/final
```

“We retain exact commits, Git bundles, pinned runtimes, dependency inventories,
prompts, patches, raw test captures and usage. Verification checks file hashes and
recomputes results from explicit membership. Three selected public tasks support
these observed outcomes, not a general model ranking.”

The [README](README.md) has commands for unit tests, an independent security
regression, individual calculator demos and a fresh paid benchmark replay.

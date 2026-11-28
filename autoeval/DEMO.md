# Three-minute demo

Run commands from the Nasiko repository root. Clones and recorded evidence from
this session are available at the paths below. The demo reads actual saved runs;
the README has commands to reproduce generation and Docker grading.

**0:00–0:25 — The workflow.**

“AutoEval turns a repository fix commit into a verified coding task, asks models
through Nasiko to fix the parent, and grades their production changes. One shared
workflow handles four language families; a repository is an input.”

```sh
python3 autoeval/run.py --help
```

Point out `inspect`, `validate`, `compare`, `report`.

**0:25–0:50 — Automatic environment discovery.**

```sh
python3 autoeval/run.py inspect --repo /tmp/nasiko-autoeval-iniconfig
```

Show manifest evidence, pytest, Python runtime, test directory, setup commands,
result format and limits. Explain that Go, Jest/Vitest and Cargo supply the same
schema; `--unit` selects a monorepo project and `--config` resolves ambiguity.

**0:50–1:20 — A trustworthy task.**

Open `autoeval/results/iniconfig-lite-models/validation.json`. Show the three
fail-to-pass identities and 51 existing passing requirements. Open
`artifacts/before-1/output.log` and `artifacts/reference-1/output.log` beside it.

“These upstream tests fail on the old implementation and pass with the reference
fix, twice in clean offline containers. Setup failures cannot establish a task.
The model never receives this hidden test patch or the reference implementation.”

**1:20–2:00 — Real measurements across languages.**

```sh
python3 autoeval/demo.py
```

“Three real tasks are verified: Python, Go and TypeScript. The two Lite models
attempted the same three tasks once each. 3.1 Flash-Lite resolved two; 3.5 resolved
one. Both fixed Go; only 3.1 fixed the BOM bug. Both failed TypeScript. The grader
also caught existing-suite regressions, and rejected a replacement that could
not compile. A separate Flash timeout stays visible as infrastructure failure.”

**2:00–2:35 — Cost honesty and grading isolation.**

Open `autoeval/results/report.md`, then one `model-1.json` in a completed run.
Point out response token counts, Nasiko trace ID, `pricing.source=Static` and
`estimated=true`.

“Usage and ledger amounts came from Nasiko. Its older image is using estimated
rates for these IDs, so verified cost metrics remain N/A. We have prepared the
price-book correction and have left it unapplied. Grade containers have no
network, credentials or Docker socket, and cannot modify the checkout.”

**2:35–3:00 — Scope and next step.**

Show `autoeval/SUBMISSION.md` and `autoeval/README.md`.

“All adapters have Docker fixture coverage; Python, Go and TypeScript have real
upstream task validation and model attempts. This is one-shot generation. Three
selected public tasks cannot establish a general model ranking, and verified
cost comparison is blocked by pricing. The next step is approved exact pricing
and a fresh strict comparison without `--allow-cost-gaps`.”

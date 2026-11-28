# Nasiko AutoEval

One Python CLI evaluates repository fixes across Python/pytest, JavaScript/Jest,
TypeScript/Vitest, Go and Rust/Cargo. Discovery adapters feed one execution plan;
Docker execution, validation, model attempts, grading and reporting are shared.
The existing prebuilt Nasiko server stays running; no frontend/server rebuild is
needed. Model attempts are one-shot production-file replacements through Nasiko.

## Requirements and quick checks

Python 3.11+, Git and Docker Desktop/Engine. Live comparisons also need Nasiko at
`http://localhost:8080`. The CLI has no third-party Python runtime dependencies.
Run everything from the Nasiko repository root:

```sh
python3 autoeval/run.py --help
PYTHONPATH=autoeval/src python3 -m unittest discover -s autoeval/tests -v
python3 -m compileall -q autoeval/src
```

The current suite has 49 unit tests, including checks that scores follow captured
test evidence and remain independent of model labels. The preserved Docker checks
cover five calculator validations, the before/after security regression and the
three real repeatability gates. Ruff lint/format and compilation passed. The
recorded `cargo fmt`, `cargo check --workspace` and `cargo clippy --workspace`
checks could not run because host Cargo was unavailable. The Rust calculator was
compiled and tested with Cargo inside Docker.

Optional installation: `python3 -m venv autoeval/.venv`, then
`autoeval/.venv/bin/pip install -e autoeval`. This installs the `autoeval` command.
The existing root `.env` supplies login credentials in memory; `NASIKO_TOKEN` or
`NASIKO_USERNAME`/`NASIKO_PASSWORD` can override them. Provider keys stay on the
server. No credentials are supplied to tested code or written to reports.

## Five synthetic calculator demos

Each folder under [examples/calculators](examples/calculators/README.md) contains
project configuration, a buggy `add(a, b)` that subtracts, three tests, and portable
inputs for a deterministic two-commit Git history. The helper prepares those
histories in `.cache`, automatically detects the toolchain, and invokes the
ordinary shared `validate` workflow. No model/server credentials are needed.

One command per example:

```sh
python3 autoeval/run.py examples python
python3 autoeval/run.py examples javascript
python3 autoeval/run.py examples typescript
python3 autoeval/run.py examples go
python3 autoeval/run.py examples rust
```

One command for all five:

```sh
python3 autoeval/run.py examples all
```

Expected for every example: **2 fail-to-pass regression tests and 1 existing
passing test**, each checked twice before/after. These are synthetic adapter demos,
not real repository model-evaluation results. Their output goes to `synthetic-runs/`
and they cannot enter the frozen real benchmark. Use `--run NEW_DIRECTORY` to
choose another output location. Recorded checks are in [evidence/calculators](evidence/calculators/report.md).

## Frozen real repository benchmark

The single final dataset is declared in [benchmark.json](benchmark.json):

- Iniconfig UTF-8 BOM parsing: `d2dcb7289291e7e7019c183659814d42f7264b2d`.
- go-humanize non-finite SI values: `1b638b2c0d9b358cbae4b51b74f136d9a6055d1a`.
- Pathe UNC resolution: `b52fcacc59717a1e4a8783d4bbfae7fa81b4db07`.

Both named Lite models get one generation request per task, identical source,
issue and output budget within each task. Python/Go use 4,096 output tokens;
TypeScript uses 8,192. Availability preflights can retry at most three times;
generation requests are not retried. All failures remain in evidence. A missing
model generation retains its assigned slot in the resolution denominator.

View and verify the final evidence:

```sh
python3 autoeval/run.py verify --run autoeval/final
python3 autoeval/run.py report --run autoeval/final
python3 autoeval/demo.py
```

Run the same declared benchmark afresh (this makes model requests):

```sh
python3 autoeval/run.py benchmark --manifest autoeval/benchmark.json \
  --run /tmp/autoeval-benchmark-new --postgres-container nasiko-postgres-1
```

Replay with the preserved Git bundles and pinned execution configurations:

```sh
python3 autoeval/run.py benchmark --manifest autoeval/final/manifest.json \
  --source-bundles autoeval/final/inputs \
  --run /tmp/autoeval-benchmark-replay --postgres-container nasiko-postgres-1
```

Every benchmark output must be a fresh directory. `planned.json` records membership
and evaluator hashes before generation. `benchmark.lock.json` seals the chosen
runs, source bundles, prompts, responses, candidates, raw captures, per-test results
and metrics. Verification recomputes metrics from explicit members and re-parses
raw captures. It detects edited/missing evidence. The checksums are integrity
checks, not signatures from an independent authority. Model responses can vary
between fresh runs; dependency inventories record what was installed, and setup
still downloads dependencies.

## Evaluate another repository

```sh
python3 autoeval/run.py inspect --repo /path/to/repo
python3 autoeval/run.py validate --repo /path/to/repo --commit FIX_SHA
python3 autoeval/run.py compare --repo /path/to/repo --commit FIX_SHA \
  --issue-file /path/to/issue.txt \
  --model gemini:gemini-3.1-flash-lite --model gemini:gemini-3.5-flash-lite \
  --postgres-container nasiko-postgres-1
```

`--unit directory:framework` selects a detected monorepo unit. `--config JSON`
overrides trusted execution settings. The [Pathe configuration](examples/pathe-plan.json)
is a frozen pnpm example using Vitest's native config loader. Standalone runs go
under `runs/`; headline reporting accepts only sealed benchmark membership and
never discovers runs by scanning folders. Synthetic and development runs remain
outside headline aggregation, wherever their files are placed.

## Grading and report integrity

The task builder exports a selected fix and its single parent, splits whole test
and production files, and rejects ambiguous manifest/execution changes, renames,
deletions, symlinks, changes across units and mixed inline Rust/Vitest tests.
Validation requires stable failures on old code and passes on reference code,
twice each in fresh offline containers. Build/setup failures do not reproduce bugs.

Models receive only an issue and parent production source. Reference fixes,
hidden regression tests and Git history stay outside their input. Candidate
changes to tests, configuration and unlisted paths are rejected. Resolution
requires every expected regression and existing test to pass, with no missing,
zero-collected or skipped requirements. Candidate compile/import failures are
unresolved; timeouts and infrastructure failures never resolve.

**The original report/log-forgery attack was reproduced and blocked after
hardening. Grading is not tamper-proof.** The new collector stores stdout/stderr on
the host, never in a candidate-writable report directory. Containers run the test
command as PID 1 and terminate its descendants on exit. Pytest emits phase events;
the host rejects missing/duplicate/out-of-order records, and a reported finish
status that disagrees with the observed container exit. Jest/Vitest send JSON to
stdout; Go/Cargo share the bounded host capture. Old writable-report overrides
are rejected. See [SECURITY.md](SECURITY.md) and the before/after regression evidence.

Candidate code still runs inside test frameworks and can potentially manipulate
framework state or forge process output. Run tokens scope events; they are not
secrets or cryptographic authentication. These tests verify the filesystem/report
boundary against the documented attacks, not arbitrary adversarial correctness.

Grade containers have no network, Docker socket, host credentials or writable
checkout. Default limits: UID 10001, all capabilities dropped, no privilege gain,
1 CPU, 1 GiB memory, 256 processes, 180 seconds, 512 MiB scratch, and 4 MB of captured
output. Setup has a separate 600-second limit with network for dependencies.
Temporary containers/images are removed on success, error and interruption;
abrupt daemon/host death can require manual cleanup of AutoEval-labelled objects.

## Exact cost and the older server

The prebuilt image lacks `/api/evals/usage`, `/api/evals/price` and the `eval`
integration. Its supported `opencode` identity is used for authenticated router
calls, with a local serialization lock and restoration of the previous binding.
`--postgres-container` reads owner/agent-scoped `trace_usage.router_*` snapshots
using `BEGIN READ ONLY`. Exactly one new row must match model and response token
counts; ambiguous concurrent rows or missing usage remain gaps. This bridge uses
this checkout's `nasiko` / `nasiko_dev` Compose database.

The two exact native-model price entries have now been added using Google's
[source-verified Standard text rates](https://aistudio.google.com/docs/pricing).
[Pricing evidence](evidence/pricing/source.json) records sources, rates, validity
and database row IDs. Existing prices and historical usage were not rewritten.
The [idempotent SQL](examples/pricing-lite-compat.sql) is an explicit operator
action and is never run by the CLI. No restart was required.

The previously approved Hydrogen / 3.8 Flash rows are also applied and recorded
in [separate pricing evidence](evidence/pricing/approved-compat.json). Those models
are outside the declared final cohort. These compatibility rows require review
before their January 1, 2027 expiry; Flash promotional rates change then.

Strict comparisons accept only `Exact`, non-estimated, attributable Nasiko costs.
Reports take dollar amounts directly from the ledger; they do not reconstruct
spend from token estimates. Preflight costs are separate overhead. Failed calls
without provider usage remain unknown. `--allow-cost-gaps` is an explicit option
for non-final exploratory runs; the final benchmark does not use it.

## Remaining limits

Support is deliberately narrow: pip, npm, Go modules and Cargo. Python records
`uv.lock` but does not consume it; SCM-versioned packages use a neutral metadata
version when Git is absent. pnpm/yarn and system dependencies need explicit setup
overrides. Parent source must fit the configured context budget. New production
files, inline-test extraction and arbitrary workspace build systems need more
work. Public-task selection bias and possible model training overlap are
uncontrolled. The small final sample supports its recorded outcomes, not a
general model ranking. Native Rust workspace checks need Cargo on the host;
the Cargo adapter itself is exercised inside Docker.

# Nasiko AutoEval

One standalone Python workflow evaluates repository bug fixes across Python/pytest,
JavaScript/Jest, TypeScript/Vitest, Go, and Rust/Cargo. It connects to Nasiko for
generation, model pinning, token usage, and recorded cost. It needs no new frontend,
main-server build, session ingestion, service restart, or provider key in a sandbox.

This is **one-shot production-file generation**, not an interactive coding-agent
replay. All models receive identical issue text, parent source, temperature, and
output/context budgets. JSON replacement files are the patch transport; the grader
applies them only to allowed existing production files.

## Run

Requirements: Python 3.11+, Git, Docker CLI and daemon, network access for initial
toolchain/dependency downloads, and an authenticated Nasiko server. The CLI has no
third-party runtime Python dependencies. Tested on this macOS checkout with Docker
Desktop. Linux is supported by the same CLI; Windows needs WSL (`fcntl` is used).

Run from the Nasiko repository root without installing:

```sh
python3 autoeval/run.py --help
python3 autoeval/run.py inspect --repo /path/to/repository
python3 autoeval/run.py validate --repo /path/to/repository --commit FIX_SHA
python3 autoeval/run.py compare --repo /path/to/repository --commit FIX_SHA \
  --model CONFIG_A --model CONFIG_B --postgres-container nasiko-postgres-1
python3 autoeval/run.py report --run autoeval/results
```

Optional installed entry point:

```sh
python3 -m venv autoeval/.venv
autoeval/.venv/bin/pip install -e autoeval
autoeval/.venv/bin/autoeval --help
```

By default authentication reads only the existing root `.env` into memory and
logs in through `POST /api/auth/login`. `NASIKO_TOKEN` or
`NASIKO_USERNAME`/`NASIKO_PASSWORD` can replace the admin login. Use `--env-file`
and `--server-url` for another environment. Never put credentials in arguments.
No credentials or encryption settings are written to artifacts or containers.

`--model` accepts a pinned Nasiko config name/ID. A `provider:model` selector
creates/reuses an `autoeval-*` config with a pin and no fallbacks after checking
Nasiko's price catalog. Availability is checked by real routed generation; a
catalog entry alone does not prove that a model can serve this account.

## Reproduce the real repository tasks

These are public upstream bug-fix commits, not authored benchmark fixtures.

```sh
git clone https://github.com/pytest-dev/iniconfig.git /tmp/nasiko-autoeval-iniconfig
git clone https://github.com/dustin/go-humanize.git /tmp/nasiko-autoeval-go-humanize

python3 autoeval/run.py inspect --repo /tmp/nasiko-autoeval-iniconfig
python3 autoeval/run.py validate --repo /tmp/nasiko-autoeval-iniconfig \
  --commit d2dcb7289291e7e7019c183659814d42f7264b2d \
  --issue-file autoeval/examples/iniconfig-issue.txt

python3 autoeval/run.py compare --repo /tmp/nasiko-autoeval-iniconfig \
  --commit d2dcb7289291e7e7019c183659814d42f7264b2d \
  --issue-file autoeval/examples/iniconfig-issue.txt \
  --model gemini:gemini-3.1-flash-lite --model gemini:gemini-3.5-flash-lite \
  --max-output-tokens 4096 --postgres-container nasiko-postgres-1 --allow-cost-gaps

python3 autoeval/run.py validate --repo /tmp/nasiko-autoeval-go-humanize \
  --commit 1b638b2c0d9b358cbae4b51b74f136d9a6055d1a \
  --issue-file autoeval/examples/go-humanize-issue.txt
```

Keep existing clones and use fresh output paths for reruns. `--run DIRECTORY`
selects output; existing results are never overwritten by `validate`/`compare`.
`report` regenerates presentation from evidence. On a directory of runs it
aggregates real tasks separately from local adapter fixtures.

TypeScript/Vitest reproduction uses an explicit frozen pnpm setup:

```sh
git clone https://github.com/unjs/pathe.git /tmp/nasiko-autoeval-pathe
python3 autoeval/run.py compare --repo /tmp/nasiko-autoeval-pathe \
  --commit b52fcacc59717a1e4a8783d4bbfae7fa81b4db07 \
  --config autoeval/examples/pathe-plan.json \
  --issue-file autoeval/examples/pathe-issue.txt \
  --model gemini:gemini-3.1-flash-lite --model gemini:gemini-3.5-flash-lite \
  --max-output-tokens 8192 --postgres-container nasiko-postgres-1 --allow-cost-gaps
```

## Workflow and grading boundary

1. Export the chosen commit and its single parent using Git's tracked-file
   archive. The host working tree is untouched. Exclude history and credential
   files, reject symlinks, and discover units from manifests/configuration.
2. Select one unambiguous unit (`--unit .:pytest`, `--unit packages/lib:vitest`).
   All adapters emit the same `ExecutionPlan`: language, framework, image, cwd,
   setup, test command, format, test rules, limits, runtime requirements and
   discovery evidence. `--config JSON` overrides trusted execution settings.
3. Separate whole production and test files. Reject merge commits, renames,
   deletions, changes across units, execution/manifest changes, unclassified
   files and mixed Rust inline-test changes. Documentation changes are ignored.
4. Prepare dependencies online in a bounded container, then snapshot them.
   Setup has no host mounts, credentials, Git history or Docker socket.
5. Validate old production plus trusted regression tests twice, and reference
   production plus the same tests twice. Each is a fresh offline container.
   Every accepted task needs stable, identical test identities; real test
   failures must become passes. Setup/build/collection failures do not reproduce
   a bug. Capture all previously passing tests as pass-to-pass requirements.
6. Send only the issue and allowed **parent production source** to Nasiko. The
   prompt has no reference patch, hidden tests, fixed commit contents, Git log,
   changed-file shortlist, or shell/tool access. The issue defaults to the commit
   subject; `--issue-file` supplies a clearer problem statement without a solution.
7. Reject candidate changes outside the allowlist, including test/config edits.
   Overlay allowed replacements on old production plus evaluator-controlled
   tests. Run the same plan offline and require every expected regression and
   existing test to pass. Missing/zero/malformed reports, missing identities,
   skips of required tests, timeouts and infrastructure errors cannot resolve.
8. Save the task SHAs, exact per-test results, plan, image IDs, prompt hash,
   response, candidate, logs, duration, token counts, pricing provenance and
   attributable Nasiko cost. Report both total resolution rate and separate
   infrastructure counts. Cost per resolution is N/A when nothing resolves.

Grade containers run as UID 10001 with network disabled, a read-only root/repo,
all capabilities dropped, no privilege gain, and default limits of one CPU,
1 GiB memory, 256 processes, 180 seconds, 512 MiB scratch and 32 MiB report space.
Stdout/stderr are captured together with bounded retained output. Setup has a
separate 600-second limit. Containers and temporary images are removed on exit,
error or interrupt. Toolchain base images remain cached. Abrupt host/daemon death
cannot execute cleanup; only AutoEval-labelled containers may be cleaned manually.

## Models and cost on the pinned prebuilt image

The running image at revision `2ec34192e0ff2c5e9422edc17d1d49e0db7595ce` lacks
the newer `/api/evals/usage`, `/api/evals/price` and `eval` integration. This CLI
detects that capability gap; it does not assume the checkout's routes are live.

- Newer servers use the `eval` integration, `x-nasiko-eval-attempt`, and the eval
  usage API. That compatibility branch has focused tests, not a live-server demo.
- This checkout uses the supported `opencode` identity and `POST
  /api/agents/{id}/llm-token`. A local process lock serializes runs. The previous
  model binding is restored when the client exits; no default config is changed.
- `--postgres-container nasiko-postgres-1` reads `trace_usage.router_*` using
  `BEGIN READ ONLY`, selecting only the authenticated owner and agent. It accepts
  exactly one new ledger row, matching the served model and response tokens.
  Multiple concurrent rows, missing usage or estimates produce an explicit gap.
  It reads `nasiko` / `nasiko_dev`, matching this Compose checkout.
- Catalog prices come from `GET /api/llm-router/providers`. Cost is the server's
  recorded amount, **never locally calculated or reconstructed**. Rate provenance,
  trace ID and response counts remain in the evidence for audit.
- The old engine cannot match the current context-suffixed prices for newer
  Gemini IDs and uses a `Static` estimated rate. Normal preflight refuses this.
  `--allow-cost-gaps` explicitly permits correctness measurements while retaining
  **N/A** for unverified cost. Raw estimated ledger entries are labelled as such;
  they are excluded from totals presented as verified cost.
- [pricing-compat.sql](examples/pricing-compat.sql) is an optional, insert-only
  operator correction with Google's published Standard rates and source links.
  It is **never applied by the CLI**, changes no existing prices and needs no
  server restart. Database changes require the operator's approval.
- The working Lite configurations have a separate prepared correction,
  [pricing-lite-compat.sql](examples/pricing-lite-compat.sql), which has not been
  executed. The earlier Hydrogen/3.8 rows were briefly inserted, then both
  removed; no AutoEval pricing correction remains in the running database.

Google's model listing included retired Gemini 2.5 models that returned 404 for
generation. Hydrogen also returned 503 high-demand errors during this session.
Preflight errors are shown separately, not described as failed model fixes.

## Support and checks

| Toolchain | Discovery/parsing | Fresh Docker fail/pass fixture | Real repository validation |
| --- | --- | --- | --- |
| Python / pytest | Implemented, unit tested | Verified | Iniconfig: 3 fail-to-pass, 51 pass-to-pass |
| JavaScript / Jest | Implemented, unit tested | Verified (Jest 29.7) | Not yet demonstrated |
| TypeScript / Vitest | Implemented, unit tested | Verified (Vitest 2.1) | Pathe: 10 fail-to-pass, 457 pass-to-pass; 16 optional skips |
| Go / go test | Implemented, unit tested | Verified | go-humanize: 1 fail-to-pass, 68 pass-to-pass |
| Rust / Cargo | Implemented; follows existing binary/test identity contract | Verified with separate integration tests | Not yet demonstrated |

Real model-attempt outcomes and cost coverage are in [results/report.md](results/report.md)
and [results/results.json](results/results.json); fixture outcomes are not benchmark scores.
The real model attempts cover Python, Go and TypeScript. Each model received the
same budget on each task: 8,192 output tokens for the larger TypeScript source
and 4,096 for Python/Go.

```sh
PYTHONPATH=autoeval/src python3 -m unittest discover -s autoeval/tests -v
python3 -m compileall -q autoeval/src
AUTOEVAL_SMOKE_OUTPUT=/tmp/autoeval-smoke-new PYTHONPATH=autoeval/src \
  python3 autoeval/tests/docker_smoke.py pytest jest vitest go cargo
```

The Docker checks create tiny local two-commit fixtures and label them
`local_fixture`. They exercise the same validator as real repositories, without
using a model. Installation and the `autoeval` entry point were checked in a
fresh temporary virtual environment. Native Rust workspace checks cannot run on
this host because `cargo` is absent; the Cargo adapter was tested in Docker.

## Limits

Package-manager support is deliberately small: pip, npm, Go modules and Cargo.
Lockfiles and runtime requirements are recorded; npm honors its lockfile, and
Go/Cargo run offline after fetching. Python does not yet consume `uv.lock`.
SCM-versioned Python packages use a neutral build-metadata override because Git
history is absent; strict version-dependent setup needs an explicit override.
pnpm/yarn need explicit setup commands; [pathe-plan.json](examples/pathe-plan.json)
is a frozen pnpm example. Vite configurations that bundle themselves into the
repository need a compatible native config loader or an explicit override.
Custom JS test-path rules also need overrides.

Parent source must fit `--context-bytes` (default 80,000); no model-dependent
truncation is performed. New production files, binary changes, Rust inline test
extraction, system dependency provisioning, network/database-dependent test
suites and arbitrary workspace build systems need further work. Reports establish
correctness only for the selected suite, not all possible behavior. Containers
provide process/resource isolation; this buildathon does not claim resistance
to deliberately adversarial repository code or forged result emitters.

Public-commit selection, possible training overlap, no independent task screening,
and a tiny sample preclude a general model-quality or cost ranking. See
[SUBMISSION.md](SUBMISSION.md) and [DEMO.md](DEMO.md).

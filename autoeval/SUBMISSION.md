# Nasiko AutoEval — buildathon submission

A standard-library Python CLI turns repository fix commits into verified coding
tasks, requests one-shot production changes through Nasiko, and grades the result.
Python/pytest, JavaScript/Jest, TypeScript/Vitest, Go and Rust/Cargo all produce one
execution plan and reuse task extraction, Docker execution, validation, grading
and reporting. The existing prebuilt Nasiko server stays running.

The final real benchmark is prospectively declared in
[benchmark.json](benchmark.json) and sealed in [final/](final/report.md). Every task
failed on its parent and passed on the reference fix twice in fresh offline
containers. Each model received one generation request per task, identical source,
issue and output budget within that task, at temperature zero.

| Public task | Fix commit | Regression / existing tests |
| --- | --- | ---: |
| pytest-dev/iniconfig, Python BOM parsing | `d2dcb7289291` | 3 / 51 |
| dustin/go-humanize, Go non-finite SI values | `1b638b2c0d9b` | 1 / 68 |
| unjs/pathe, TypeScript UNC resolution | `b52fcacc5971` | 10 / 457 |

Pathe also has 16 optional skipped/todo tests, excluded from passing requirements.
Python/Go use 4,096 output tokens; TypeScript uses 8,192. Availability preflights
can retry up to three times; generation is never retried. All six assigned slots
are included. Development reruns and synthetic fixtures cannot enter the headline
aggregation through directory scanning.

| Model | Resolved | Exact recorded generation cost |
| --- | ---: | ---: |
| Gemini 3.1 Flash-Lite | 2/3 (66.7%) | $0.016999 |
| Gemini 3.5 Flash-Lite | 2/3 (66.7%) | $0.025345 |

Both fixed Python and Go; both failed TypeScript. The grader caught existing-suite
regressions in 3.1's patch and a compilation/loading failure in 3.5's patch. Total
generation cost was **$0.042344**, with **$0.000081** separate preflight overhead
and no failed/retried preflights. These are Nasiko ledger costs at Google's
[verified Standard text rates](evidence/pricing/source.json), not provider invoice
reconciliation. Exact native-model entries are applied; every final cost has
`source=Exact`, `estimated=false`, an attributable trace and response-checked
usage. The older server's missing evaluation endpoints are bridged with read-only,
owner/agent-scoped Postgres snapshots.

The real benchmark used six live generation requests through Nasiko's
`/v1/chat/completions`, one per model/task. Each `final/runs/*/model-*.json`
retains the response ID, served model, generated content, provider token counts,
attributable trace and ledger cost; `candidate-*.json` retains the parsed changes.
The [six-generation ledger audit](evidence/pricing/final-ledger-audit.json)
records matching response usage and exact, non-estimated costs. `demo.py` and
`verify` inspect these recordings without making new model calls. This is
preserved local provenance, not independently signed provider attestation.

Scores are computed from test evidence, never assigned by model name.
`results.grade` takes a test run and verified test requirements, with no model
argument. Every required regression and existing test must pass; missing tests,
skips, suite failures and infrastructure errors cannot resolve a task.
`benchmark.verify_benchmark` re-parses raw captures, regrades candidates and
checks the recomputed aggregation against the sealed results. Model names only
group attempts in `report.summarize`. Unit checks also rename model labels and
change copied test evidence to verify that scores follow test outcomes.

The original writable-report attack was reproduced: buggy candidate code replaced
the report and log, forced exit zero, and incorrectly resolved. Host-side bounded
stream capture, read-only test execution and pytest phase/exit consistency checks
now reject that same attack; broken/fixed controls still behave correctly.
**Grading is not tamper-proof.** Candidate code shares framework execution and may
manipulate internals or forge stdout. [Security evidence and limits](SECURITY.md)
state exactly what was verified. Sandboxes have no network, credentials or Docker
socket during grading, with time/memory/process/output limits.

[Five synthetic calculator projects](examples/calculators/README.md) each include
configuration, a buggy add function, tests and deterministic reference-fix history.
All five pass repeated Docker validation with two regression tests and one existing
test each. They demonstrate adapter coverage, use no model calls, and remain
separate from real benchmark scores.

The calculator bug, reference patch and two-commit history are deterministic
synthetic fixtures. Directly running regression tests on their buggy source is
expected to fail. Use `python3 autoeval/run.py examples all` to validate both code
versions through the shared Docker workflow; it does not ask an AI to fix them.

Reproducibility evidence retains source Git bundles, pinned image digests,
dependency inventories, prompts, responses, candidates, raw captures, normalized
tests and ledger records. `verify` checks sealed hashes, recomputes membership and
metrics, and re-parses captures. Checksums are unsigned. The small selected public
sample, possible training overlap, narrow package-manager support and one-shot
file replacement limit generalization. Host Cargo is unavailable; Cargo adapter
validation passed inside Docker.

[Run and test commands](README.md) · [Three-minute demo](DEMO.md) ·
[Final report](final/report.md)

Run the final submission checks from the repository root (Python 3.11+ and Git;
these commands make no model calls and do not require a running server or Docker):

```sh
PYTHONPATH=autoeval/src python3 -m unittest discover -s autoeval/tests -v
python3 autoeval/run.py verify --run autoeval/final
python3 autoeval/demo.py
```

Finalization checks passed: 49 unit tests; sealed verification of three real tasks
and six assigned slots; demo output with both models at 2/3 and generation costs
of $0.016999 / $0.025345 respectively; Ruff lint/format and Python compilation.
All 191 files under `final/` and `evidence/`, including 85 sealed artifacts,
remain byte-for-byte unchanged. The original Docker validations are retained;
finalization did not repeat paid model generation or Docker evaluation.
A read-only recheck against the running Postgres ledger also matched all six
frozen trace IDs, served models, provider token counts and costs ($0.042344 total).

To exercise live models again, run the paid replay into a fresh output directory
with Nasiko, Docker and valid server-side provider credentials available:

```sh
python3 autoeval/run.py benchmark --manifest autoeval/final/manifest.json \
  --source-bundles autoeval/final/inputs \
  --run /tmp/autoeval-submission-replay --postgres-container nasiko-postgres-1
```

A fresh replay can produce different patches and scores. Preserve `final/` and
`evidence/`; the submission consists of this document, `DEMO.md`, `demo.py`, the
README, CLI source/tests, benchmark manifest, synthetic examples and both evidence
directories, including `final/report.md` and `final/benchmark.lock.json`.

# Run AgentKV

The benchmark launches an authenticated Nasiko server and three original A2A agents in a disposable Modal CPU VM. Nasiko injects agent identities and routes their model calls to a private gateway in front of Qwen/vLLM on an A100 80 GB. The coding coder uses Pi as its harness inside that worker; tester and reviewer stay Nasiko A2A steps. AgentKV still owns prefix compilation and eviction policy. Jev remains a TypeSafe service. Tempo runs inside the VM for native Nasiko traces. The public Vercel desk shows recorded evidence independently of GPU compute.

## Prerequisites

Authenticate `gh` and Modal locally. Run `uv sync --frozen --extra infra`. Store the Jev credential in Modal Secret `agentkv-jev` under `TYPESAFE_API_KEY`; never put it in source or command history. The configured model volume is `agentkv-model-weights`. Download the pinned model once with:

```sh
uv run modal run infra/model.py --mode download
```

The dependency image is built from the unmodified pinned Nasiko source. Its image tar is cached in `agentkv-build-cache`; it contains no database or login credentials. Model and dependency volumes persist across compute teardown.

## Integration and benchmark

```sh
# First, a small real integration screen.
uv run modal run -m infra.experiment --limit 2 --methods A,B,C,D,E --concurrency 1 --repeats 1

# Natural-memory coding cohorts: run separately, with the same frozen task manifest.
# Natural-memory coding cohorts use Pi inside the Nasiko coder. Do not pool with single-shot coder runs.
uv run modal run -m infra.experiment --limit 20 --methods A,B,C,D,E --concurrency 1 --repeats 3 --show-ui
uv run modal run -m infra.experiment --limit 20 --methods A,B,C,D,E --concurrency 4 --repeats 3
AGENTKV_HARNESS=direct uv run modal run -m infra.experiment --limit 2 --methods A,B --concurrency 1 --repeats 1

# Idle-GPU warming is on for C–E when the engine is empty. Disable it for a labeled ablation.
AGENTKV_WARMING=0 uv run modal run -m infra.experiment --limit 4 --methods C,E --concurrency 1 --repeats 1

# Separate support workflow (8 tickets, not mixed into the coding Pareto panel).
AGENTKV_WORKFLOW=support uv run modal run -m infra.experiment --limit 8 --methods A,B,C,E --concurrency 1 --repeats 1

# Separate, deliberately constrained-memory mechanism check.
AGENTKV_CACHE_BYTES=1073741824 uv run modal run -m infra.experiment --limit 20 --methods A,B,C,D,E --concurrency 4 --repeats 3

uv run python benchmarks/analyze.py results/integrated-latest.json
```

These are execution instructions, not evidence that every matrix cell has completed. Consult `BENCHMARK_RESULTS.md` for actual completed runs. Each invocation saves a timestamped report and `integrated-latest.json`; archive the timestamped report before analyzing another run. Do not merge different cache budgets into one frontier. Methods are randomized per repetition with a fixed seed. Every trial drains its finite task batch before a reset. Metric-flush waits are outside the measured trial window and remain part of the broader resource window.

A uses stock prefix caching and task-first prompts. B moves unchanged repository or knowledge-base context into the system prefix. C adds deterministic eviction preferences; D uses a calibration repair frequency; E uses two timely Jev Noul estimates (repair, later prefix reuse) and may prefill the exact next LLM prompt while the GPU is idle. B–E use identical prompts, model settings and cache budgets. All methods have the same adapter instrumentation; A/B leave eviction priorities disabled.

The pilot contains 20 authored pure-Python utility repairs in four fixture repositories, in fixed round-robin repository order. Four separate calibration tasks provide a repair frequency and a linear prefill-cost estimate. Repository tests are visible context. A second, smaller support workflow uses eight tickets and one authored knowledge base; report it separately. This is a small engineering pilot, not SWE-bench or a general capability evaluation.

## Inspect the Nasiko demo

`--show-ui` prints a temporary TLS URL for Nasiko and saves its generated login privately to `results/demo-access.json` (mode 0600, gitignored). The URL works only while the run is active. Use Nasiko's Agents, Flows and Observability pages to inspect deployed workers, authenticated invocations and traces. An optional `--hold-seconds 600` keeps the bounded demo available after results are written; the maximum hold is 900 seconds.

The runner applies deterministic workflow routing, calling every agent through Nasiko. It does not claim that Nasiko's semantic orchestrator chose the next agent. AgentKV's companion shows request-level cache usage directly from vLLM because the pinned Nasiko response schema drops nested cache-usage details.

## Recorded dashboard

```sh
uv run modal deploy infra/dashboard.py
uv run modal run infra/dashboard.py --path results/integrated-latest.json
```

The public dashboard exposes a sanitized projection of the authored benchmark, never service credentials, source prompts or generated code. It marks recorded results explicitly. Its Pareto line connects observed non-dominated trial points within one concurrency cohort; it is not a statistical superiority claim. Unknown costs stay unknown.

## Bounds and teardown

Each experiment has at most one A100 container and one 4-CPU/8-GiB VM. The GPU subprocess has a 3,300-second ceiling, the guest command 3,450 seconds, and the VM/function 3,600 seconds. The runner terminates the VM in `finally`; ending `modal run` stops its GPU service. Do not detach benchmark runs. If interrupted, verify with `modal app list` and stop a remaining experiment with `modal app stop APP_ID`.

Use `uv run modal billing report --for today --resolution h --json` for reported workspace charges. Reports can lag. Keep $10 of the $50 budget reserved. Requested-resource estimates in benchmark reports are separate from actual app-attributed charges and exclude items listed in their ledger. Delete `results/demo-access.json` after a demo if you no longer need the expired login record.

## Supported boundaries

This is one owner, one Nasiko VM and one engine. The gateway assigns the cache namespace server-side. Retention changes only the free-block eviction queue, never reference counts or active allocations. A lease is an eviction preference, not a hard memory pin. The adapter is pinned to vLLM 0.29.0 and checks complete hybrid cache groups and their current hashes before acting. Missing state, expiry or decision failures preserve stock inference behavior. Streaming is deliberately excluded from this bounded benchmark service.

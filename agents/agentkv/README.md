# AgentKV

Workflow-aware inference optimization through Nasiko's control plane.

Nasiko deploys the coding team, authenticates its calls, pins Qwen in its model router and exposes execution traces. The coding coder step runs [Pi](https://pi.dev/) inside that Nasiko worker and still talks to Qwen through Nasiko's injected OpenAI endpoint. AgentKV compiles stable prompts and applies bounded eviction preferences to complete reusable vLLM cache groups. Jev supplies optional repair and prefix-reuse estimates; deterministic code controls memory priority, expiry and idle-GPU warming of an exact later prompt.

The entire operated stack runs on Modal: Nasiko, its backing services, A2A workers, Qwen3.8-27B/vLLM and the recorded-results dashboard. Jev remains TypeSafe-hosted. The support pilot is a separate eight-ticket cohort.

**Evidence, not a promised speedup:** integration has executed real coding/test/review workflows and verified prefix hits. Comparative results must separate prompt layout, retention and Jev, including costs, failures and uncertainty. See the benchmark report for completed runs; a high cache-hit rate alone does not demonstrate lower workflow cost.

## Run

```sh
uv sync --frozen --extra infra
uv run python -m unittest discover -s tests -v
uv run modal run -m infra.experiment --limit 2 --methods A,B,C,D,E --concurrency 1 --repeats 1 --show-ui
```

The [runbook](docs/RUNBOOK.md) covers credentials, the pinned model download, bounded experiments, Nasiko's temporary UI, publishing recorded results and teardown. Real model runs incur cloud charges. Credentials stay in Modal Secrets and a gitignored, private temporary demo-access file.

## What gets compared

| Method | Behavior |
|---|---|
| A | Stock prefix caching; task-first prompts |
| B | Stock prefix caching; stable repository prefixes |
| C | B plus deterministic workflow retention |
| D | C with a disjoint-calibration repair frequency |
| E | C with two timely Jev Noul estimates; idle-GPU exact-prefix warm |

C–E share the same retention implementation, lease and quota. The adapter reorders free cache blocks; it never hard-pins memory or changes active references. Exact cache keys and all hybrid state groups remain engine-owned. The single-owner gateway assigns its cache namespace server-side.

The pilot uses 20 original, executable Python utility repairs, visible repository tests and four separate calibration tasks. It is not SWE-bench. Measured-window resource estimates, actual reported app charges, cold startup and one-token prefill probes are distinct evidence. External systems such as KVFlow, PBKV and LMCache are related work, not beaten baselines.

## Public site

The Next.js landing and metrics desk live in `web/`. Nasiko remains the live control plane on Modal; Vercel hosts the sanitized recorded-evidence companion.

```sh
cd web && npm install && npm run dev
```

Set `NEXT_PUBLIC_NASIKO_URL` only when a Nasiko demo is actually running. The desk never invents cache wins.

## Project documents

- [PRD issue #1](https://github.com/perfect7613/AgentKV/issues/1)
- [Product requirements](docs/PRD.md)
- [Runbook and supported boundaries](docs/RUNBOOK.md)
- [Controller API](docs/CONTROL_PLANE.md)
- [Provenance](docs/PROVENANCE.md)
- [Benchmark results](BENCHMARK_RESULTS.md)
- [Benchmark protocol](benchmarks/PROTOCOL.md)

No TurboQuant, SpectralQuant or reference-repository implementation is copied into AgentKV. Nasiko runs as an unmodified, pinned dependency. The engine extension requires vLLM 0.29.0; model weights use an immutable revision. This is a bounded, single-owner demonstration and benchmark service, not a general multi-tenant inference platform.

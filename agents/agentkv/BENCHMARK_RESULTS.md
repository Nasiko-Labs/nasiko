# Benchmark results

Two single-shot natural-memory coding cohorts and two Pi-harness coding cohorts ran on Qwen3.8-27B BF16, one A100 80 GB, vLLM 0.29.0, 20 authored utility repairs, methods A–E, three randomized repetitions, concurrency four. Do not pool their timings or costs. Pi tables are a separate harness. All figures are measured-window resource estimates, not reconciled Modal invoices.

## First cohort (task-first vs prefix, pre-compiler)

All 300 workflows passed. Raw evidence: `results/natural-c4-final.json` (local).

| Method | Meaning | Passed | Cached prompt tokens | p95 seconds | Estimated $/1,000 successes |
|---|---|---:|---:|---:|---:|
| A | stock cache, task-first prompt | 60/60 | 0% | 39.95 | 2.60 |
| B | stable repository prefix | 60/60 | 78.0% | 46.18 | 3.39 |
| C | B + retention policy | 60/60 | 78.0% | 47.58 | 3.52 |
| D | C + calibration frequency | 60/60 | 78.0% | 46.61 | 2.72 |
| E | C + Jev estimate | 60/60 | 78.0% | 48.45 | 3.58 |

A had the lowest measured p95 and estimated unit cost. Stable prefixes produced real reuse, and the adapter produced 45 acknowledged engine priorities across C, 44 across D and 46 across E, but those actions did not offset prompt-layout, generation and controller overhead. E issued 60 Jev decisions after the tester result (`timely_prediction_count: 0`). The paired E–A latency delta was +2.96 seconds with three trial means (bootstrap interval +1.90 to +4.01 seconds); E–C was +0.46 seconds (−1.20 to +1.61 seconds). All task outcomes were equal.

One-token probes from that run: A p50 0.709 seconds with no cached tokens; B p50 0.509 seconds with 72.0% cached prompt tokens. They are not coding workflows.

## Decode-neutral cohort (system prefix, timely Jev)

Ran after the decode-neutral compiler, timely two-Noul plan, and idle-GPU warming path. Modal app `ap-N5TMiWRHPgrwNRMKluhWNP`. Raw evidence: `results/decode-neutral-c4.json` and `results/run-1789897073-c4.json` (local). All 300 workflows passed. Zero engine preemptions. Warming skipped on every workflow at concurrency 4 (GPU was not idle). Calibration repair frequency 0.167 from four disjoint tasks, none of which repaired.

| Method | Meaning | Passed | Cached prompt tokens | Generated tokens | p50 seconds | p95 seconds | Estimated $/1,000 successes |
|---|---|---:|---:|---:|---:|---:|---:|
| A | stock cache, task-first prompt | 60/60 | 0% | 6418 | 25.03 | 45.60 | 3.04 |
| B | stable system prefix | 60/60 | 78.1% | 7986 | 22.28 | 50.12 | 3.10 |
| C | B + retention policy | 60/60 | 78.1% | 7930 | 22.73 | 50.80 | 3.13 |
| D | C + calibration frequency | 60/60 | 78.1% | 7944 | 22.88 | 50.68 | 3.14 |
| E | C + two timely Jev Nouls | 60/60 | 78.1% | 7919 | 23.12 | 50.85 | 3.17 |

A still has the lowest p95 and estimated unit cost. B–E still generate more tokens than A (~24%), so decode work is not yet matched even with a shared system prefix. B improved paired mean latency versus A by 1.12 seconds (bootstrap interval −1.17 to −1.03 seconds) and improved p50, but not p95 or unit cost. C, D and E each received 50–51 acknowledged engine priorities. C–E recorded 60/60 proposals before the tester completed. E issued 60 Jev requests (mean 0.33 seconds). Paired E–A mean latency was −0.48 seconds (−0.83 to +0.06 seconds); the interval includes zero. E–C was +0.07 seconds (−0.01 to +0.21 seconds). Timely repair Brier scores: D 0.028, C 0.25 (fixed 0.5), E 0.38 uncalibrated. No method repaired any task. All task outcomes were equal.

One-token probes from this run: A p50 0.460 seconds with no cached tokens; B p50 0.243 seconds with 72.0% cached prompt tokens. They remain serving-time probes, not workflow economics.

The live engine gate again acknowledged a retention action, reused 784 cached tokens, and kept an identical first output token. Known process-window resource estimate for this run is $1.32, excluding startup, teardown, storage, network and unreported charges. It is not a reconciled invoice.

This cohort still does not demonstrate an end-to-end AgentKV win. Cache hits and timely Jev coverage are real; they did not beat stock cache on p95 or estimated cost on this small, uncompressed-memory workload.

## Pi coding harness, first retry (do not pool)

Dockerfile OTel/`USER` fix. Modal app `ap-LLAZ4Uj2TkOXZnfY6l9gwz`. Raw evidence: `results/run-1789899452-c4.json` (local). All 300 records have `"harness": "pi"`. All 300 task tests failed. Coder events stored `usage: null` (600 completions). Reviewer/tester cache-usage: A 0%; B/D/E 70.5%; C 88.1%. Process-window estimate $1.00.

| Method | Meaning | Passed | Cached prompt tokens | Generated tokens | p50 seconds | p95 seconds | Estimated $/1,000 successes |
|---|---|---:|---:|---:|---:|---:|---:|
| A | stock cache, task-first prompt | 0/60 | 0% | 2663 | 22.80 | 35.67 | — |
| B | stable system prefix | 0/60 | 70.5% | 2301 | 17.79 | 30.84 | — |
| C | B + retention policy | 0/60 | 88.1% | 2315 | 18.51 | 32.02 | — |
| D | C + calibration frequency | 0/60 | 70.5% | 2275 | 17.42 | 30.54 | — |
| E | C + two timely Jev Nouls | 0/60 | 70.5% | 2306 | 18.57 | 31.76 | — |

## Pi coding harness, costing-fix retry (do not pool)

Pi catalog written to both `~/.pi/models.json` and `~/.pi/agent/models.json`, streaming forced off at the gateway with an SSE wrap back to Pi, usage indexed by client `request_id` and engine id. Modal app `ap-rRSPWgO6i6WQavuEfH6SOU`. Raw evidence: `results/run-1789901249-c4.json` and `results/integrated-latest.json` (local). All 300 records have `"harness": "pi"`. All 300 Pi processes exited 1. Coder usage remained null (1,200 coder events). Unit cost stays undefined: zero successes and incomplete coder usage. Do not pool with the tables above.

Reviewer/tester cache-usage is still real: A 0% cached prompt tokens; B/D/E 70.5%; C 88.1%. Generated-token totals 2.2k–2.8k per method across 60 workflows. All 60 A/B/C/D/E tasks attempted a repair. E issued 60/60 Jev requests (mean 0.31 seconds) with 60 timely predictions. C recorded 16 idle-GPU warms. Zero engine priority acknowledgements on the task batch. The live engine gate acknowledged a retention action, reused 784 cached tokens, and kept an identical first output token. Estimated process-window resources $0.94, not a reconciled invoice.

| Method | Meaning | Passed | Cached prompt tokens | Generated tokens | p50 seconds | p95 seconds | Estimated $/1,000 successes |
|---|---|---:|---:|---:|---:|---:|---:|
| A | stock cache, task-first prompt | 0/60 | 0% | 2766 | 21.97 | 33.29 | — |
| B | stable system prefix | 0/60 | 70.5% | 2184 | 16.87 | 29.02 | — |
| C | B + retention policy | 0/60 | 88.1% | 2196 | 18.47 | 30.49 | — |
| D | C + calibration frequency | 0/60 | 70.5% | 2204 | 18.52 | 30.50 | — |
| E | C + two timely Jev Nouls | 0/60 | 70.5% | 2197 | 17.12 | 29.74 | — |

B was 4.22 seconds faster than A on paired mean latency (bootstrap −4.57 to −3.77 seconds) and E was 3.72 seconds faster than A (−3.87 to −3.46 seconds). That is queue-inclusive time for workflows whose tests did not pass. One-token probes from this run: A p50 0.479 seconds with no cached tokens; B p50 0.259 seconds with 72.0% cached prompt tokens.

Pi exited 1 on every coder invocation. Captured stderr is a slice of Pi's bundled git-install path (`git clone` / `runNpmCommand`), not a model completion. The capture proxy therefore recorded no coder usage, so `$/1,000` stays blank.

Re-run commands are in [docs/RUNBOOK.md](docs/RUNBOOK.md). Frozen interpretation rules are in [benchmarks/PROTOCOL.md](benchmarks/PROTOCOL.md). The public desk never exposes prompts, generated code, service credentials or cache hashes.

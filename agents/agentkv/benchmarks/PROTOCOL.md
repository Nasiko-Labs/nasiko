# Frozen pilot protocol

Implementation snapshot for the completed natural-memory coding cohort: `bec0cd5`. Integration screens before that snapshot are diagnostic evidence, not final comparisons. The decode-neutral system prefix, timely two-Noul Jev plan, and idle-GPU warming land after that snapshot; do not pool their timings with `results/natural-c4-final.json`. Coding cohorts that run Pi inside the Nasiko coder are a separate harness; do not pool them with the single-shot coder tables. The first Pi retry (`results/run-1789899452-c4.json`, Modal `ap-LLAZ4Uj2TkOXZnfY6l9gwz`) recorded `"harness": "pi"` on all 300 workflows and 0/300 passing tests; coder usage was null. The costing-fix retry (`results/run-1789901249-c4.json`, Modal `ap-rRSPWgO6i6WQavuEfH6SOU`) also recorded `"harness": "pi"` on all 300 workflows, 0/300 passing tests, Pi exit code 1, and null coder usage. Neither is a coding-quality or unit-cost comparison.

## Matrix

- Main natural-memory cohort: 20 tasks, A–E, concurrency 4, three repetitions.
- Low-load cohort: 20 tasks, A–E, concurrency 1, one screening repetition.
- Separate stress cohort: 20 tasks, A–E, concurrency 4, three repetitions, explicit 1-GiB cache budget. Its maximum context is 4,096 rather than the natural cohort's 8,192; all fixture requests must fit either limit. This deliberately small cache is a mechanism test, not a claim of representative production memory pressure.
- Separate support cohort: 8 authored tickets against one knowledge base, Triage → Drafter → local citation check. Do not merge its frontier with the coding panel.
- Separate one-token probes: fixed task messages, A/B, serial requests, one generated token, repeated with each run's repetition count. They measure serving latency without variable completion lengths. They are not successful coding workflows or a complete replay of the agent pipeline.

Qwen/Qwen3.8-27B revision `1d4bf0f2ff6012fd82039f2fa52739d0dd7c60c0`, BF16, vLLM 0.29.0, A100 80 GB, one engine, maximum four sequences, 2,048 batched tokens, eager execution, temperature zero, seed 7, 512-token generation cap for live steps. Modal accepts up to 16 HTTP inputs so metrics and control requests can proceed alongside inference. Every method uses the same instrumentation and resource allocation.

## Work and arrival policy

Twenty authored utility repairs span four fixture repositories. Source, contracts and executable boundary tests are visible context. The Nasiko-deployed coder runs Pi against `module.py`; tester and reviewer remain Nasiko A2A steps. Pi's model calls go through Nasiko's injected OpenAI router via a worker-local capture proxy so AgentKV still sees usage and request ids. Tasks follow fixed round-robin repository order. Four disjoint calibration fixtures determine a smoothed repair frequency and a linear prefill-time estimate. No evaluation outcome is used to retrain Jev or choose its threshold.

Each trial offers the same finite batch of tasks simultaneously; a semaphore admits one or four workflows. End-to-end latency includes this client queue. Service time is retained separately. The batch drains completely before a cache reset. Method order is randomized with seed `37 + repetition`. Warmup, calibration, readiness and periodic-metric flush waits are outside measured trial windows and remain in the broader resource ledger.

## Treatments and gates

A puts task instructions before repository or knowledge-base context in the user message. B moves the same information into the system prefix so later agents share an exact token prefix. C/D/E use B's exact prompts and the same eviction-priority policy. C uses a fixed 0.5 repair probability, D a Laplace-smoothed calibration frequency, and E a Jev probability. Jev is one `system_one` request with at most two Noul questions (repair within two transitions; later LLM reuse of the shared prefix), started when the first LLM step starts. It never gates inference. Unreported Jev usage remains an unknown cost. Known reviewer/drafter successors are p=1 without a provider call.

Idle-GPU warming is a labeled factor (`AGENTKV_WARMING=0|1`). It prefills the exact next LLM prompt (`max_tokens=1`) only when vLLM reports zero running and waiting requests, repair probability is below 0.7, and prefix-reuse probability is at least 0.5. Warming does not invent future code or run tools. Compare warming off/on separately; do not attribute warming solely to Jev.

The ranking heuristic is `(1 + repair_probability) × calibrated_prefill_ms_per_token × reusable_tokens / resident_bytes`: one mandatory later LLM step plus a possible repair. It is an estimate, not a guarantee that both uses occur before lease expiry. Report prediction quality separately from policy usefulness and actual cache reuse.

At most three hints compete for a 25% pool quota. Shared physical blocks count once. A 15-second hint may reorder only free blocks whose recorded hashes still match. The engine's joint cache lookup defines a complete hybrid prefix, including recurrent-state checkpoints. It never changes model tensors, hashes, reference counts or active allocations. Terminal flows release their proposals; expiry handles lost cleanup.

Before timing C/D/E, an actual engine gate must acknowledge a priority, return a cached token count and preserve the first generated token on an identical prompt. Failure stops the run. This gate is distinct from the full executed task tests and does not prove a speedup.

## Interpretation

Report successful/attempted tasks, successful workflows/hour, p50/p95 queue-inclusive latency, service latency, prompt/cached/generated tokens, native cache/preemption/prefill counters, action acknowledgements, Jev latency/usage and prediction coverage. A cache hit after a priority does not establish that stock caching would have missed it.

Compare E/A, B/A, C/B, E/C and E/D. Preserve paired repetitions and show observed variation. Bootstrap independent paired trial means when at least three repeats exist; do not treat tokens or twenty queued workflows as independent timing experiments. Three repeats provide weak uncertainty estimates and no broad superiority guarantee. A one-repetition cohort has no confidence interval. Twenty authored tasks cannot establish a two-percentage-point quality margin.

The plotted frontier uses comparable trials with all observed task tests passing and complete cost evidence. It connects observed non-dominated points, not statistically proven winners. Different concurrency and cache-budget cohorts remain separate. A/B may produce different completion lengths despite equivalent instructions; report those lengths and use the one-token probes to distinguish prefill behavior from workflow performance.

Costs per 1,000 successes use the configured GPU/CPU/RAM envelope during each complete trial, plus reported Jev input usage. Failed work stays in the denominator accounting. Shared startup, calibration, teardown, storage, network and unreported charges are not silently allocated into those points. Report app-attributed Modal charges separately, with their reporting time and lag. Do not advertise a lower bill from a lower estimated steady-state cost alone.

Timing qualification: the recorded `decision_after_test` field describes proposal readiness, including resident-state lookup. Report its coverage separately. A proposal recorded after the first test result is a retrospective score, even though its input snapshot excluded that result. Only proposals recorded before testing completes qualify for the reported timely prediction Brier score. This does not establish when the remote model internally formed its answer.

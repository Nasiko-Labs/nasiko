## Problem Statement

Teams running agent workflows can observe requests and token usage but struggle to connect workflow state to inference resource decisions and verified economic outcomes. Generic cache recency does not capture whether a workflow is active, which prefixes multiple agents share, or whether expensive context will soon be reused. Conversely, a higher cache-hit rate or lower first-token latency does not prove lower cost per successful task.

We want AgentKV to use Nasiko as the control plane for execution, routing, telemetry and decision accountability. Jev should contribute bounded semantic judgments where deterministic workflow knowledge is insufficient. The product must show the incremental benefit of each component through fair, reproducible benchmarks, including small or negative results.

The initial workspace contains planning documents only. There is no existing AgentKV serving implementation or measured performance result. The first delivery establishes the repository, original domain primitives, cloud access verification and this issue; subsequent milestones build the integrated system.

## Solution

Technical positioning: **workflow-aware KV-cache optimization through Nasiko, assisted by Jev**. Better cache management is a claim to demonstrate against strong baselines; its product value is the cost–latency tradeoff at acceptable task quality.

Build AgentKV, workflow-aware inference optimization through the Nasiko control plane. Nasiko observes workflow execution; Jev estimates near-term reuse when uncertain; AgentKV applies a bounded cache policy; vLLM executes inference; Nasiko correlates actions with outcomes and displays benchmark results.

Host all project-operated services on Modal. A CPU VM Sandbox runs Nasiko, its required backing services and the small agent team. A separate single-replica GPU service runs a pinned Qwen/vLLM stack. Jev is accessed through TypeSafe's API from our Modal-hosted client.

The first demonstration is a real coding workflow with Coder, Tester and Reviewer roles, repair loops, actual test execution and a task-completion result. Preserve source prompts and model semantics. Manage reusable inference state rather than deleting conversation information. Start with exact prefix organization and retention; add bounded prewarming only when the evidence warrants it.

The product is called AgentKV. Nasiko is the control plane, not simply the host or branding. Prediction is an internal technique, not the product framing. The MVP stays focused on reuse, control and measurement.

## User Stories

1. As a workflow operator, I want to deploy and run the demo agents through Nasiko, so that their execution remains under one control plane.
2. As a workflow operator, I want authenticated model calls to pass through Nasiko, so that agent identity and provider configuration remain centrally managed.
3. As a workflow operator, I want handoffs, retries and terminal outcomes in one trace, so that I can understand task execution.
4. As a workflow operator, I want inference metrics correlated to a workflow, so that aggregate server counters are not mistaken for individual task measurements.
5. As a workflow operator, I want to enable or disable AgentKV, so that I can return to stock cache behavior.
6. As a workflow operator, I want inference to continue when the controller or Jev fails, so that an optimization does not become a correctness dependency.
7. As a workspace owner, I want cache reuse isolated to an authorized domain, so that another tenant cannot choose my namespace.
8. As a developer, I want stable context compiled consistently, so that compatible requests can reuse exact token prefixes.
9. As a developer, I want prompt roles and tool-result relationships preserved, so that efficiency work does not discard task evidence.
10. As a developer, I want model, tokenizer, template and repository revisions to participate in cache identity, so that incompatible state is never reused.
11. As an operator, I want active engine state protected, so that a cache hint cannot corrupt a running request.
12. As an operator, I want shared prefixes accounted for once, so that finishing one workflow does not discard another workflow's useful state.
13. As an operator, I want expired, stale and duplicate events handled conservatively, so that missing telemetry cannot leave permanent protection.
14. As an operator, I want complete hybrid state groups retained together, so that the selected model's recurrent and attention state remain consistent.
15. As an operator, I want verified engine acknowledgements for retention actions, so that the UI distinguishes a requested action from a completed one.
16. As an operator, I want Jev to evaluate a small candidate set in one request, so that decision overhead remains bounded.
17. As an operator, I want reuse estimates tied to a defined horizon, so that predicted probabilities can be evaluated against real outcomes.
18. As an operator, I want known workflow transitions handled by code, so that Jev is used only where interpretation adds value.
19. As an operator, I want raw and calibrated estimates recorded separately, so that model confidence is not presented as measured accuracy.
20. As an operator, I want warming limited by load and budget, so that speculation does not delay foreground work.
21. As an operator, I want no speculative tool execution, so that cache preparation cannot cause external side effects.
22. As a benchmark author, I want a stock prefix-caching baseline, so that AgentKV is compared against an already optimized engine.
23. As a benchmark author, I want separate prefix, deterministic-policy and Jev ablations, so that I can attribute gains correctly.
24. As a benchmark author, I want a transition-frequency alternative, so that I can measure Jev against a cheap predictor.
25. As a benchmark author, I want identical resources and paired workloads, so that hardware or sampling differences do not masquerade as policy gains.
26. As a benchmark author, I want low-reuse, repeated-workflow and pressure regimes, so that overhead and unfavorable cases are visible.
27. As a benchmark author, I want frozen replay and live task evaluation, so that serving performance and end-task quality are both assessed.
28. As a benchmark author, I want calibration and evaluation repositories separated, so that thresholds do not learn the test workload.
29. As a benchmark author, I want failed and pending tasks counted explicitly, so that throughput and cost reports cannot hide unfinished work.
30. As a benchmark author, I want uncertainty and absolute differences reported, so that a small noisy improvement is not overstated.
31. As a budget owner, I want total resource and Jev costs included, so that cost per successful workflow reflects the actual operating expense.
32. As a budget owner, I want bounded cloud runs and teardown, so that unused services do not consume the credit reserve.
33. As a developer, I want credentials in Modal Secrets, so that source, fixtures, issues and logs contain no keys.
34. As a reviewer, I want original implementation and documented dependencies, so that the project's provenance is clear.
35. As a reviewer, I want reproduction manifests and raw evidence, so that performance claims can be checked.
36. As a demo viewer, I want to see Nasiko execution, an actual AgentKV action and its observed outcome together, so that the integration's value is understandable.
37. As a project owner, I want the social demo to use verified project metrics, so that Jev's vendor claims are not misrepresented as our results.
38. As an operator, I want a documented backup and restore path for the demo environment, so that a sandbox restart does not silently erase all evidence.

39. As an operator, I want a measured cost–latency Pareto frontier, so that I can choose an operating point rather than rely on one favorable benchmark.
40. As a budget owner, I want cost compared at the same latency target and workload, so that a lower price does not conceal slower service.
41. As a reviewer, I want cache-policy gains isolated from prefix-formatting gains, so that better KV management is supported by the evidence.
42. As a reviewer, I want projected savings distinguished from observed bills, so that increased capacity is not presented as cash savings automatically.

## Implementation Decisions

### Originality and dependency policy

All AgentKV implementation is written independently for this project. Reference source may be studied to understand public interfaces, invariants and behavior; no source is copied, vendored, transplanted or mechanically rewritten. Nasiko itself is an unmodified, pinned dependency and the actual control plane being showcased. Necessary SDKs and libraries are allowed and declared. Original extensions or patches are limited to the integration seams needed for the product. TurboQuant, SpectralQuant, fast-jev-compaction, KVFlow and PBKV are references, not copied implementations.

### Deep modules and interfaces

- **Workflow context and prefix registry:** consumes authenticated events and request context; exposes a coherent revisioned snapshot and exact prefix identity. Encapsulates deduplication, lifecycle, namespace derivation and invalidation. Candidate identities are tied to full model/template and authorized-sharing context.
- **Decision and policy module:** consumes workflow snapshots, measured engine costs and candidate reuse estimates; returns bounded, expiring hints plus machine-derived reasons. Encapsulates deterministic rules, transition-frequency estimates, Jev batching, deadlines, calibration and fairness. No GPU pointer or allocator logic is delegated to Jev.
- **Engine adapter:** consumes authenticated hints and reports accepted/rejected actions plus observed residency. Encapsulates the pinned engine's active references, shared-prefix dependencies, hybrid state groups, restart identity and lease expiry. An HTTP proxy alone is insufficient for retention.
- **Telemetry and benchmark accounting:** consumes request outcomes, decisions, engine observations and resource-cost records; produces trace-linked metrics and reproducible comparison reports. Encapsulates attribution, missing data, billing windows and uncertainty. Zero successful tasks never becomes a zero-cost success result.
- **Deployment and control-plane integration:** starts and tears down the Modal services and exposes existing Nasiko execution and UI surfaces. It provides a small authorized AgentKV view rather than a second dashboard or new agent framework.

Internal event contracts include event identity, flow revision, verified owner, flow/session, agent and timestamp. Cache hints include prefix identity, model/engine identity, bounded lease, policy version and expiry. The engine acknowledges actions independently of the controller's request. Decisions and outcomes are joined by identifiers rather than inferred from matching timestamps. Minimal durable records cover experiment manifests, decisions, usage, costs and task outcomes; full prompt retention is not required by default.

### Deployment and model

- Modal CPU VM Sandbox with Docker supports the intended Nasiko stack; GPU inference runs in a separate Modal GPU service. Account availability, runtime integration, service connectivity and restore are feasibility gates.
- Keep one inference replica during controlled trials; concurrent requests share its cache. CPU and GPU instances are bounded and explicitly stopped after experiments.
- The current target is Qwen3.8-27B with a compatible pinned vLLM release, initially on an A100 80 GB. Validate memory fit, tool use, streaming and hybrid prefix-state behavior before committing benchmark spend. Fix thinking and output limits across comparisons.
- A separately labeled simpler-attention Qwen coding model may be used to isolate adapter behavior if hybrid compatibility blocks the target. Never present its measurements as the target model's results.
- The current Jev model target is the documented versioned model, with its secret injected only into the client needing it. API listing does not establish successful model decisions; a typed-decision test is a separate gate.
- Keep private database services inside the CPU environment. Use consistent exports to durable storage and a tested restore procedure; do not assume arbitrary shared volume semantics provide database durability.

### Policy

Use deterministic lifecycle and known transitions first. For uncertainty, make one Jev request with at most three independent yes/no reuse questions, all covering the next two agent transitions. Values are independent probabilities, not a normalized distribution and not guarantees. Invalid, late or stale answers trigger deterministic fallback.

Rank complete resident reusable state by estimated useful reuse probability multiplied by measured recomputation avoided, divided by incremental retained bytes. This is a starting heuristic, not a proven optimal algorithm. Protect active state, account for shared dependencies once, constrain leases by fairness and expiry, and invalidate controller residency knowledge after an engine restart. Completion releases that workflow's protection rather than indiscriminately deleting shared state.

Warm only an exactly reconstructable nonresident prefix, with at most one speculative candidate and a budget gate. Do not fabricate future code or tool output. Measure the additional work and displacement; warming may improve latency without reducing total cost. Retention-only comparisons precede any warming experiment.

### Benchmark design

Required variants: A stock automatic prefix caching; B A plus stable prefix organization; C B plus deterministic AgentKV retention; D C plus transition-frequency estimates; E C plus Jev estimates. C through E share one policy and resource budget; only uncertain-demand estimates differ. If warming is added, compare it separately and give the cheap estimator an equivalent capability.

Show total gain E/A, prefix gain B/A, workflow-policy gain C/B, Jev increment E/C and Jev versus cheap prediction E/D. A no-cache run is optional explanatory context, not the main competitor. LMCache, SGLang and external research-system reproduction are deferred unless compatible, fair runs fit after the required comparisons. Published paper numbers are related work, not directly comparable measurements.

Use equal instrumentation and model/GPU/cache/sampling configurations. Compare semantically paired prompts for A/B, then identical prompts for B–E. Include natural and constrained-memory regimes, log actual prompt lengths, and never pad context just to produce a gain. Start with a 20-task pilot plus disjoint calibration tasks, screen cheaply, and repeat finalists in randomized paired order. Frozen replay isolates serving; live held-out tasks use actual tests and bounded repair loops. Reset cache state consistently, separate cold/steady-state trials, disclose pending work and account for drain costs.

Primary outcomes are total cost per successful workflow and successful workflows per hour. Secondary outcomes include task pass counts, p50/p95 completion latency, TTFT, reuse coverage, recomputation, evictions/preemptions, queueing, decision cost and wasted preparation. Metrics absent from the engine remain unavailable rather than fabricated. Cost estimates and reconciled bills are labeled separately. Confidence intervals use independent tasks/runs, not correlated tokens.

### Cache superiority, Pareto frontier and pitch

The judge named in the event page is Karan Bharadwaj, Co-founder & Head of Product at Nasiko. Lead with the economic value of an actual KV-cache improvement and demonstrate Nasiko as the observation/control surface. Do not presume the judge lacks technical understanding. The product name remains AgentKV.

Require an empirical cost–latency Pareto plot: horizontal axis p95 end-to-end workflow seconds, vertical axis USD per 1,000 successful workflows, with task quality/reliability as eligibility constraints. Each plotted point comes from a measured configuration; show all points, non-dominated points per method and uncertainty. Dominance requires no worse cost or latency and strictly better performance in at least one; uncertainty can make apparent dominance inconclusive. Do not claim global optimality or invent intermediate measurements.

Within each panel hold model, GPU, precision, offered arrival workload, cache budget and task distribution fixed; sweep a small shared concurrency/batching grid with equal baseline tuning effort. Record queueing, failure/timeouts and completed throughput. Separate natural-memory and pressure regimes. Different offered-load points belong to a separately labeled capacity sweep, not an equal-load frontier.

Compare minimum measured cost meeting the same predeclared p95 target, and minimum measured latency under the same budget cap. If only one method meets a target, report feasibility rather than an invented ratio. Include actual/estimated total costs and show normalized dollars per 1,000 successful workflows. Report E/A as total benefit and E/C plus E/D as Jev's marginal contribution. Prefix formatting is separate; C–E must demonstrate actual cache-policy effects at identical prompts. Lower recomputation supports the mechanism; net cost/latency and task quality support the product outcome.

Distinguish measured bills, resource-based steady-state estimates and projections. Improved saturated throughput is a capacity gain; it does not necessarily lower an underutilized persistent GPU's bill. Never hide failed or pending work or claim a fast p95 solely by dropping slow failures. Frontiers that cross show a tradeoff, not universal superiority. A small test set cannot establish unchanged quality with high confidence.

### Delivery gates

- [ ] Nasiko and real deployed agents run on Modal with authenticated model calls.
- [ ] The target engine/model combination passes exact prefix and complete-state compatibility checks.
- [ ] A real retention seam is implemented and acknowledged; active state and tenant boundaries are protected.
- [ ] Stable prefixes and deterministic fallback are verified without information loss.
- [ ] Jev estimates are recorded before the events being evaluated and joined to actual outcomes.
- [ ] A–E comparisons run with manifests and evidence, showing total and marginal gains or inconclusive results.
- [ ] Nasiko displays execution, policy decisions, metrics and the benchmark comparison.
- [ ] The measured cost–latency frontier and same-latency economic comparison distinguish total and marginal gains, quality constraints and uncertainty.
- [ ] The demo, README, reproduction procedure and original-source provenance are complete.

## Testing Decisions

A good test exercises observable behavior, failure recovery or an invariant that could harm correctness or invalidate a measurement. Avoid assertions on private helper structure, incidental ordering or code that simply restates the implementation.

There is no prior AgentKV application test suite. The initial standard-library cost-accounting tests establish the pattern: explicit fixtures, deterministic inputs, invalid data rejected, failures/pending tasks not counted as success, and no finite unit-cost claim when success count is zero. Upstream repositories are conceptual references only; their tests are not copied.

Required module tests:

- Registry: duplicate and out-of-order events, lifecycle completion, revision invalidation, exact token identity and security-domain separation.
- Policy: expired hints, unavailable Jev, malformed values, late responses, bounded candidates, deterministic known transitions, lease limits and no speculative tool execution.
- Adapter: executing state cannot be evicted, shared dependencies remain valid, hybrid groups remain complete, cancellation releases references, restarts invalidate residency and acknowledgements match observed action.
- Accounting: all run costs retained when tasks fail, pending work reported, zero-success handling, missing measurements distinct from zero, and shared costs not double counted.
- Benchmark: equal configurations, clean cache boundaries, held-out data, paired run order, complete manifests, correct cost denominators and reproducible report generation.
- Frontier/reporting: dominated points, equal ties, quality-ineligible configurations, incomparable workload panels, zero-success runs and missing metrics are handled explicitly; uncertainty is not mistaken for proven dominance.
- Integration: real Nasiko identity and trace propagation, streaming/cancellation, prefix namespaces through the router, authenticated engine control, and controller-outage fallback.

Use local deterministic tests before cloud integration; mocks establish contracts but cannot prove actual GPU retention. A bounded CPU credential check proves Modal-to-TypeSafe access without printing secrets. GPU integration and real tasks are separate tests and must be budgeted. A pilot cannot establish a two-percentage-point quality noninferiority claim; show raw pass counts and uncertainty until sample size is sufficient.

## Out of Scope

- Autopilot branding, a generic optimization platform, or replacing Nasiko/vLLM.
- Copied source from reference projects, transcript compaction, semantic response caching, and lossy evidence deletion.
- TurboQuant/SpectralQuant implementation, spectral calibration, custom attention kernels and per-prefix precision switching.
- New GPU allocators, multi-GPU/distributed scheduling, disk cache tiers and mandatory CPU offload.
- Model routing, a new firewall, DronaHQ, speculative tool execution and autonomous publication.
- Training/fine-tuning Jev, self-hosting Jev weights, or claims of research novelty without evidence.
- Production HA/database failover and a broad multi-cloud deployment framework.
- Unmatched performance claims against paper numbers, fabricated gains, or presenting proxy hints as real cache actions.

## Further Notes

The prior discussion is the requirements interview; this issue incorporates the subsequent corrections. Priorities are Nasiko as control plane first, useful Jev decisions second, original implementation and YAGNI throughout. No extra interview is needed to begin the agreed work.

Modal budget ceiling is $50 with a $10 reserve; aim to keep planned usage at $40. Allocate approximately $6 CPU/integration, $20 development/ablations, $10 final comparisons and $4 demo, adjusting from actual usage. Jev charges are separate. Check prices before long runs; set bounded timeouts and teardown instead of leaving a GPU active. A full unbuilt adapter and strong statistical evaluation are not promised within a four-hour event sprint.

Aspirations of roughly 15% lower unit cost or 20% higher successful throughput are hypotheses, not acceptance numbers to manufacture. Repeatable smaller gains are valid. If uncertainty spans zero, say no demonstrated improvement. Preserve task quality and disclose sample limits. At equal hourly cost, 20% more throughput implies approximately 16.7% less cost per workflow.

Research references and current official documentation:

- [Nasiko Build-A-Thon and named judge](https://luma.com/l5brddfg)
- [Nasiko](https://github.com/Nasiko-Labs/nasiko)
- [TypeSafe primitives](https://docs.typesafe.ai/primitives) and [models](https://docs.typesafe.ai/models)
- [Modal VM Sandboxes](https://modal.com/docs/guide/vm-sandboxes)
- [Qwen3.8-27B](https://huggingface.co/Qwen/Qwen3.8-27B)
- [vLLM prefix caching](https://docs.vllm.ai/en/stable/design/prefix_caching/)
- [fast-jev-compaction](https://github.com/tamaratran/fast-jev-compaction)
- [TurboQuant](https://arxiv.org/abs/2504.19874) and [SpectralQuant](https://github.com/Dynamis-Labs/spectralquant/blob/main/paper_output_consolidated/spectralquant_unrestricted_paper.pdf)
- [KVFlow](https://arxiv.org/abs/2507.07400) and [PBKV](https://arxiv.org/abs/2605.06472)

The demo starts in Nasiko and ends with measured A–E results and task outcomes. The prepared Jev quote-post may be published only after implementation and user direction. Current request authorizes repository and issue creation and bounded cloud setup, not social publication. Event-specific submission requirements should be verified separately.

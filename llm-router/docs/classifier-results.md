# P2 implementation and measured results

Measured locally on 2026-10-03. The implementation is on `feat/jev-cache-aware-classifier`. JEV uses the OpenRouter Decisions endpoint and model revision `typesafe/jev-1.13-20260917`. The keyword classifier remains the repository default.

## Classification results

These results use the unchanged prompt, a three-second timeout, and an initial confidence cutoff of 0.6. The cutoff has not been calibrated. The confidence value is JEV's probability for its selected task category. The keyword baseline returns fixed difficulty 3 and confidence 0.5. Those constants are not predictions.

| Set | Backend | Correct task labels | Difficulty mean absolute error | Median decision time | 95th percentile time | Reported decision cost |
| --- | --- | --- | --- | --- | --- | --- |
| Public smoke, 10 cases | Keyword | 3/10 | 1.20 | 0.229 ms | 69.296 ms | No API calls |
| Public smoke, 10 cases | JEV | 8/10 | 0.50 | 726.221 ms | 1291.928 ms | $0.000295680 total |
| Own validation, 21 cases | Keyword | 7/21 | 1.238 | 0.044 ms | 0.460 ms | No API calls |
| Own validation, 21 cases | JEV | 21/21 | 0.143 | 719.343 ms | 825.987 ms | $0.000565026 total |

The first baseline run included initial regular-expression setup in the first case. The runner now initializes the keyword classifier before the timed loop. Treat the first baseline latency table as historical evidence. Hosted times include network latency. Local hardware is a 12th Gen Intel i5-1245U with 12 logical CPUs. These were debug builds on the local CPU. Hosted model size, hardware, and cold-start load time are not available from this API.

JEV averaged $0.000029568 per decision on the public set and $0.000026906 per decision on validation. No fallback occurred during those successful live runs. They do not establish the expected fallback rate on other workloads.

JEV misclassified public cases pub-06 and pub-10. These ask for investigation and also ask for designs or fixes. The public ten-bin confidence calibration error was about 0.210 for JEV and 0.200 for the keyword constants. JEV's public calibration result was worse despite higher label accuracy. Validation calibration error was about 0.0167 for JEV. The small synthetic validation set is easier than the private organizer set may be. No broad accuracy claim follows from 21 correct labels.

## Repeatability

Both backends were run twice on each set. Keyword task labels, difficulty, confidence, and fallback reasons were unchanged. JEV task labels were unchanged on both sets. Four public confidence values changed. Three validation confidence values changed. One validation difficulty changed. No fallback reason changed.

The router uses a fixed `CLASSIFIER_SEED` for its existing tier selection when JEV is enabled. This seed does not control OpenRouter's hosted inference. The endpoint does not offer a documented inference seed in the verified contract. Strict hosted repeatability is an open qualification issue. Pinning the model revision does not solve it.

## Cache demo

The `cache_switch_demo` example ran against the local PostgreSQL server. It creates temporary copies of the usage, flow, and price tables on one database connection. Its usage counts and prices are synthetic. Existing application rows remain intact.

- Cached case. Estimated stay cost is $0.013. Estimated switch cost is $0.0515. The policy keeps the current model.
- Uncached case. Estimated stay cost is $0.103. Estimated switch cost is $0.0515. The policy permits a switch.
- Missing evidence. The policy keeps the current model.

The demo calls the same usage lookup and pricing engine as runtime routing. It proves those paths can produce keep and switch decisions. It does not measure actual answer quality or cost savings.

Runtime comparison uses the most recent matching usage record within ten minutes. It matches user, agent, conversation, provider, and current model. It requires reported token counts and exact matched prices. Earlier cache writes are treated as possible reads when estimating staying cost. A candidate receives the previous prompt tokens as fresh input. Candidate output tokens are estimated from the prior response. A proposed switch needs more than 20% estimated savings.

Cache reuse is an estimate. Context changes, cache expiry, provider selection, and different output lengths can change actual costs. Coding-agent keys are currently per turn, so cross-turn cache comparison is not supported on that path. It keeps the current model when matching evidence is unavailable.

## Verification and remaining work

The router library checks passed with 384 tests passing and one existing test ignored. Four additional safety tests passed. They cover timeout, low confidence, invalid output, and retention of recent context under long instructions. Routing tests cover pinning, continuation, and seeded selection. The PostgreSQL demo passed all three cases.

The local application was rebuilt and restarted with JEV and the cache policy enabled in the ignored local environment file. The repository defaults remain keyword classification and no cache-based switching.

No downstream answer-model workload has been evaluated. Do not claim cheaper or better answers yet. Strict hosted reproducibility, confidence calibration on a larger set, and downstream answer-quality and cost comparisons remain before a qualifying submission can be claimed. No fork PR has been submitted.

Final verification repeated after the database timeout guard. All 388 tests passed across the library and safety suites, with one existing ignored test. Server and examples rebuilt successfully.

## Repeatable local backend and startup configuration

A separate `CLASSIFIER_BACKEND=local` backend uses `CLASSIFIER_MODEL=local-training-v1`. It fits smoothed multinomial naive Bayes category and difficulty models once from the embedded 21-case training set. It reads neither validation nor public scoring data during model construction. Confidence is a softened posterior multiplied by known-vocabulary coverage; it is not calibrated. No validation-based tuning was performed.

Two fresh debug evaluation processes produced identical category, difficulty, confidence, and fallback results on both datasets. Timing and model initialization times are measurements, not repeatable predictions. With the unchanged 0.6 cutoff, all 10 public and all 21 validation cases fell back to regex. Guarded accuracy was therefore 3/10 and 7/21. This backend provides repeatability but no measured accuracy gain. API charge is zero; local CPU cost has not been priced. JEV retains its measured hosted variability and is not claimed deterministic.

Classifier and cache-switch settings now load in the binary configuration module. The standalone binary, evaluation example, and server binary pass resolved settings into the router. Existing non-classifier environment loading remains for compatibility. The server host injection needs three small files outside llm-router; the strict submission scope must be reviewed before creating the fork PR.

Final compliance checks passed with 400 tests across library, safety, binary-config, and evaluation-example suites, plus one existing ignored test. The in-process server library and binary passed cargo check. Three server host-injection files are outside the strict llm-router submission fence; they are included as integration work, not claimed scope-compliant. The running local server has not been rebuilt or restarted for this follow-up.

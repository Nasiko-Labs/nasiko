# Request-classifier benchmark

Release build; routing columns are `pick_tier` over 50 cold-start seeds (tier costs 15 / 3 / 0.8).

Routing columns simulate the tier sampler for every classification, including low-confidence cases. The configured-model abstention rate is reported separately; configured-model cost and downstream answer quality are not measured here.

Strands endpoint model: `strands-decider-2B-hobson-v19-q4`; device: `mlx`; ready: `true`. Both backends use the runtime factory with a 30000 ms timeout. `strands_raw` asks both questions and reports native concentration confidence. `strands` asks only type, reports selected-label probability with identity temperature 1.0 after rejecting training-only sharpening, and takes complexity from the embedded local model. Confidence semantics differ, so their ECE/Brier values are not a comparison of two probabilities with the same meaning.

## Held-out validation (154 requests)

| metric | regex | local | strands_raw | strands |
|---|---|---|---|---|
| type accuracy | 32.5% | 86.4% | 90.9% | 90.9% |
| type macro-F1 | 0.335 | 0.864 | 0.907 | 0.907 |
| scalar confidence <0.4 (regex exempt; diagnostic) | 0.0% | 2.6% | 10.4% | 3.9% |
| low confidence → configured model, unpinned | 0.0% | 2.6% | 10.4% | 10.4% |
| type accuracy when confident | 32.5% | 86.7% | 93.5% | 93.5% |
| complexity exact | 30.5% | 66.9% | 37.7% | 66.9% |
| complexity within ±1 | 69.5% | 98.1% | 91.6% | 98.1% |
| complexity MAE | 1.00 | 0.35 | 0.71 | 0.35 |
| hard (4–5) recognised as hard | 0.0% | 78.6% | 57.1% | 78.6% |
| confidence ECE (lower is better) | 0.157 | 0.036 | 0.204 | 0.161 |
| Brier score (lower is better) | 0.159 | 0.093 | 0.113 | 0.095 |
| latency p50 | 2 µs | 49 µs | 481 ms | 244 ms |
| latency p95 | 24 µs | 233 µs | 688 ms | 345 ms |
| latency p99 | 44 µs | 307 µs | 806 ms | 414 ms |
| routing: mean tier cost | 2.18 | 2.86 | 3.34 | 2.95 |
| routing: hard → cheapest tier (lower is better) | 57.3% | 10.0% | 13.3% | 10.9% |
| routing: easy → cheapest tier (higher is better) | 61.4% | 75.8% | 59.2% | 76.1% |
| fell back to regex | 0 | 0 | 0 | 0 |

### Accuracy by failure mode

| tag | regex | local | strands_raw | strands |
|---|---|---|---|---|
| context_dependent (n=22) | 13.6% | 68.2% | 90.9% | 90.9% |
| keyword_trap (n=18) | 44.4% | 77.8% | 83.3% | 83.3% |
| long_spec (n=18) | 44.4% | 83.3% | 94.4% | 94.4% |
| multilingual (n=27) | 25.9% | 77.8% | 81.5% | 81.5% |
| no_keyword (n=19) | 31.6% | 78.9% | 78.9% | 78.9% |
| plain (n=43) | 48.8% | 100.0% | 95.3% | 95.3% |
| short_simple (n=39) | 30.8% | 94.9% | 89.7% | 89.7% |
| typo_informal (n=18) | 16.7% | 100.0% | 100.0% | 100.0% |

### F1 by request type

| type | regex | local | strands_raw | strands |
|---|---|---|---|---|
| code_generation | 0.41 | 0.95 | 0.94 | 0.94 |
| code_understanding | 0.24 | 0.86 | 0.93 | 0.93 |
| technical_design | 0.24 | 0.86 | 0.92 | 0.92 |
| analytical_reasoning | 0.41 | 0.80 | 0.92 | 0.92 |
| writing | 0.31 | 0.86 | 0.98 | 0.98 |
| factual_lookup | 0.43 | 0.87 | 0.88 | 0.88 |
| general | 0.30 | 0.85 | 0.80 | 0.80 |

### local vs regex: 89 fixed, 6 broken

Broken: `val-065` technical_design → local said analytical_reasoning; `val-140` general → local said analytical_reasoning; `val-143` general → local said writing; `val-145` general → local said factual_lookup; `val-150` general → local said analytical_reasoning; `val-154` general → local said analytical_reasoning

### strands_raw vs regex: 97 fixed, 7 broken

Broken: `val-114` factual_lookup → strands_raw said technical_design; `val-133` general → strands_raw said code_generation; `val-137` general → strands_raw said technical_design; `val-140` general → strands_raw said technical_design; `val-145` general → strands_raw said factual_lookup; `val-148` general → strands_raw said analytical_reasoning; `val-153` general → strands_raw said analytical_reasoning

### strands vs regex: 97 fixed, 7 broken

Broken: `val-114` factual_lookup → strands said technical_design; `val-133` general → strands said code_generation; `val-137` general → strands said technical_design; `val-140` general → strands said technical_design; `val-145` general → strands said factual_lookup; `val-148` general → strands said analytical_reasoning; `val-153` general → strands said analytical_reasoning

## Hackathon public sample (10 requests)

| metric | regex | local | strands_raw | strands |
|---|---|---|---|---|
| type accuracy | 30.0% | 70.0% | 100.0% | 100.0% |
| type macro-F1 | 0.262 | 0.567 | 0.857 | 0.857 |
| scalar confidence <0.4 (regex exempt; diagnostic) | 0.0% | 10.0% | 30.0% | 20.0% |
| low confidence → configured model, unpinned | 0.0% | 10.0% | 30.0% | 30.0% |
| type accuracy when confident | 30.0% | 77.8% | 100.0% | 100.0% |
| complexity exact | 20.0% | 40.0% | 40.0% | 40.0% |
| complexity within ±1 | 60.0% | 80.0% | 90.0% | 80.0% |
| complexity MAE | 1.20 | 0.80 | 0.70 | 0.80 |
| hard (4–5) recognised as hard | 0.0% | 100.0% | 100.0% | 100.0% |
| confidence ECE (lower is better) | 0.150 | 0.191 | 0.388 | 0.333 |
| Brier score (lower is better) | 0.165 | 0.127 | 0.220 | 0.162 |
| latency p50 | 10 µs | 99 µs | 490 ms | 244 ms |
| latency p95 | 452 µs | 358 µs | 781 ms | 381 ms |
| latency p99 | 452 µs | 358 µs | 781 ms | 381 ms |
| routing: mean tier cost | 2.58 | 4.18 | 3.91 | 4.26 |
| routing: hard → cheapest tier (lower is better) | 26.7% | 0.0% | 4.0% | 0.0% |
| routing: easy → cheapest tier (higher is better) | 46.4% | 51.2% | 44.8% | 51.6% |
| fell back to regex | 0 | 0 | 0 | 0 |

### Accuracy by failure mode

| tag | regex | local | strands_raw | strands |
|---|---|---|---|---|
| architecture (n=1) | 0.0% | 100.0% | 100.0% | 100.0% |
| audience (n=1) | 0.0% | 100.0% | 100.0% | 100.0% |
| code_explanation (n=1) | 100.0% | 100.0% | 100.0% | 100.0% |
| code_generation (n=1) | 0.0% | 100.0% | 100.0% | 100.0% |
| code_keyword_low_effort (n=1) | 0.0% | 100.0% | 100.0% | 100.0% |
| constraint_preservation (n=1) | 100.0% | 100.0% | 100.0% | 100.0% |
| context_grounding (n=1) | 0.0% | 100.0% | 100.0% | 100.0% |
| factual (n=1) | 0.0% | 100.0% | 100.0% | 100.0% |
| minimal_edit (n=1) | 0.0% | 0.0% | 100.0% | 100.0% |
| misleading_cache_keyword (n=1) | 0.0% | 0.0% | 100.0% | 100.0% |
| misleading_keywords (n=1) | 0.0% | 0.0% | 100.0% | 100.0% |
| moderate_context (n=2) | 0.0% | 100.0% | 100.0% | 100.0% |
| multi_constraint (n=1) | 0.0% | 100.0% | 100.0% | 100.0% |
| multiple_constraints (n=1) | 0.0% | 100.0% | 100.0% | 100.0% |
| negation (n=1) | 0.0% | 0.0% | 100.0% | 100.0% |
| negative_constraints (n=1) | 0.0% | 100.0% | 100.0% | 100.0% |
| no_context (n=1) | 0.0% | 100.0% | 100.0% | 100.0% |
| reasoning (n=1) | 100.0% | 0.0% | 100.0% | 100.0% |
| risk (n=1) | 100.0% | 0.0% | 100.0% | 100.0% |
| short_context (n=5) | 60.0% | 60.0% | 100.0% | 100.0% |
| short_request (n=2) | 0.0% | 100.0% | 100.0% | 100.0% |
| temporal_reasoning (n=1) | 0.0% | 0.0% | 100.0% | 100.0% |
| writing (n=1) | 100.0% | 100.0% | 100.0% | 100.0% |

### F1 by request type

| type | regex | local | strands_raw | strands |
|---|---|---|---|---|
| code_generation | 0.00 | 0.80 | 1.00 | 1.00 |
| code_understanding | 0.50 | 0.67 | 1.00 | 1.00 |
| technical_design | 0.00 | 0.50 | 1.00 | 1.00 |
| analytical_reasoning | 0.67 | 0.00 | 1.00 | 1.00 |
| writing | 0.67 | 1.00 | 1.00 | 1.00 |
| factual_lookup | 0.00 | 1.00 | 1.00 | 1.00 |
| general | 0.00 | 0.00 | 0.00 | 0.00 |

### local vs regex: 5 fixed, 1 broken

Broken: `pub-10` analytical_reasoning → local said technical_design

### strands_raw vs regex: 7 fixed, 0 broken

Broken: none

### strands vs regex: 7 fixed, 0 broken

Broken: none

## Labelled requests with chat history (100 requests)

| metric | regex | local | strands_raw | strands |
|---|---|---|---|---|
| type accuracy | 43.0% | 92.0% | 97.0% | 97.0% |
| type macro-F1 | 0.448 | 0.918 | 0.970 | 0.970 |
| scalar confidence <0.4 (regex exempt; diagnostic) | 0.0% | 0.0% | 6.0% | 0.0% |
| low confidence → configured model, unpinned | 0.0% | 0.0% | 6.0% | 6.0% |
| type accuracy when confident | 43.0% | 92.0% | 97.9% | 97.9% |
| complexity exact | 20.0% | 66.0% | 29.0% | 66.0% |
| complexity within ±1 | 64.0% | 93.0% | 97.0% | 93.0% |
| complexity MAE | 1.16 | 0.42 | 0.74 | 0.42 |
| hard (4–5) recognised as hard | 0.0% | 47.1% | 52.9% | 47.1% |
| confidence ECE (lower is better) | 0.125 | 0.053 | 0.198 | 0.166 |
| Brier score (lower is better) | 0.178 | 0.053 | 0.078 | 0.062 |
| latency p50 | 2 µs | 36 µs | 416 ms | 217 ms |
| latency p95 | 26 µs | 64 µs | 477 ms | 251 ms |
| latency p99 | 108 µs | 90 µs | 516 ms | 298 ms |
| routing: mean tier cost | 2.40 | 2.41 | 3.15 | 2.44 |
| routing: hard → cheapest tier (lower is better) | 54.4% | 24.8% | 13.3% | 24.4% |
| routing: easy → cheapest tier (higher is better) | 57.5% | 71.5% | 61.4% | 72.9% |
| fell back to regex | 0 | 0 | 0 | 0 |

### Accuracy by failure mode

| tag | regex | local | strands_raw | strands |
|---|---|---|---|---|
| context_dependent (n=5) | 0.0% | 100.0% | 100.0% | 100.0% |
| plain (n=95) | 45.3% | 91.6% | 96.8% | 96.8% |

### F1 by request type

| type | regex | local | strands_raw | strands |
|---|---|---|---|---|
| code_generation | 0.40 | 0.96 | 0.96 | 0.96 |
| code_understanding | 0.35 | 0.86 | 0.96 | 0.96 |
| technical_design | 0.25 | 0.90 | 0.93 | 0.93 |
| analytical_reasoning | 0.70 | 0.85 | 0.97 | 0.97 |
| writing | 0.60 | 0.93 | 1.00 | 1.00 |
| factual_lookup | 0.48 | 0.97 | 0.96 | 0.96 |
| general | 0.36 | 0.97 | 1.00 | 1.00 |

### local vs regex: 50 fixed, 1 broken

Broken: `ar-05` analytical_reasoning → local said factual_lookup

### strands_raw vs regex: 55 fixed, 1 broken

Broken: `fl-12` factual_lookup → strands_raw said analytical_reasoning

### strands vs regex: 55 fixed, 1 broken

Broken: `fl-12` factual_lookup → strands said analytical_reasoning


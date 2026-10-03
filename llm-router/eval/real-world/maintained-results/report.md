# Real WildChat P2 evaluation — 1500 ms deadline

**Exploratory evaluation against pre-inference Codex annotations; not independent human gold.**

| Metric | Regex | Local | Laya | Strands 4-bit | Strands raw 4-bit |
|---|---:|---:|---:|---:|---:|
| Type accuracy | 16.0% (16/100) | 52.0% (52/100) | 41.0% (41/100) | 68.0% (68/100) | 68.0% (68/100) |
| Macro-F1 | 0.143 | 0.341 | 0.307 | 0.597 | 0.597 |
| Unambiguous type accuracy (63) | 19.0% | 68.3% | 49.2% | 77.8% | 77.8% |
| Near-duplicate-filtered accuracy (96) | 15.6% | 51.0% | 40.6% | 68.8% | 68.8% |
| Alternative-aware type accuracy | 20.0% | 60.0% | 50.0% | 80.0% | 80.0% |
| Initial-turn accuracy (63) | 22.2% | 52.4% | 39.7% | 65.1% | 65.1% |
| Follow-up accuracy (37) | 5.4% | 51.4% | 43.2% | 73.0% | 73.0% |
| Complexity exact | 42.0% | 48.0% | 50.0% | 48.0% | 49.0% |
| Complexity within ±1 | 90.0% | 91.0% | 86.0% | 91.0% | 95.0% |
| Complexity MAE | 0.68 | 0.61 | 0.64 | 0.61 | 0.56 |
| Hard recall (10) | 0.0% | 70.0% | 50.0% | 70.0% | 0.0% |
| Returned-confidence ECE (semantics differ) | 0.239 | 0.231 | 0.268 | 0.137 | 0.150 |
| Top-label Brier | 0.121 | 0.262 | 0.281 | 0.192 | 0.206 |
| Latency p50 | 0.003 ms | 0.059 ms | 143.205 ms | 298.301 ms | 545.718 ms |
| Latency p95 | 0.020 ms | 0.349 ms | 354.184 ms | 525.464 ms | 992.821 ms |
| Latency p99 | 0.076 ms | 0.397 ms | 462.760 ms | 602.602 ms | 1209.609 ms |
| Error/timeout fallbacks | 0 | 0 | 1 | 0 | 0 |
| Confidence <0.4 (regex exempt) | 0 | 8 | 17 | 12 | 26 |
| Safe-default policy requests | 0 | 8 | 17 | 26 | 26 |
| Accuracy among policy-accepted types | 16.0% (100 accepted) | 51.1% (92 accepted) | 46.3% (82 accepted) | 77.0% (74 accepted) | 77.0% (74 accepted) |
| Accepted wrong predictions | 84 | 45 | 44 | 17 | 17 |
| Changed classifications on repeat | 0 | 0 | 1 | 0 | 0 |

## Repeat 2

| Backend | Accuracy | p50 | p95 | Fallbacks | Low confidence |
|---|---:|---:|---:|---:|---:|
| regex | 16.0% | 0.002 ms | 0.020 ms | 0 | 0 |
| local | 52.0% | 0.064 ms | 0.410 ms | 0 | 8 |
| laya | 42.0% | 146.742 ms | 384.133 ms | 0 | 17 |
| strands | 68.0% | 275.085 ms | 511.759 ms | 0 | 26 |
| strands_raw | 68.0% | 517.979 ms | 992.518 ms | 0 | 26 |

Safe-default policy uses the returned low_confidence flag when present. Optimized Strands exposes probability confidence and preserves native concentration admission with an additional selected-probability minimum of (1+6*0.4)/7; literal confidence <0.4 alone is not its full routing policy.

Accuracy and confidence metrics include all recorded outputs, including operational fallback classifications where present. Accepted-type metrics exclude failure fallbacks; a failed backend does not provide a native model prediction.

Classification changes compare the complete outputs excluding latency. A change in fallback state is an operational change, not necessarily model nondeterminism.


## Type F1 and support

| Type | n | Regex | Local | Laya | Strands 4-bit | Strands raw 4-bit |
|---|---:|---:|---:|---:|---:|---:|
| code_generation | 16 | 0.476 | 0.471 | 0.478 | 0.811 | 0.811 |
| code_understanding | 9 | 0.000 | 0.000 | 0.143 | 0.308 | 0.308 |
| technical_design | 6 | 0.000 | 0.167 | 0.421 | 0.769 | 0.769 |
| analytical_reasoning | 9 | 0.000 | 0.308 | 0.000 | 0.462 | 0.462 |
| writing | 41 | 0.178 | 0.795 | 0.613 | 0.827 | 0.827 |
| factual_lookup | 14 | 0.235 | 0.381 | 0.273 | 0.696 | 0.696 |
| general | 5 | 0.109 | 0.267 | 0.222 | 0.308 | 0.308 |

## Run provenance

Requested backend order: regex, local, laya, strands, strands_raw.
Binary SHA-256: `b14d2485f5b45f06be17345e1c32b9c22539052ded3465fa4460c0cffc6e83bf`.
Frozen-label SHA-256: `c23ddedb82c675ba125c828c0fb2fa7efe5388866d05f6e1cd1d0eba97f661a7`.
Source snapshots, endpoint mappings, runtime and sidecar residency metadata, calibration decisions, and prediction hashes are recorded in [run-manifest.json](run-manifest.json).

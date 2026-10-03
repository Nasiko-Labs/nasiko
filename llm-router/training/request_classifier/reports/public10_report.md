# Report on classifier-eval.json

### local v2 (public 10, smoke only, not evidence)

n=10 · **type accuracy 1.000** · macro-F1 0.857 · ECE(10) 0.106 · top-label Brier 0.028 · complexity exact 0.500, ±1 1.000, MAE 0.500 · latency p50 221µs p95 687µs

| class | support | precision | recall | F1 |
|---|---|---|---|---|
| code_generation | 3 | 1.000 | 1.000 | 1.000 |
| code_understanding | 1 | 1.000 | 1.000 | 1.000 |
| technical_design | 1 | 1.000 | 1.000 | 1.000 |
| analytical_reasoning | 2 | 1.000 | 1.000 | 1.000 |
| writing | 2 | 1.000 | 1.000 | 1.000 |
| factual_lookup | 1 | 1.000 | 1.000 | 1.000 |
| general | 0 | 0.000 | 0.000 | 0.000 |

Confusion (rows = gold, cols = predicted, order as above):

| gold \ pred | code_g | code_u | techni | analyt | writin | factua | genera |
|---|---|---|---|---|---|---|---|
| code_generat | 3 | 0 | 0 | 0 | 0 | 0 | 0 |
| code_underst | 0 | 1 | 0 | 0 | 0 | 0 | 0 |
| technical_de | 0 | 0 | 1 | 0 | 0 | 0 | 0 |
| analytical_r | 0 | 0 | 0 | 2 | 0 | 0 | 0 |
| writing | 0 | 0 | 0 | 0 | 2 | 0 | 0 |
| factual_look | 0 | 0 | 0 | 0 | 0 | 1 | 0 |
| general | 0 | 0 | 0 | 0 | 0 | 0 | 0 |

Reliability (ECE bins): range · count · accuracy · mean confidence

- (0.6, 0.7] · 2 · 1.000 · 0.663
- (0.8, 0.9] · 2 · 1.000 · 0.846
- (0.9, 1.0] · 6 · 1.000 · 0.987

Selective accuracy (abstain below τ):

| τ | coverage | accuracy on accepted |
|---|---|---|
| 0.0 | 1.000 | 1.000 |
| 0.3 | 1.000 | 1.000 |
| 0.4 | 1.000 | 1.000 |
| 0.5 | 1.000 | 1.000 |
| 0.6 | 1.000 | 1.000 |
| 0.7 | 0.800 | 1.000 |
| 0.8 | 0.800 | 1.000 |
| 0.9 | 0.600 | 1.000 |

Per slice / tag accuracy:

| slice/tag | n | accuracy |
|---|---|---|
| architecture | 1 | 1.000 |
| audience | 1 | 1.000 |
| code_explanation | 1 | 1.000 |
| code_generation | 1 | 1.000 |
| code_keyword_low_effort | 1 | 1.000 |
| constraint_preservation | 1 | 1.000 |
| context_grounding | 1 | 1.000 |
| factual | 1 | 1.000 |
| minimal_edit | 1 | 1.000 |
| misleading_cache_keyword | 1 | 1.000 |
| misleading_keywords | 1 | 1.000 |
| moderate_context | 2 | 1.000 |
| multi_constraint | 1 | 1.000 |
| multiple_constraints | 1 | 1.000 |
| negation | 1 | 1.000 |
| negative_constraints | 1 | 1.000 |
| no_context | 1 | 1.000 |
| reasoning | 1 | 1.000 |
| risk | 1 | 1.000 |
| short_context | 5 | 1.000 |
| short_request | 2 | 1.000 |
| temporal_reasoning | 1 | 1.000 |
| writing | 1 | 1.000 |

### regex (public 10)

n=10 · **type accuracy 0.300** · macro-F1 0.262 · ECE(10) 0.140 · top-label Brier 0.140 · complexity exact 0.200, ±1 0.600, MAE 1.200 · latency p50 15µs p95 61µs

| class | support | precision | recall | F1 |
|---|---|---|---|---|
| code_generation | 3 | 0.000 | 0.000 | 0.000 |
| code_understanding | 1 | 0.333 | 1.000 | 0.500 |
| technical_design | 1 | 0.000 | 0.000 | 0.000 |
| analytical_reasoning | 2 | 1.000 | 0.500 | 0.667 |
| writing | 2 | 1.000 | 0.500 | 0.667 |
| factual_lookup | 1 | 0.000 | 0.000 | 0.000 |
| general | 0 | 0.000 | 0.000 | 0.000 |

Confusion (rows = gold, cols = predicted, order as above):

| gold \ pred | code_g | code_u | techni | analyt | writin | factua | genera |
|---|---|---|---|---|---|---|---|
| code_generat | 0 | 0 | 0 | 0 | 0 | 0 | 3 |
| code_underst | 0 | 1 | 0 | 0 | 0 | 0 | 0 |
| technical_de | 0 | 1 | 0 | 0 | 0 | 0 | 0 |
| analytical_r | 0 | 1 | 0 | 1 | 0 | 0 | 0 |
| writing | 0 | 0 | 0 | 0 | 1 | 0 | 1 |
| factual_look | 0 | 0 | 0 | 0 | 0 | 0 | 1 |
| general | 0 | 0 | 0 | 0 | 0 | 0 | 0 |

Reliability (ECE bins): range · count · accuracy · mean confidence

- (0.1, 0.2] · 5 · 0.000 · 0.120
- (0.7, 0.8] · 5 · 0.600 · 0.760

Selective accuracy (abstain below τ):

| τ | coverage | accuracy on accepted |
|---|---|---|
| 0.0 | 1.000 | 0.300 |
| 0.3 | 0.500 | 0.600 |
| 0.4 | 0.500 | 0.600 |
| 0.5 | 0.500 | 0.600 |
| 0.6 | 0.500 | 0.600 |
| 0.7 | 0.500 | 0.600 |

Per slice / tag accuracy:

| slice/tag | n | accuracy |
|---|---|---|
| architecture | 1 | 0.000 |
| audience | 1 | 0.000 |
| code_explanation | 1 | 1.000 |
| code_generation | 1 | 0.000 |
| code_keyword_low_effort | 1 | 0.000 |
| constraint_preservation | 1 | 1.000 |
| context_grounding | 1 | 0.000 |
| factual | 1 | 0.000 |
| minimal_edit | 1 | 0.000 |
| misleading_cache_keyword | 1 | 0.000 |
| misleading_keywords | 1 | 0.000 |
| moderate_context | 2 | 0.000 |
| multi_constraint | 1 | 0.000 |
| multiple_constraints | 1 | 0.000 |
| negation | 1 | 0.000 |
| negative_constraints | 1 | 0.000 |
| no_context | 1 | 0.000 |
| reasoning | 1 | 1.000 |
| risk | 1 | 1.000 |
| short_context | 5 | 0.600 |
| short_request | 2 | 0.000 |
| temporal_reasoning | 1 | 0.000 |
| writing | 1 | 1.000 |

Head-to-head wins per slice/tag (A = first OUT, B = second OUT):

| slice/tag | A only correct | B only correct |
|---|---|---|
| architecture | 1 | 0 |
| audience | 1 | 0 |
| code_generation | 1 | 0 |
| code_keyword_low_effort | 1 | 0 |
| context_grounding | 1 | 0 |
| factual | 1 | 0 |
| minimal_edit | 1 | 0 |
| misleading_cache_keyword | 1 | 0 |
| misleading_keywords | 1 | 0 |
| moderate_context | 2 | 0 |
| multi_constraint | 1 | 0 |
| multiple_constraints | 1 | 0 |
| negation | 1 | 0 |
| negative_constraints | 1 | 0 |
| no_context | 1 | 0 |
| short_context | 2 | 0 |
| short_request | 2 | 0 |
| temporal_reasoning | 1 | 0 |

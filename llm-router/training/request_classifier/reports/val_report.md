# Report on eval_val.json

### local v2 (val split; used for one error-analysis round, so optimistic)

n=213 · **type accuracy 0.854** · macro-F1 0.855 · ECE(10) 0.077 · top-label Brier 0.111 · complexity exact 0.690, ±1 0.977, MAE 0.333 · latency p50 104µs p95 197µs

| class | support | precision | recall | F1 |
|---|---|---|---|---|
| code_generation | 45 | 0.767 | 0.733 | 0.750 |
| code_understanding | 25 | 0.769 | 0.800 | 0.784 |
| technical_design | 30 | 0.909 | 1.000 | 0.952 |
| analytical_reasoning | 30 | 0.935 | 0.967 | 0.951 |
| writing | 30 | 0.800 | 0.667 | 0.727 |
| factual_lookup | 33 | 1.000 | 0.970 | 0.985 |
| general | 20 | 0.783 | 0.900 | 0.837 |

Confusion (rows = gold, cols = predicted, order as above):

| gold \ pred | code_g | code_u | techni | analyt | writin | factua | genera |
|---|---|---|---|---|---|---|---|
| code_generat | 33 | 6 | 1 | 2 | 3 | 0 | 0 |
| code_underst | 4 | 20 | 0 | 0 | 0 | 0 | 1 |
| technical_de | 0 | 0 | 30 | 0 | 0 | 0 | 0 |
| analytical_r | 1 | 0 | 0 | 29 | 0 | 0 | 0 |
| writing | 5 | 0 | 2 | 0 | 20 | 0 | 3 |
| factual_look | 0 | 0 | 0 | 0 | 0 | 32 | 1 |
| general | 0 | 0 | 0 | 0 | 2 | 0 | 18 |

Reliability (ECE bins): range · count · accuracy · mean confidence

- (0.2, 0.3] · 1 · 0.000 · 0.293
- (0.3, 0.4] · 5 · 0.600 · 0.358
- (0.4, 0.5] · 16 · 0.562 · 0.454
- (0.5, 0.6] · 26 · 0.654 · 0.555
- (0.6, 0.7] · 27 · 0.852 · 0.650
- (0.7, 0.8] · 23 · 0.783 · 0.752
- (0.8, 0.9] · 34 · 0.912 · 0.852
- (0.9, 1.0] · 81 · 1.000 · 0.971

Selective accuracy (abstain below τ):

| τ | coverage | accuracy on accepted |
|---|---|---|
| 0.0 | 1.000 | 0.854 |
| 0.3 | 0.995 | 0.858 |
| 0.4 | 0.972 | 0.865 |
| 0.5 | 0.897 | 0.890 |
| 0.6 | 0.775 | 0.927 |
| 0.7 | 0.648 | 0.942 |
| 0.8 | 0.540 | 0.974 |
| 0.9 | 0.380 | 1.000 |

Per slice / tag accuracy:

| slice/tag | n | accuracy |
|---|---|---|
| boundary | 28 | 0.821 |
| clear | 137 | 0.854 |
| misleading_keyword | 26 | 0.885 |
| multi_intent | 28 | 0.857 |
| negation | 4 | 1.000 |
| noisy | 56 | 0.786 |
| non_english | 17 | 0.941 |
| ood | 7 | 1.000 |
| padded | 57 | 0.895 |
| placeholder_context | 24 | 0.917 |
| trap | 16 | 0.875 |
| very_short | 40 | 0.850 |

### regex (val split)

n=213 · **type accuracy 0.296** · macro-F1 0.328 · ECE(10) 0.003 · top-label Brier 0.125 · complexity exact 0.197, ±1 0.714, MAE 1.089 · latency p50 4µs p95 26µs

| class | support | precision | recall | F1 |
|---|---|---|---|---|
| code_generation | 45 | 1.000 | 0.200 | 0.333 |
| code_understanding | 25 | 0.500 | 0.160 | 0.242 |
| technical_design | 30 | 0.643 | 0.300 | 0.409 |
| analytical_reasoning | 30 | 0.867 | 0.433 | 0.578 |
| writing | 30 | 0.625 | 0.167 | 0.263 |
| factual_lookup | 33 | 1.000 | 0.152 | 0.263 |
| general | 20 | 0.117 | 0.900 | 0.207 |

Confusion (rows = gold, cols = predicted, order as above):

| gold \ pred | code_g | code_u | techni | analyt | writin | factua | genera |
|---|---|---|---|---|---|---|---|
| code_generat | 9 | 0 | 3 | 2 | 3 | 0 | 28 |
| code_underst | 0 | 4 | 0 | 0 | 0 | 0 | 21 |
| technical_de | 0 | 2 | 9 | 0 | 0 | 0 | 19 |
| analytical_r | 0 | 2 | 0 | 13 | 0 | 0 | 15 |
| writing | 0 | 0 | 0 | 0 | 5 | 0 | 25 |
| factual_look | 0 | 0 | 0 | 0 | 0 | 5 | 28 |
| general | 0 | 0 | 2 | 0 | 0 | 0 | 18 |

Reliability (ECE bins): range · count · accuracy · mean confidence

- (0.1, 0.2] · 154 · 0.117 · 0.120
- (0.7, 0.8] · 59 · 0.763 · 0.760

Selective accuracy (abstain below τ):

| τ | coverage | accuracy on accepted |
|---|---|---|
| 0.0 | 1.000 | 0.296 |
| 0.3 | 0.277 | 0.763 |
| 0.4 | 0.277 | 0.763 |
| 0.5 | 0.277 | 0.763 |
| 0.6 | 0.277 | 0.763 |
| 0.7 | 0.277 | 0.763 |

Per slice / tag accuracy:

| slice/tag | n | accuracy |
|---|---|---|
| boundary | 28 | 0.214 |
| clear | 137 | 0.328 |
| misleading_keyword | 26 | 0.231 |
| multi_intent | 28 | 0.214 |
| negation | 4 | 0.000 |
| noisy | 56 | 0.268 |
| non_english | 17 | 0.000 |
| ood | 7 | 0.286 |
| padded | 57 | 0.298 |
| placeholder_context | 24 | 0.333 |
| trap | 16 | 0.125 |
| very_short | 40 | 0.275 |

Head-to-head wins per slice/tag (A = first OUT, B = second OUT):

| slice/tag | A only correct | B only correct |
|---|---|---|
| boundary | 17 | 0 |
| clear | 73 | 1 |
| misleading_keyword | 17 | 0 |
| multi_intent | 18 | 0 |
| negation | 4 | 0 |
| noisy | 31 | 2 |
| non_english | 16 | 0 |
| ood | 5 | 0 |
| padded | 35 | 1 |
| placeholder_context | 14 | 0 |
| trap | 12 | 0 |
| very_short | 23 | 0 |

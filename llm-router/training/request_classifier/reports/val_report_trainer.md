# Validation report (val)

### regex (fixed confidence constants, complexity 3)

n=213 · **type accuracy 0.296** · macro-F1 0.328 · ECE(10) 0.000 · top-label Brier 0.125 · complexity exact 0.197, ±1 0.714, MAE 1.089

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

- (0.1, 0.2] · 154 · 0.117 · 0.117
- (0.7, 0.8] · 59 · 0.763 · 0.763

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

### local raw (T=1)

n=213 · **type accuracy 0.859** · macro-F1 0.860 · ECE(10) 0.070 · top-label Brier 0.099 · complexity exact 0.695, ±1 0.981, MAE 0.324

| class | support | precision | recall | F1 |
|---|---|---|---|---|
| code_generation | 45 | 0.786 | 0.733 | 0.759 |
| code_understanding | 25 | 0.778 | 0.840 | 0.808 |
| technical_design | 30 | 0.909 | 1.000 | 0.952 |
| analytical_reasoning | 30 | 0.935 | 0.967 | 0.951 |
| writing | 30 | 0.800 | 0.667 | 0.727 |
| factual_lookup | 33 | 1.000 | 0.970 | 0.985 |
| general | 20 | 0.783 | 0.900 | 0.837 |

Confusion (rows = gold, cols = predicted, order as above):

| gold \ pred | code_g | code_u | techni | analyt | writin | factua | genera |
|---|---|---|---|---|---|---|---|
| code_generat | 33 | 6 | 1 | 2 | 3 | 0 | 0 |
| code_underst | 3 | 21 | 0 | 0 | 0 | 0 | 1 |
| technical_de | 0 | 0 | 30 | 0 | 0 | 0 | 0 |
| analytical_r | 1 | 0 | 0 | 29 | 0 | 0 | 0 |
| writing | 5 | 0 | 2 | 0 | 20 | 0 | 3 |
| factual_look | 0 | 0 | 0 | 0 | 0 | 32 | 1 |
| general | 0 | 0 | 0 | 0 | 2 | 0 | 18 |

Reliability (ECE bins): range · count · accuracy · mean confidence

- (0.3, 0.4] · 3 · 0.333 · 0.338
- (0.4, 0.5] · 16 · 0.500 · 0.467
- (0.5, 0.6] · 16 · 0.625 · 0.554
- (0.6, 0.7] · 20 · 0.850 · 0.648
- (0.7, 0.8] · 30 · 0.867 · 0.748
- (0.8, 0.9] · 29 · 0.759 · 0.859
- (0.9, 1.0] · 99 · 1.000 · 0.972

Selective accuracy (abstain below τ):

| τ | coverage | accuracy on accepted |
|---|---|---|
| 0.0 | 1.000 | 0.859 |
| 0.3 | 1.000 | 0.859 |
| 0.4 | 0.986 | 0.867 |
| 0.5 | 0.911 | 0.897 |
| 0.6 | 0.836 | 0.921 |
| 0.7 | 0.746 | 0.931 |
| 0.8 | 0.601 | 0.945 |
| 0.9 | 0.465 | 1.000 |

Per slice / tag accuracy:

| slice/tag | n | accuracy |
|---|---|---|
| boundary | 28 | 0.821 |
| clear | 137 | 0.869 |
| misleading_keyword | 26 | 0.885 |
| multi_intent | 28 | 0.857 |
| negation | 4 | 1.000 |
| noisy | 56 | 0.786 |
| non_english | 17 | 0.941 |
| ood | 7 | 1.000 |
| padded | 57 | 0.877 |
| placeholder_context | 24 | 0.917 |
| trap | 16 | 0.875 |
| very_short | 40 | 0.875 |

### local + temperature (T=1.157)

n=213 · **type accuracy 0.859** · macro-F1 0.860 · ECE(10) 0.073 · top-label Brier 0.106 · complexity exact 0.695, ±1 0.981, MAE 0.324

| class | support | precision | recall | F1 |
|---|---|---|---|---|
| code_generation | 45 | 0.786 | 0.733 | 0.759 |
| code_understanding | 25 | 0.778 | 0.840 | 0.808 |
| technical_design | 30 | 0.909 | 1.000 | 0.952 |
| analytical_reasoning | 30 | 0.935 | 0.967 | 0.951 |
| writing | 30 | 0.800 | 0.667 | 0.727 |
| factual_lookup | 33 | 1.000 | 0.970 | 0.985 |
| general | 20 | 0.783 | 0.900 | 0.837 |

Confusion (rows = gold, cols = predicted, order as above):

| gold \ pred | code_g | code_u | techni | analyt | writin | factua | genera |
|---|---|---|---|---|---|---|---|
| code_generat | 33 | 6 | 1 | 2 | 3 | 0 | 0 |
| code_underst | 3 | 21 | 0 | 0 | 0 | 0 | 1 |
| technical_de | 0 | 0 | 30 | 0 | 0 | 0 | 0 |
| analytical_r | 1 | 0 | 0 | 29 | 0 | 0 | 0 |
| writing | 5 | 0 | 2 | 0 | 20 | 0 | 3 |
| factual_look | 0 | 0 | 0 | 0 | 0 | 32 | 1 |
| general | 0 | 0 | 0 | 0 | 2 | 0 | 18 |

Reliability (ECE bins): range · count · accuracy · mean confidence

- (0.3, 0.4] · 5 · 0.400 · 0.350
- (0.4, 0.5] · 18 · 0.611 · 0.456
- (0.5, 0.6] · 22 · 0.636 · 0.557
- (0.6, 0.7] · 27 · 0.852 · 0.656
- (0.7, 0.8] · 24 · 0.792 · 0.748
- (0.8, 0.9] · 35 · 0.914 · 0.852
- (0.9, 1.0] · 82 · 1.000 · 0.972

Selective accuracy (abstain below τ):

| τ | coverage | accuracy on accepted |
|---|---|---|
| 0.0 | 1.000 | 0.859 |
| 0.3 | 1.000 | 0.859 |
| 0.4 | 0.977 | 0.870 |
| 0.5 | 0.892 | 0.895 |
| 0.6 | 0.789 | 0.929 |
| 0.7 | 0.667 | 0.944 |
| 0.8 | 0.549 | 0.974 |
| 0.9 | 0.385 | 1.000 |

Per slice / tag accuracy:

| slice/tag | n | accuracy |
|---|---|---|
| boundary | 28 | 0.821 |
| clear | 137 | 0.869 |
| misleading_keyword | 26 | 0.885 |
| multi_intent | 28 | 0.857 |
| negation | 4 | 1.000 |
| noisy | 56 | 0.786 |
| non_english | 17 | 0.941 |
| ood | 7 | 1.000 |
| padded | 57 | 0.877 |
| placeholder_context | 24 | 0.917 |
| trap | 16 | 0.875 |
| very_short | 40 | 0.875 |

Suggested τ (accuracy on accepted ≥ 0.85 at max coverage): 0.0 (coverage 1.000)

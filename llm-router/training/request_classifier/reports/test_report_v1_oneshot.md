# Report on eval_test.json

### local (test split, run once)

n=219 · **type accuracy 0.763** · macro-F1 0.769 · ECE(10) 0.117 · top-label Brier 0.162 · complexity exact 0.685, ±1 0.986, MAE 0.329 · latency p50 103µs p95 236µs

| class | support | precision | recall | F1 |
|---|---|---|---|---|
| code_generation | 46 | 0.692 | 0.783 | 0.735 |
| code_understanding | 23 | 1.000 | 0.652 | 0.789 |
| technical_design | 30 | 0.677 | 0.700 | 0.689 |
| analytical_reasoning | 35 | 0.735 | 0.714 | 0.725 |
| writing | 30 | 1.000 | 0.700 | 0.824 |
| factual_lookup | 30 | 0.839 | 0.867 | 0.852 |
| general | 25 | 0.657 | 0.920 | 0.767 |

Confusion (rows = gold, cols = predicted, order as above):

| gold \ pred | code_g | code_u | techni | analyt | writin | factua | genera |
|---|---|---|---|---|---|---|---|
| code_generat | 36 | 0 | 0 | 3 | 0 | 1 | 6 |
| code_underst | 8 | 15 | 0 | 0 | 0 | 0 | 0 |
| technical_de | 3 | 0 | 21 | 4 | 0 | 0 | 2 |
| analytical_r | 0 | 0 | 5 | 25 | 0 | 4 | 1 |
| writing | 5 | 0 | 2 | 2 | 21 | 0 | 0 |
| factual_look | 0 | 0 | 1 | 0 | 0 | 26 | 3 |
| general | 0 | 0 | 2 | 0 | 0 | 0 | 23 |

Reliability (ECE bins): range · count · accuracy · mean confidence

- (0.3, 0.4] · 5 · 0.200 · 0.334
- (0.4, 0.5] · 5 · 0.200 · 0.453
- (0.5, 0.6] · 11 · 0.636 · 0.571
- (0.6, 0.7] · 14 · 0.643 · 0.644
- (0.7, 0.8] · 17 · 0.353 · 0.752
- (0.8, 0.9] · 32 · 0.750 · 0.851
- (0.9, 1.0] · 135 · 0.881 · 0.978

Selective accuracy (abstain below τ):

| τ | coverage | accuracy on accepted |
|---|---|---|
| 0.0 | 1.000 | 0.763 |
| 0.3 | 1.000 | 0.763 |
| 0.4 | 0.977 | 0.776 |
| 0.5 | 0.954 | 0.789 |
| 0.6 | 0.904 | 0.798 |
| 0.7 | 0.840 | 0.810 |
| 0.8 | 0.763 | 0.856 |
| 0.9 | 0.616 | 0.881 |

Per slice / tag accuracy:

| slice/tag | n | accuracy |
|---|---|---|
| boundary | 14 | 0.429 |
| clear | 156 | 0.821 |
| misleading_keyword | 21 | 0.571 |
| multi_intent | 12 | 1.000 |
| negation | 15 | 0.333 |
| noisy | 61 | 0.705 |
| non_english | 14 | 0.429 |
| ood | 16 | 0.812 |
| padded | 50 | 0.760 |
| placeholder_context | 30 | 0.800 |
| trap | 18 | 0.500 |
| very_short | 23 | 0.783 |

### regex (test split)

n=219 · **type accuracy 0.224** · macro-F1 0.216 · ECE(10) 0.062 · top-label Brier 0.158 · complexity exact 0.228, ±1 0.699, MAE 1.073 · latency p50 5µs p95 21µs

| class | support | precision | recall | F1 |
|---|---|---|---|---|
| code_generation | 46 | 0.467 | 0.152 | 0.230 |
| code_understanding | 23 | 1.000 | 0.087 | 0.160 |
| technical_design | 30 | 0.333 | 0.067 | 0.111 |
| analytical_reasoning | 35 | 0.400 | 0.057 | 0.100 |
| writing | 30 | 1.000 | 0.233 | 0.378 |
| factual_lookup | 30 | 0.438 | 0.233 | 0.304 |
| general | 25 | 0.131 | 0.880 | 0.228 |

Confusion (rows = gold, cols = predicted, order as above):

| gold \ pred | code_g | code_u | techni | analyt | writin | factua | genera |
|---|---|---|---|---|---|---|---|
| code_generat | 7 | 0 | 2 | 3 | 0 | 0 | 34 |
| code_underst | 0 | 2 | 0 | 0 | 0 | 2 | 19 |
| technical_de | 3 | 0 | 2 | 0 | 0 | 2 | 23 |
| analytical_r | 0 | 0 | 0 | 2 | 0 | 2 | 31 |
| writing | 5 | 0 | 2 | 0 | 7 | 0 | 16 |
| factual_look | 0 | 0 | 0 | 0 | 0 | 7 | 23 |
| general | 0 | 0 | 0 | 0 | 0 | 3 | 22 |

Reliability (ECE bins): range · count · accuracy · mean confidence

- (0.1, 0.2] · 168 · 0.131 · 0.120
- (0.7, 0.8] · 51 · 0.529 · 0.760

Selective accuracy (abstain below τ):

| τ | coverage | accuracy on accepted |
|---|---|---|
| 0.0 | 1.000 | 0.224 |
| 0.3 | 0.233 | 0.529 |
| 0.4 | 0.233 | 0.529 |
| 0.5 | 0.233 | 0.529 |
| 0.6 | 0.233 | 0.529 |
| 0.7 | 0.233 | 0.529 |

Per slice / tag accuracy:

| slice/tag | n | accuracy |
|---|---|---|
| boundary | 14 | 0.571 |
| clear | 156 | 0.199 |
| misleading_keyword | 21 | 0.238 |
| multi_intent | 12 | 0.333 |
| negation | 15 | 0.000 |
| noisy | 61 | 0.180 |
| non_english | 14 | 0.000 |
| ood | 16 | 0.375 |
| padded | 50 | 0.220 |
| placeholder_context | 30 | 0.300 |
| trap | 18 | 0.000 |
| very_short | 23 | 0.478 |

Head-to-head wins per slice/tag (A = first OUT, B = second OUT):

| slice/tag | A only correct | B only correct |
|---|---|---|
| boundary | 0 | 2 |
| clear | 97 | 0 |
| misleading_keyword | 9 | 2 |
| multi_intent | 8 | 0 |
| negation | 5 | 0 |
| noisy | 32 | 0 |
| non_english | 6 | 0 |
| ood | 7 | 0 |
| padded | 27 | 0 |
| placeholder_context | 16 | 1 |
| trap | 9 | 0 |
| very_short | 7 | 0 |

# Report on eval_test.json

> **Provenance.** Final-code `classifier_eval` run on 2026-10-03 with `CLASSIFIER_BACKEND=hosted`. Settings: endpoint `https://openrouter.ai/api/v1`, model `mistralai/ministral-8b-2512` (Ministral 3 8B), `CLASSIFIER_TIMEOUT_MS=1500` (default), `EVAL_VERBOSE=1`.
> - 217/219 answered by the hosted model (194 correct). 2 hit the 1.5 s timeout and fell back to regex (0 correct). The accuracy below includes those regex answers.
> - The model returns no logprobs on this route, so every hosted confidence is the fixed default 0.7 (uncalibrated). That is why ECE is about 0.19.
> - Reproducibility: a second identical run scored 0.890. Of the 217 items answered in both runs, 216 (99.5%) gave the same type and 211 (97.2%) the same type and complexity.
> - Cost per call was measured on the cascade's escalations (same request shape; see `test_report_cascade.md`). The hosted-only runs were not cost-instrumented.
> - An earlier saved run on Bedrock (`mistral.ministral-3-8b-instruct`, pre-audit code) scored 0.840 with 20/219 timeouts.

### hosted OpenRouter mistralai/ministral-8b-2512 (test)

n=219 · **type accuracy 0.886** · macro-F1 0.879 · ECE(10) 0.193 · top-label Brier 0.131 · complexity exact 0.607, ±1 0.982, MAE 0.411 · latency p50 427809µs p95 1007495µs

| class | support | precision | recall | F1 |
|---|---|---|---|---|
| code_generation | 46 | 1.000 | 0.935 | 0.966 |
| code_understanding | 23 | 1.000 | 0.739 | 0.850 |
| technical_design | 30 | 1.000 | 0.900 | 0.947 |
| analytical_reasoning | 35 | 0.853 | 0.829 | 0.841 |
| writing | 30 | 0.875 | 0.933 | 0.903 |
| factual_lookup | 30 | 0.824 | 0.933 | 0.875 |
| general | 25 | 0.688 | 0.880 | 0.772 |

Confusion (rows = gold, cols = predicted, order as above):

| gold \ pred | code_g | code_u | techni | analyt | writin | factua | genera |
|---|---|---|---|---|---|---|---|
| code_generat | 43 | 0 | 0 | 0 | 0 | 2 | 1 |
| code_underst | 0 | 17 | 0 | 4 | 0 | 2 | 0 |
| technical_de | 0 | 0 | 27 | 0 | 1 | 0 | 2 |
| analytical_r | 0 | 0 | 0 | 29 | 1 | 2 | 3 |
| writing | 0 | 0 | 0 | 0 | 28 | 0 | 2 |
| factual_look | 0 | 0 | 0 | 0 | 0 | 28 | 2 |
| general | 0 | 0 | 0 | 1 | 2 | 0 | 22 |

Reliability (ECE bins): range · count · accuracy · mean confidence

- (0.1, 0.2] · 2 · 0.000 · 0.120
- (0.6, 0.7] · 217 · 0.894 · 0.700

Selective accuracy (abstain below τ):

| τ | coverage | accuracy on accepted |
|---|---|---|
| 0.0 | 1.000 | 0.886 |
| 0.3 | 0.991 | 0.894 |
| 0.4 | 0.991 | 0.894 |
| 0.5 | 0.991 | 0.894 |
| 0.6 | 0.991 | 0.894 |
| 0.7 | 0.991 | 0.894 |

Per slice / tag accuracy:

| slice/tag | n | accuracy |
|---|---|---|
| boundary | 14 | 0.714 |
| clear | 156 | 0.929 |
| misleading_keyword | 21 | 0.952 |
| multi_intent | 12 | 1.000 |
| negation | 15 | 0.667 |
| noisy | 61 | 0.918 |
| non_english | 14 | 0.786 |
| ood | 16 | 0.625 |
| padded | 50 | 0.800 |
| placeholder_context | 30 | 0.900 |
| trap | 18 | 0.889 |
| very_short | 23 | 0.957 |

### local (test)

n=219 · **type accuracy 0.758** · macro-F1 0.764 · ECE(10) 0.072 · top-label Brier 0.154 · complexity exact 0.685, ±1 0.986, MAE 0.329 · latency p50 60µs p95 134µs

| class | support | precision | recall | F1 |
|---|---|---|---|---|
| code_generation | 46 | 0.679 | 0.783 | 0.727 |
| code_understanding | 23 | 1.000 | 0.652 | 0.789 |
| technical_design | 30 | 0.690 | 0.667 | 0.678 |
| analytical_reasoning | 35 | 0.735 | 0.714 | 0.725 |
| writing | 30 | 1.000 | 0.700 | 0.824 |
| factual_lookup | 30 | 0.812 | 0.867 | 0.839 |
| general | 25 | 0.657 | 0.920 | 0.767 |

Confusion (rows = gold, cols = predicted, order as above):

| gold \ pred | code_g | code_u | techni | analyt | writin | factua | genera |
|---|---|---|---|---|---|---|---|
| code_generat | 36 | 0 | 0 | 3 | 0 | 1 | 6 |
| code_underst | 8 | 15 | 0 | 0 | 0 | 0 | 0 |
| technical_de | 4 | 0 | 20 | 4 | 0 | 0 | 2 |
| analytical_r | 0 | 0 | 5 | 25 | 0 | 4 | 1 |
| writing | 5 | 0 | 2 | 2 | 21 | 0 | 0 |
| factual_look | 0 | 0 | 1 | 0 | 0 | 26 | 3 |
| general | 0 | 0 | 1 | 0 | 0 | 1 | 23 |

Reliability (ECE bins): range · count · accuracy · mean confidence

- (0.2, 0.3] · 3 · 0.000 · 0.278
- (0.3, 0.4] · 7 · 0.143 · 0.358
- (0.4, 0.5] · 14 · 0.714 · 0.465
- (0.5, 0.6] · 22 · 0.545 · 0.550
- (0.6, 0.7] · 26 · 0.731 · 0.650
- (0.7, 0.8] · 30 · 0.600 · 0.765
- (0.8, 0.9] · 34 · 0.824 · 0.859
- (0.9, 1.0] · 83 · 0.940 · 0.959

Selective accuracy (abstain below τ):

| τ | coverage | accuracy on accepted |
|---|---|---|
| 0.0 | 1.000 | 0.758 |
| 0.3 | 0.986 | 0.769 |
| 0.4 | 0.954 | 0.789 |
| 0.5 | 0.890 | 0.795 |
| 0.6 | 0.790 | 0.827 |
| 0.7 | 0.671 | 0.844 |
| 0.8 | 0.534 | 0.906 |
| 0.9 | 0.384 | 0.940 |

Per slice / tag accuracy:

| slice/tag | n | accuracy |
|---|---|---|
| boundary | 14 | 0.429 |
| clear | 156 | 0.814 |
| misleading_keyword | 21 | 0.571 |
| multi_intent | 12 | 1.000 |
| negation | 15 | 0.333 |
| noisy | 61 | 0.705 |
| non_english | 14 | 0.429 |
| ood | 16 | 0.812 |
| padded | 50 | 0.740 |
| placeholder_context | 30 | 0.800 |
| trap | 18 | 0.500 |
| very_short | 23 | 0.739 |

Head-to-head wins per slice/tag (A = first OUT, B = second OUT):

| slice/tag | A only correct | B only correct |
|---|---|---|
| boundary | 5 | 1 |
| clear | 24 | 6 |
| misleading_keyword | 9 | 1 |
| negation | 5 | 0 |
| noisy | 14 | 1 |
| non_english | 6 | 1 |
| ood | 0 | 3 |
| padded | 7 | 4 |
| placeholder_context | 5 | 2 |
| trap | 7 | 0 |
| very_short | 5 | 0 |

# Report on eval_test.json

> **Provenance.**
> - **Run:** `classifier_eval` on commit `5597a59a` (2026-10-03).
> - **Configuration:** `CLASSIFIER_BACKEND=hosted`, endpoint `https://openrouter.ai/api/v1`, model `mistralai/ministral-8b-2512` (Ministral 3 8B), default `CLASSIFIER_TIMEOUT_MS=1500`, `EVAL_VERBOSE=1`.
> - **Answers:** 219/219 from the hosted model; 0 timeouts; 0 regex fallbacks.
> - **Calibration:** the model returns no logprobs on this route, so every hosted confidence is the fixed default 0.7 (uncalibrated). That is why ECE is about 0.19.
> - **Run-to-run variance (best-effort determinism):**
>   - Two earlier runs with the same classify code scored 0.886 (2 timeouts → regex) and 0.890.
>   - Against those runs, this run agrees on type for 217/217 and 217/219 items answered in both, and on type and complexity for 214/217 and 211/219.
> - **Cost:** hosted-only runs were not cost-instrumented. Per-call cost was measured on cascade escalations; see `test_report_cascade.md`.
> - **Earlier comparison:** a Bedrock run (`mistral.ministral-3-8b-instruct`, pre-audit code) scored 0.840 with 20/219 timeouts.

### hosted OpenRouter mistralai/ministral-8b-2512 (test)

n=219 · **type accuracy 0.895** · macro-F1 0.888 · ECE(10) 0.195 · top-label Brier 0.132 · complexity exact 0.626, ±1 0.982, MAE 0.393 · latency p50 414174µs p95 716412µs

| class | support | precision | recall | F1 |
|---|---|---|---|---|
| code_generation | 46 | 1.000 | 0.935 | 0.966 |
| code_understanding | 23 | 1.000 | 0.739 | 0.850 |
| technical_design | 30 | 1.000 | 0.933 | 0.966 |
| analytical_reasoning | 35 | 0.857 | 0.857 | 0.857 |
| writing | 30 | 0.875 | 0.933 | 0.903 |
| factual_lookup | 30 | 0.824 | 0.933 | 0.875 |
| general | 25 | 0.733 | 0.880 | 0.800 |

Confusion (rows = gold, cols = predicted, order as above):

| gold \ pred | code_g | code_u | techni | analyt | writin | factua | genera |
|---|---|---|---|---|---|---|---|
| code_generat | 43 | 0 | 0 | 0 | 0 | 2 | 1 |
| code_underst | 0 | 17 | 0 | 4 | 0 | 2 | 0 |
| technical_de | 0 | 0 | 28 | 0 | 1 | 0 | 1 |
| analytical_r | 0 | 0 | 0 | 30 | 1 | 2 | 2 |
| writing | 0 | 0 | 0 | 0 | 28 | 0 | 2 |
| factual_look | 0 | 0 | 0 | 0 | 0 | 28 | 2 |
| general | 0 | 0 | 0 | 1 | 2 | 0 | 22 |

Reliability (ECE bins): range · count · accuracy · mean confidence

- (0.6, 0.7] · 219 · 0.895 · 0.700

Selective accuracy (abstain below τ):

| τ | coverage | accuracy on accepted |
|---|---|---|
| 0.0 | 1.000 | 0.895 |
| 0.3 | 1.000 | 0.895 |
| 0.4 | 1.000 | 0.895 |
| 0.5 | 1.000 | 0.895 |
| 0.6 | 1.000 | 0.895 |
| 0.7 | 1.000 | 0.895 |

Per slice / tag accuracy:

| slice/tag | n | accuracy |
|---|---|---|
| boundary | 14 | 0.714 |
| clear | 156 | 0.942 |
| misleading_keyword | 21 | 0.952 |
| multi_intent | 12 | 1.000 |
| negation | 15 | 0.667 |
| noisy | 61 | 0.918 |
| non_english | 14 | 0.857 |
| ood | 16 | 0.625 |
| padded | 50 | 0.800 |
| placeholder_context | 30 | 0.900 |
| trap | 18 | 0.889 |
| very_short | 23 | 0.957 |

### local (test)

n=219 · **type accuracy 0.758** · macro-F1 0.764 · ECE(10) 0.072 · top-label Brier 0.154 · complexity exact 0.685, ±1 0.986, MAE 0.329 · latency p50 66µs p95 158µs

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
| clear | 26 | 6 |
| misleading_keyword | 9 | 1 |
| negation | 5 | 0 |
| noisy | 14 | 1 |
| non_english | 7 | 1 |
| ood | 0 | 3 |
| padded | 7 | 4 |
| placeholder_context | 5 | 2 |
| trap | 7 | 0 |
| very_short | 5 | 0 |

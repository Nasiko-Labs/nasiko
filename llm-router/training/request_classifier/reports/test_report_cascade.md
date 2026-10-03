# Report on eval_test.json

> **Provenance.**
> - **Run:** `classifier_eval` on commit `5597a59a` (2026-10-03).
> - **Configuration:** `CLASSIFIER_BACKEND=cascade`, hosted OpenRouter `mistralai/ministral-8b-2512`, `CLASSIFIER_ESCALATE_BELOW=0.6`, `CLASSIFIER_TIMEOUT_MS=1500` (hosted budget 1.2 s), `EVAL_VERBOSE=1`.
> - **Stderr accounting:** `escalations=46 (failed 0)`; `cascade answers: local=173 hosted=46 local-after-failed-escalation=0 regex=0 unclassified=0`.
> - **Escalations:** 46/219 (21.0%), exactly the 46 test items where local confidence is below 0.6.
>   - 37 of the hosted answers are correct; the local answers for those items would have scored 23.
>   - The 173 non-escalated items (143 correct) are byte-identical to the local run.
> - **Selective escalation, not higher accuracy.** The cascade (0.822) is less accurate than hosted alone (0.895 in the same session). It sends 21% of requests to the provider and answers the rest in-process.
> - **Latency:** p50 and p95 cover all 219 calls. Escalated items: p50 414 ms, max 1.026 s, network included. Non-escalated items: p50 274 µs, versus 59–66 µs in local-only runs on the same code path; the cause was not investigated.
> - **Calibration:** hosted confidences are the fixed 0.7 default (no logprobs), so the cascade ECE mixes calibrated local and uncalibrated hosted confidences.
> - **Cost, measured on a separate instrumented cascade run (same commit):**
>   - **Method:** a local proxy forwarded requests unchanged and recorded OpenRouter's per-response `usage`. `CLASSIFIER_TIMEOUT_MS=5000` was used so the extra hop would not cause timeouts; that run's latency is not reported.
>   - **Result:** 46 escalations, 45 answered and billed. 1 (`wr-0054-v2`) hit the 4 s hosted budget and kept the local answer. Accuracy 0.822.
>   - **Billed:** $0.00089493 in total, $1.99e-5 mean per billed call (range $1.05e-5–$2.73e-5), $4.09e-6 per request over 219. All 45 calls hit the provider's prompt cache; ~652 prompt + 3 completion tokens each.
>   - **Earlier instrumented run:** 46 billed calls, $0.00139092 in total, $3.02e-5 mean. One uncached call cost $9.72e-5.
> - **Earlier comparison:** a Bedrock run (pre-audit code) scored 0.822; 5 of its 46 escalations hit the 1.2 s budget and kept the local answer.

### cascade local→hosted OpenRouter mistralai/ministral-8b-2512 (test)

n=219 · **type accuracy 0.822** · macro-F1 0.828 · ECE(10) 0.067 · top-label Brier 0.141 · complexity exact 0.680, ±1 0.982, MAE 0.338 · latency p50 335µs p95 511151µs

| class | support | precision | recall | F1 |
|---|---|---|---|---|
| code_generation | 46 | 0.731 | 0.826 | 0.776 |
| code_understanding | 23 | 1.000 | 0.739 | 0.850 |
| technical_design | 30 | 0.913 | 0.700 | 0.792 |
| analytical_reasoning | 35 | 0.778 | 0.800 | 0.789 |
| writing | 30 | 1.000 | 0.767 | 0.868 |
| factual_lookup | 30 | 0.848 | 0.933 | 0.889 |
| general | 25 | 0.714 | 1.000 | 0.833 |

Confusion (rows = gold, cols = predicted, order as above):

| gold \ pred | code_g | code_u | techni | analyt | writin | factua | genera |
|---|---|---|---|---|---|---|---|
| code_generat | 38 | 0 | 0 | 3 | 0 | 2 | 3 |
| code_underst | 6 | 17 | 0 | 0 | 0 | 0 | 0 |
| technical_de | 3 | 0 | 21 | 4 | 0 | 0 | 2 |
| analytical_r | 0 | 0 | 2 | 28 | 0 | 3 | 2 |
| writing | 5 | 0 | 0 | 1 | 23 | 0 | 1 |
| factual_look | 0 | 0 | 0 | 0 | 0 | 28 | 2 |
| general | 0 | 0 | 0 | 0 | 0 | 0 | 25 |

Reliability (ECE bins): range · count · accuracy · mean confidence

- (0.6, 0.7] · 72 · 0.778 · 0.682
- (0.7, 0.8] · 30 · 0.600 · 0.765
- (0.8, 0.9] · 34 · 0.824 · 0.859
- (0.9, 1.0] · 83 · 0.940 · 0.959

Selective accuracy (abstain below τ):

| τ | coverage | accuracy on accepted |
|---|---|---|
| 0.0 | 1.000 | 0.822 |
| 0.3 | 1.000 | 0.822 |
| 0.4 | 1.000 | 0.822 |
| 0.5 | 1.000 | 0.822 |
| 0.6 | 1.000 | 0.822 |
| 0.7 | 0.881 | 0.834 |
| 0.8 | 0.534 | 0.906 |
| 0.9 | 0.384 | 0.940 |

Per slice / tag accuracy:

| slice/tag | n | accuracy |
|---|---|---|
| boundary | 14 | 0.571 |
| clear | 156 | 0.872 |
| misleading_keyword | 21 | 0.762 |
| multi_intent | 12 | 1.000 |
| negation | 15 | 0.467 |
| noisy | 61 | 0.820 |
| non_english | 14 | 0.500 |
| ood | 16 | 0.812 |
| padded | 50 | 0.780 |
| placeholder_context | 30 | 0.900 |
| trap | 18 | 0.611 |
| very_short | 23 | 0.783 |

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
| boundary | 2 | 0 |
| clear | 11 | 2 |
| misleading_keyword | 4 | 0 |
| negation | 2 | 0 |
| noisy | 8 | 1 |
| non_english | 2 | 1 |
| padded | 2 | 0 |
| placeholder_context | 3 | 0 |
| trap | 2 | 0 |
| very_short | 1 | 0 |

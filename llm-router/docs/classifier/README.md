# Request classifier (`[classifier]`, P2)

A pluggable request classifier behind the router's Level 3 tier selection: the regex
vote-count classifier stays the default, and two model backends can be opted into through
configuration, each with regex fallback on any failure — **Jev** (typesafe.ai, hosted) and
**Laya** (Convai Innovations, local, in-process ONNX). Both are asked the same rubric. This
directory holds the contribution's docs; the code lives in `llm-router/src/routing/`.

| Doc | What it covers |
|---|---|
| this file | setup, configuration, how to run the eval and the scorer, requirements matrix, limits |
| [DECISIONS.md](DECISIONS.md) | request path trace, critique of the proposal, architecture decisions |
| [DATASETS.md](DATASETS.md) | labelled data: provenance, label criteria, splits, leakage check |
| [RESULTS.md](RESULTS.md) | measured results for regex, Laya and (not run) Jev; what is and is not verified; runtime feasibility |
| [LAYA.md](LAYA.md) | the local Laya backend: setup script, verified reference behaviour, parity, packaging obstacles |
| [PR-core.md](PR-core.md) | the hackathon PR description (Track, how to run, model ids, results, limits) |
| [COMPANION.md](COMPANION.md) / [PR-companion.md](PR-companion.md) | the optional UI/server companion (outside the hackathon scope) |

## Quick start

Regex (default; no network, no key):

```sh
curl -fsSL https://registry.nasiko.dev/r/nasiko/classifier-eval -o /tmp/classifier-eval.json
EVAL_SET=/tmp/classifier-eval.json OUT=/tmp/classifier-out.jsonl \
cargo run --release -p nasiko-llm-router --example classifier_eval
```

Jev (hosted, opt-in):

```sh
CLASSIFIER_BACKEND=jev TYPESAFE_API_KEY=… \
EVAL_SET=/tmp/classifier-eval.json OUT=/tmp/classifier-out.jsonl \
cargo run --release -p nasiko-llm-router --example classifier_eval
```

Laya (local, opt-in; one-time ~1.7 GB download, no key):

```sh
llm-router/scripts/laya-setup.sh                     # fetches the pinned ONNX bundle + onnxruntime into ./.laya
CLASSIFIER_BACKEND=laya CLASSIFIER_MODEL_PATH=$PWD/.laya/model \
CLASSIFIER_ORT_DYLIB=$PWD/.laya/onnxruntime/lib/libonnxruntime.1.28.0.dylib \
EVAL_SET=/tmp/classifier-eval.json OUT=/tmp/classifier-out.jsonl \
cargo run --release -p nasiko-llm-router --example classifier_eval
```

Score a run (reads labels; prints Markdown):

```sh
EVAL_SET=/tmp/classifier-eval.json PRED=/tmp/classifier-out.jsonl \
cargo run --release -p nasiko-llm-router --example classifier_report
```

The eval writes exactly the harness schema to `OUT` (`id`, `request_type`, integer
`complexity`, numeric `confidence`, measured `latency_us`), one row per case in input
order. Everything else (which backend answered each row, fallback reason, hosted
distributions, token usage, model version) goes to `<OUT>.diagnostics.jsonl` (or `DIAG_OUT`)
and a summary goes to stderr. Exit 0 means the eval ran.

## Configuration

All settings are read once at the process boundary by `ClassifierConfig::from_env()` in
`llm-router/src/config.rs`, which `GatewayConfig::from_env()` embeds as `cfg.classifier`.
The host server, the standalone `llm-router` binary and the eval example all build the same
`ClassifierService` from it. Classifier and Jev code never read the environment.

| Var | Default | Meaning |
|---|---|---|
| `CLASSIFIER_BACKEND` | `regex` | `regex`, `jev` (hosted) or `laya` (local). Unknown values log a warning and keep regex. Only the selected model backend is initialized. |
| `CLASSIFIER_ENDPOINT` | `https://api.typesafe.ai/v1/systemone` | The only URL the key is ever sent to. Must be `https://` (plain `http://` only for loopback). Redirects are never followed. |
| `CLASSIFIER_MODEL` | `jev-1.13.0` | Versioned model id. The response's own `model` is recorded per call. `jev-latest` works but is a moving alias and not reproducible. |
| `TYPESAFE_API_KEY` | unset | Required for `jev`. Redacted from `Debug`, logs, errors and payloads. |
| `CLASSIFIER_TIMEOUT_MS` | `3000` | Overall deadline per classification: queueing, connect, every attempt, backoff and validation. Expiry ⇒ regex fallback. |
| `CLASSIFIER_MIN_CONFIDENCE` | `0.0` | Below this request-type probability a hosted answer is an *abstention* (see below). `0` disables. Never applied to regex. |
| `CLASSIFIER_ROUTING_SEED` | unset | When set, tier sampling is seeded from `(seed, provider, request_type, query, learned cells)` so identical state picks the same tier. Unset keeps the legacy entropy RNG. |
| `CLASSIFIER_MAX_CONCURRENCY` | `8` | In-flight hosted calls; extra callers wait within the deadline. |
| `CLASSIFIER_RETRIES` | `0` | Extra attempts after 429/529/5xx/connect failures, inside the same deadline. Keep `0` in the routing hot path. |
| `CLASSIFIER_MODEL_PATH` | unset | Laya: directory with `laya.onnx`, `laya.onnx.data`, `laya_config.json`, `tokenizer/` (from `scripts/laya-setup.sh`). |
| `CLASSIFIER_ORT_DYLIB` (or `ORT_DYLIB_PATH`) | unset | Laya: the ONNX Runtime shared library to load at run time. |
| `CLASSIFIER_THREADS` | `0` | Laya: ONNX Runtime intra-op threads; `0` = runtime default. |

With defaults nothing is contacted, downloaded or required. For Laya, `CLASSIFIER_TIMEOUT_MS`
around `6000` fits CPU latency better than the hosted-oriented default (see RESULTS.md).

## How it fits the router

```
query (+ bounded context) ──► ClassifierService ──► Classification {request_type, complexity, confidence}
                                   │                      │
                       regex (default) or Jev        request_type ──► Thompson tier ──► registry ──► model
                       with regex fallback           complexity: observational only (logged, not used for tier)
```

- `RequestClassifier` (trait), `RegexClassifier`, `ClassifyInput`, `Classification`,
  `ClassifyError`: `routing/classifier.rs`.
- `ClassifierService` (validation, timing, deadline, fallback counters, abstention):
  `routing/classifier_service.rs`. Every caller — router, eval, UI preview — goes through it.
- `JevClassifier`: `routing/jev.rs`. One `POST` with the query/context as `state` and two
  questions (Choice over the seven types, Score over the five levels).
- `LayaClassifier`: `routing/laya.rs`. The same two questions, built into the model's token
  sequence and run in-process through ONNX Runtime; see LAYA.md.
- The shared rubric both model backends send: `routing/rubric.rs` (`RUBRIC_VERSION`).
- Context extraction: `routing/context.rs` (chat IR) and `handlers/responses.rs`
  (Responses `input`), both via `routing::classifier_context`.
- Router integration: `routing/mod.rs::route_model` Level 3; `LlmRouterCtx.classifier`.
- Eval I/O and scorer arithmetic: `routing/classifier_eval.rs`; examples
  `classifier_eval.rs`, `classifier_report.rs`.

Routing precedence is unchanged: pinned → cache hit → salience gate → **classify** →
config → default. Classification runs only at `cold_start`/`switch` boundaries in
`free_flowing` mode on a cache miss; tool-loop `continue` turns and cache hits never
reach it. A `NoopCache` (no `REDIS_URL`) provides no stickiness, exactly as before.

### Confidence and abstention

- Regex reports fixed, **uncalibrated** values: complexity 3; confidence 0.5 when a pattern
  voted, 0.3 when `General` was the fallthrough. They let regex rows share the contract;
  they are not probabilities of correctness. The scorer measures the ECE they produce.
- Jev's and Laya's public confidence is the probability the model assigns to the chosen
  request type (Laya calls this `answer_confidence`, the quantity its calibration targets).
  Each vendor's entropy/spread statistic is kept separately as `vendor_type_confidence`.
  Complexity has no confidence of its own.
- With `CLASSIFIER_MIN_CONFIDENCE > 0`, a hosted answer below the floor is an abstention:
  the router serves the agent's configured model (`RouteSource::LowConfidence`) and caches
  that for the conversation with no tier/request type, so later turns are sticky but no
  feedback is ever credited to it. The eval still writes the hosted label and its low
  confidence (the raw prediction); the sidecar marks the row `abstained`. Backend
  *failures* are different: regex answers, with regex's label and regex's confidence, and
  the reason is counted (`init`, `inference`, `invalid_output`, `network`, `timeout`).

### Complexity

The Score question has five 0-based levels mirroring the rubric (identical text for Jev and
Laya). The served level is the **mode** of the distribution (+1), ties to the lower level. The probability-weighted
expected level is recorded as `complexity_expected` so the rounding rule can be compared
offline from one run. Complexity is predicted and evaluated but does not change tier
selection and is not in the bandit key.

## Requirements → implementation → tests

| Requirement (brief / request) | Implementation | Tests |
|---|---|---|
| `RequestClassifier` trait, equivalent signatures, async, `Send + Sync` | `routing/classifier.rs` | `regex_backend_wraps_legacy_and_reports_fixed_values` |
| Regex default, behaviour unchanged when off | `RegexClassifier` wraps `classify_request_type` unchanged; `ClassifierService::classify` answers regex directly with no gate/truncation | `regex_only_service_answers_directly_without_gate`, `request_type_matches_reference_examples`, routing tests |
| Complexity 1..=5, confidence finite 0..=1, validated | `Classification::validate`; service rejects invalid primary output | `classification_validation_rejects_out_of_contract_values`, `invalid_primary_output_is_rejected_before_it_can_route` |
| Backend chosen by config; config read at the boundary only | `ClassifierConfig::from_env`, `GatewayConfig.classifier` | `classifier_config_defaults_need_no_network_or_key`, `classifier_backend_parses_known_labels_and_rejects_unknown` |
| Regex fallback on load/inference/network/timeout, counted | `ClassifierService` + `FallbackReason` | `every_error_kind_falls_back_to_regex_and_is_counted_once`, `slow_primary_hits_the_deadline_and_falls_back`, `init_failure_counts_an_init_fallback_per_call_and_never_calls_primary` |
| Router holds `Arc<dyn RequestClassifier>`, built once | `LlmRouterCtx.classifier: Arc<ClassifierService>` holding `Arc<dyn RequestClassifier>` | `eligible_boundary_calls_the_classifier_once_with_query_and_context` |
| Eval calls the same path as the router | `run_eval` → `ClassifierService::classify` | `run_eval_writes_one_schema_row_per_case_in_order_with_real_timing` |
| Jev request shape per official docs; query/context separated; no labels | `JevClassifier::request_body` | `request_body_matches_the_documented_api_and_carries_no_labels`, `eval_never_shows_labels_to_the_backend` |
| Response validation (types, labels, ranges, sums, finiteness, argmax) | `jev::parse_response` | `malformed_and_out_of_contract_responses_are_rejected` |
| Score → 1..=5 deterministic conversion, ties defined | `complexity_level_from_probabilities` | `complexity_conversion_covers_every_level_ties_and_bounds` |
| 401/422/429/529/5xx/connect/timeout, bounded retry, no retry by default | `JevClassifier::classify_detailed` | `upstream_statuses_map_to_typed_errors_without_leaking_bodies`, `bounded_retry_recovers_from_a_single_429_and_counts_attempts`, `no_retry_by_default_and_retry_never_exceeds_the_deadline`, `slow_upstream_times_out_within_the_deadline` |
| Bounded input/response, bounded concurrency, no redirects, key only to configured endpoint | size caps, `Semaphore`, `redirect::Policy::none()`, https check | `hosted_input_is_capped_but_regex_sees_the_raw_query`, `oversized_body_is_rejected`, `redirects_are_not_followed`, `init_requires_key_https_and_model` |
| Secrets never in Debug/logs/errors/payloads | `config::Secret` | `secret_is_redacted_in_debug_and_display`, `from_config_jev_without_key_is_an_init_error_not_a_panic` |
| Context: bounded, role-labelled, no system/tool/credentials, Unicode-safe | `routing/context.rs`, Responses adapter | `context.rs` tests |
| Classify only at safe boundaries; sticky through `continue`; cache wins | `route_model` unchanged precedence | `pinned_cache_hit_continue_and_no_conv_paths_never_call_the_classifier`, `tool_loop_stays_sticky` (e2e) |
| Low-confidence behaviour specified; abstention not credited | `RouteSource::LowConfidence`, cache entry without tier/rt | `low_confidence_serves_the_configured_model_and_pins_without_a_learnable_cell`, `abstention_cache_entry_is_sticky_but_never_learns` |
| Provider tier mapping, overrides, registry miss, feedback attribution preserved | untouched registry/cells paths | `per_config_override_and_registry_miss_apply_to_hosted_labels_too`, `feedback_still_credits_the_unchanged_tier_request_type_cell` |
| Deterministic tier selection with a seed; legacy randomness otherwise | `classifier::tier_rng` | `seeded_tier_rng_is_repeatable_and_sensitive_to_state`, `seeded_tier_choice_is_repeatable_and_unseeded_is_legacy` |
| Eval contract: no args, `EVAL_SET`/`OUT`, one row per case, exit codes | `examples/classifier_eval.rs`, `classifier_eval::read_eval_cases` | `read_eval_cases_reads_only_inference_fields_and_rejects_bad_input` |
| Labelled own data, documented criteria, no leakage across splits | `tests/data/classifier/*.json`, `split-manifest.json` | `tests/classifier_data.rs` |
| Scorer: accuracy, per-class, confusion, macro F1, ECE, complexity, latency, fallbacks, selective accuracy, repeatability | `classifier_eval::metrics`, `examples/classifier_report.rs` | `score_computes_accuracy_f1_confusion_ece_complexity_and_latency_by_hand`, `semantic_diff_ignores_latency_and_flags_label_changes` |
| Laya: same contract, same rubric, lazy init only when selected | `routing/laya.rs`, `ClassifierService::from_config` arm | `from_config_laya_without_a_bundle_is_an_init_error_and_loads_nothing` |
| Laya: sequence layout, option/head/state budgets, `[MASK]` scrubbing, collation, temperature buckets + clamp, softmax, conversion, entropy confidence | pure functions in `routing/laya.rs` | `sequence_layout_follows_the_reference_template`, `state_is_truncated_to_the_remaining_room_and_reported`, `oversized_options_are_shrunk_evenly…`, `collate_right_pads_rows_and_markers`, `temperature_buckets_and_clamp_match_the_reference`, `decode_applies_temperature_softmax_and_rejects_bad_logits` |
| Laya: missing/corrupt bundle, bounded blocking queue, timeout fallback | `LayaClassifier::new`, semaphore-before-queue | `missing_or_corrupt_bundle_is_an_init_error_without_touching_the_runtime`, `local_inference_is_repeatable_and_times_out_into_regex` (opt-in) |
| Laya: parity with the PyTorch reference | `tests/data/classifier/laya-reference-public.json` | `local_bundle_reproduces_the_pytorch_reference_on_the_public_sample` (opt-in) |
| Live/local checks opt-in; CI needs no key or model | `tests/jev_live.rs`, `tests/laya_local.rs` (`#[ignore]`) | — |

Mandatory items above are all implemented. **Proposed decisions** (ours, not the brief's):
abstention semantics, the mode conversion rule, the seeded RNG derivation, the fixed regex
confidence pair. **Stretch work not done:** tool preselection, stop/continue decisions,
reasoning budgets, a complexity-aware tier policy (deliberately left observational). See
DECISIONS.md for the reasoning and RESULTS.md for what remains unverified.

## Known limits and unsupported cases

- **Live Jev results are unverified in this checkout**: no `TYPESAFE_API_KEY` was available,
  so every hosted number in RESULTS.md is marked "not run — credentials/billing unavailable".
  The adapter is verified against the documented contract with a local HTTP mock only.
- **Laya is slow on CPU and memory-hungry**: ~0.85 s p50 per decision and ~2 GB resident
  here; its 512-token window sees only ~200 tokens of query+context (reported per call as
  truncation). It is clearly better than regex on these sets but misreads technical writing
  as code and reasoning as explanation; see RESULTS.md.
- Two new workspace dependencies (`ort`, `tokenizers`) and a run-time ONNX Runtime library
  come with Laya; see LAYA.md "Packaging obstacles".
- Jev documents no seed/temperature; cross-run repeatability is measured by
  `tests/jev_live.rs` when a key is present, not assumed.
- Jev 1.13 is documented as sensitive to option order and to unrelated context; the
  adapter uses a fixed canonical order and bounded context, and the live test measures the
  order effect. No reordering/ensembling is done in routing.
- Only text is classified. Image/audio/file parts contribute nothing to query or context.
- The packed-history A2A form (`…\n\nCurrent message: …`) yields a query but no context.
- Regex complexity/confidence are placeholders, not estimates.
- No downstream routing-quality experiment was run (no credentials); this contribution
  claims classification accuracy only, never cheaper or better routing.
- Non-English input is accepted; Jev documents lower accuracy outside English.

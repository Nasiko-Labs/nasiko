# Decisions and critique

## 1. How a request actually flows (as of this checkout)

Traced from `llm-router/src/handlers/chat.rs` (`chat_core` → `resolve_routed_request`) and
`handlers/responses.rs` (`responses_core`), both of which converge on
`routing::route_model`.

1. **Auth** — `authenticate_request` verifies the agent-identity JWT (`auth.rs`) and yields
   `(agent_id, owner_id)`.
2. **Inbound parse** — `inbound_for(format).parse_chat(body)` normalizes OpenAI, Anthropic
   and Gemini bodies to the chat IR; the Responses surface keeps raw JSON `input`.
3. **Request signals** — `latest_user_query` (last `user` message, with the A2A
   `Current message:` prefix stripped), `user_turn_ordinal`, `is_tool_continuation`, and
   now `context_from_messages` / `classifier_context`.
4. **Provider resolution** — `resolver::resolve` fixes provider, credentials, params and
   the configured/default model, plus `pinned_model` and per-config `tierN_model`.
5. **Boundary signals** — a coding-agent integration derives them from the transcript
   (`BoundarySignals::for_coding_agent`: new prompt ⇒ `Switch`, trailing tool result ⇒
   `Continue`, `conv_id` hashed from the turn); every other agent needs a `traceparent`
   that attributes to a live flow (`attribution::resolve`), which yields `in_flow` signals
   (`Switch`, mode from the flow). No attributable flow ⇒ 403 for non-coding agents.
6. **Routing precedence** (`route_model`): pinned → cache hit (`(conv_id, agent_id)`; the
   current turn's feedback `signal` is credited to the cached `(tier, request_type)` cell
   here) → salience gate on a cache miss at a fireable boundary (small talk is served the
   cheapest model and **not** cached) → classify → config → default.
7. **Classification + tier** — request type, then Thompson sampling over the provider's
   learned cells, then per-config override or `TierRegistry::model_for(provider, tier)`;
   the decision is written through to the cache. Registry miss ⇒ configured model.
8. **Inference** — `fallback::execute_chat{,_stream}` over the resolved provider with
   ordered fallbacks (cleared when pinned); usage rows are written fire-and-forget.
9. **Feedback** — next turn, step 6 credits the cached cell from the user's message.

Two facts that shaped the design: the decision cache is `NoopCache` unless `REDIS_URL` is
set (no stickiness at all then; every in-flow call is `Switch` and re-classifies), and the
`ThreadRng` used for Thompson sampling is `!Send`, so it must be scoped before any await.

## 2. Critique of the proposal

| Proposal | Verdict | What was done |
|---|---|---|
| "Backend choice … read in the binary's `config.rs`" | The router has no binary-only config: `GatewayConfig::from_env` is shared by the server mount and the standalone binary. | Added `ClassifierConfig` inside `GatewayConfig`, with its own `from_env` reused by the example. Nothing below it reads env. |
| Regex confidence as a fixed value | Fine as a contract filler, but one constant hides the difference between "a pattern voted" and "nothing matched, General by default". | Two documented constants (0.5 / 0.3); `classify_request_type_scored` exposes the vote count without changing `classify_request_type`. Still labelled uncalibrated. |
| Confidence gate | Must not change default routing. | Applied only to a hosted primary; a regex-configured service has no gate. Default floor 0.0 (off) because it cannot be tuned without live data; the calibration split exists to set it. |
| Abstention = "safe default" | Needed precision. Serving the configured model without caching would re-query Jev every in-flow turn (every call is `Switch`), amplifying cost. | Abstention is cached with no tier/request type: sticky, never credited, distinct from fallback in every diagnostic. |
| Complexity → bandit key | Would change learned-cell semantics and require a feedback simulation. | Left out. Complexity is observational (logged, evaluated, surfaced). |
| Score conversion "on development data" | No key, so no data. | Frozen rule: mode, ties low; the expected value is recorded per row so the alternative can be compared offline from the same run. Declared, not tuned. |
| Retries | Amplify cost in the hot path. | `CLASSIFIER_RETRIES=0` default; retries only on 429/529/5xx/connect and only inside the one deadline. |
| First-call latency | The baseline example billed the regex table compile (~10 ms) to row 1. | `warm_regex_tables()` at service init; init time reported separately. |
| Diff of repeated outputs | `latency_us` is measured and will differ. | Kept real timing; `classifier_report` compares the semantic fields across two runs (`PRED2`). Flagged for the organizers in RESULTS.md. |
| Context forwarding | "Relevant context" is underspecified and dangerous. | Bounded, role-labelled, last four user/assistant turns; system, tool results, tool calls and non-text parts excluded; packed-history prefix not mined. |
| Local backend option | No real local backend in this slice. | Not exposed; `CLASSIFIER_BACKEND` accepts only `regex|jev`. The service's `with_primary` is the seam a local backend would use. |

## 3. Architecture decisions

- **One inference path.** `ClassifierService` owns validation, timing, deadline, fallback
  and abstention. The router, the eval and the UI preview call it; backends implement only
  `RequestClassifier`.
- **Diagnostics outside the core result.** `Classification` stays three fields;
  `classify_detailed` returns optional `BackendDiagnostics` with the distributions, vendor
  confidences, usage and model version. Nothing in the hot path depends on them.
- **Typed wire structs, not `serde_json::Value`, for the Jev request.** The workspace's
  `serde_json` has no `preserve_order`; a `Value` would alphabetize the Choice options and
  silently fix the order Jev is sensitive to. `OrderedMap` keeps the canonical order and
  lets the live test reverse it.
- **Fail closed on contract violations.** Every documented guarantee (labels, level keys,
  ranges, sum≈1, finiteness, choice=argmax, answer set) is checked; violations are invalid
  output and fall back. A 2xx is never trusted on its own.
- **Credential hygiene.** `Secret` newtype; dedicated reqwest client with
  `redirect::Policy::none()`; https-only unless loopback; errors carry status codes and
  never bodies.
- **Seeded tier RNG derivation.** `hash(seed, provider, request_type, query, sorted cells)`
  so repeatability holds for identical inputs/config/state while different queries still
  explore. Unset seed ⇒ `rand::rng()` as before, byte-for-byte the legacy path.
- **Eval contract kept narrow.** `OUT` has exactly the five harness fields in a fixed key
  order; diagnostics live in a sidecar. Labels are not parsed by the inference reader.
- **Data in `llm-router/tests/data/classifier/`** so the hackathon scope (`llm-router/` plus
  tests) holds; a cargo test enforces the split policy.

## 3b. Laya decisions (second iteration)

- **In-process, not a sidecar.** The Python package and its `laya-serve` HTTP server were used
  only to verify behaviour and record reference outputs; the shipped backend runs the
  published ONNX export through `ort` with the checkpoint's tokenizer, so the judges' run
  needs no Python and no manually started service. Cost: two workspace dependencies and a
  run-time ONNX Runtime library (`load-dynamic`, so nothing is downloaded at build time).
- **One rubric for both models** (`routing/rubric.rs`). The first Laya reference run used a
  hand-typed variant of the complexity instruction and disagreed with the Rust port by 0.09
  in probability; with the exact text the gap is 5e-5. The rubric is therefore exported from
  the Rust source for any reference run, never retyped.
- **Semaphore before queue.** A timed-out local inference cannot be cancelled, so the permit
  is taken inside the deadline before `spawn_blocking`; the backlog is bounded by
  `CLASSIFIER_MAX_CONCURRENCY` however many callers give up.
- **Token-level truncation is reported, not hidden.** Laya's state budget is ~200 tokens
  with this rubric; `BackendDiagnostics.input_truncated` carries it to the eval sidecar and
  the UI.
- **Temperature clamp follows the Python package**, not the TypeScript port (which uses the
  shipped out-of-range `choice:11+` value raw). Irrelevant for 7 options, documented anyway.
- **Defaults unchanged.** The 3 s timeout and 0.0 floor stay; RESULTS.md says what a Laya
  deployment should set instead and why.

## 4. Alternatives considered and not taken

- A `complexity`-aware tier policy (e.g. complexity ≥ 4 ⇒ never Tier3). Plausible, but it
  would be a routing-quality claim with no downstream measurement behind it. Left as a
  separately opt-in follow-up once a downstream experiment exists.
- Caching hosted answers across runs for "determinism". Rejected: it would hide the thing
  the brief asks to measure.
- Ensembling two option orders per decision. Doubles cost per decision for an effect that
  should be measured first (`tests/jev_live.rs`).
- A Python worker process managed by the router for Laya. Simpler to write, but it would
  make a 2 GB PyTorch install a run-time requirement of the router and leave a second
  process to supervise; the ONNX path removes both.
- Exporting ONNX ourselves from the pinned safetensors. The published export is pinned,
  checksummed and reproduces PyTorch to 5e-5, so re-exporting adds a torch dependency for no
  measured gain.

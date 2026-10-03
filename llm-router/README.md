# nasiko-llm-router

A provider-agnostic, OpenAI-compatible **egress proxy** for user-uploaded agents.

Agents are deployed pointed at this router (`OPENAI_BASE_URL`) with a Nasiko identity
JWT as their `OPENAI_API_KEY` (not a real provider key). The router verifies the JWT,
looks up the agent's provider/model/key in Postgres, decrypts the owner's real key,
and forwards the call to OpenAI / Anthropic / Gemini — translating both directions so
the agent never knows which provider answered. Provider + model are a **runtime config
change** (one `agents.llm_config` update), with no agent redeploy.

This is a **library crate** mounted in-process by `nasiko-server` (it has no dependency
on the server — everything it needs comes via [`LlmRouterCtx`]). It's structured to be
promotable to a standalone binary later without logic changes.

## Request path

```
agent (OpenAI SDK)
  → POST {OPENAI_BASE_URL}/chat/completions   Authorization: Bearer <nasiko-JWT>
  → Pingora gateway strips the /llm prefix  →  server mounts this router at /v1
  → verify JWT → (agent_id, owner_id)                     [auth.rs]
  → resolve provider/model/key (request.model DISCARDED)  [resolver/]   (TTL-cached)
  → translate + call provider, with ordered fallbacks     [providers/] [fallback.rs]
  → return OpenAI shape (JSON or SSE stream)               [inbound/openai.rs]
  → fire-and-forget usage row → token_usage               [usage.rs]
```

The request's hardcoded `model` is **always ignored** (C4); the registry/default model
is authoritative on every path (chat, stream, embeddings).

## HTTP surface

| Route | Notes |
|---|---|
| `POST /v1/chat/completions` | OpenAI Chat Completions, streaming + non-streaming |
| `POST /v1/embeddings` | OpenAI embeddings (OpenAI + Gemini; Anthropic 501) |
| `GET /v1/models` | static provider/model catalog (public) |
| `GET /v1/health` | liveness (`{"status":"ok"}`) |

Errors are `{"detail": "<msg>"}` with the right status (401 auth / 400 client / 500
internal / 502 upstream-after-fallbacks).

## Layout

```
src/
  lib.rs        router(ctx) + LlmRouterCtx (db, http, cfg, cache)
  config.rs     GatewayConfig (env)
  auth.rs       agent-identity JWT verify (+ mint_agent_token dev helper)
  error.rs      GatewayError → (status, {"detail"})
  resolver/     resolve() + TTL ConfigCache + RegistryStore (PgRegistry)
  ir/           canonical OpenAI-shaped IR (chat + embeddings), permissive/passthrough
  inbound/      InboundParser + OpenAiInbound (identity)
  providers/    ProviderClient + openai / anthropic / gemini, sse, fallback
  usage.rs      token_usage writer (fire-and-forget; cost via DB trigger)
  handlers/     chat / embeddings / models / health
examples/mint_token.rs   dev/test JWT minter
examples/classifier_eval.rs, classifier_report.rs, dump_classifier_features.rs   request classifier eval/sim/training
src/routing/  model routing: precedence, boundaries, cache, classifier backends, salience gate
training/request_classifier/   labelled data + trainer for the local classifier
```

## Configuration (env)

`AGENT_JWT_SECRET` (required; fail-closed if empty), `AGENT_JWT_ALGORITHM` (HS256),
`DEFAULT_PROVIDER` (openai), `DEFAULT_MODEL` (gpt-4o-mini), `PLATFORM_OPENAI_API_KEY`,
`LLM_CONFIG_CACHE_TTL` (30s), `{OPENAI,ANTHROPIC,GEMINI}_API_BASE` (test overrides).
Reuses the platform's `SECRETS_ENCRYPTION_KEY` (per-user HKDF AES-256-GCM) and
`DATABASE_URL`.

Storage: `agents.llm_config` (JSONB; NULL → defaults), `user_secrets` (decrypt via
`SecretsCrypto::try_for_user`), `token_usage` (written), `model_pricing` (cost trigger).

## Model routing & request classifier

`routing::route_model` picks the model for each call. The rules are applied in order, and the first match wins:

1. **Pinned.** Compliance-locked agents always use their pinned model.
2. **Sticky decision cache.** The cache is keyed on `(conv_id, agent_id)`.
3. **Salience gate.** Small talk gets the cheap tier and is not pinned.
4. **Classifier + Thompson tier selection.** Runs only on a cache miss at a safe boundary (`cold_start` / `switch` + `free_flowing`).
5. **Configured or default model.**

Tool-loop (`continue`) turns never reclassify: in-flow tool continuations and Anthropic `tool_result` turns are `continue` (`BoundarySignals::in_flow_turn`, `routing::turn_anchor`). The tier→model mapping stays provider-specific: per-config `tier{1,2,3}_model`, else the price-ranked `model_registry` / catalog.

**Request classifier** (`routing::classifier::RequestClassifier`):

```rust
classify(&ClassifyInput { query, context }) -> Result<Classification { request_type, complexity, confidence }, ClassifyError>
```

- `request_type` is one of the 7 `RequestType`s, labelled by the primary deliverable; see `training/request_classifier/LABELING.md`.
- `complexity` is 1–5: *1 trivial single operation; 2 straightforward; 3 multi-step with limited constraints; 4 substantial reasoning or design; 5 intricate cross-component reasoning and validation*.
- `confidence` is a calibrated estimate of P(`request_type` is correct).

| Backend | What it is |
|---|---|
| `regex` (**default**) | The existing vote counter, unchanged. It reports complexity 3 (fixed, so complexity routing does nothing), and its confidence is a fixed constant: its measured validation accuracy, 0.76 when a pattern fired and 0.12 when it defaulted to `general` |
| `local` | Embedded linear model over hashed n-grams plus structural features (`routing::request_features`). Temperature-scaled type head, Frank–Hall complexity head. No network, deterministic, ~100 µs p50, 1.4 MB embedded |
| `hosted` | Any OpenAI-compatible endpoint. The model answers with a letter+digit code (e.g. `C4`), and confidence comes from token logprobs (or a configured default). Memoized; `temperature: 0`, `seed: 7` |
| `cascade` | Runs `local` first and escalates to `hosted` only when local confidence is below `CLASSIFIER_ESCALATE_BELOW`. If the escalation fails it keeps the local answer |

Every backend is wrapped in `GuardedClassifier`. It applies `CLASSIFIER_TIMEOUT_MS`, and on any load, inference or network error or a timeout it returns the regex result and counts a fallback by reason (`ClassifierStats`). A timeout can only interrupt async backends; the local model is synchronous and never yields. `build_request_classifier` is the only constructor, shared by the router and `examples/classifier_eval.rs`. When below `CLASSIFIER_MIN_CONFIDENCE`, Level 3 serves the configured model **without pinning** (`RouteSource::LowConfidence`). Logs carry lengths and a hash, never prompt text.

| Env var | Default | Meaning |
|---|---|---|
| `CLASSIFIER_BACKEND` | `regex` | `regex` \| `local` \| `hosted` \| `cascade`. An unknown value warns and falls back to `regex` |
| `CLASSIFIER_MODEL_PATH` | embedded | Weights file for `local` |
| `CLASSIFIER_TIMEOUT_MS` | `1500` | Classification budget; on expiry the regex answers |
| `CLASSIFIER_MIN_CONFIDENCE` | `0.0` (off) | Abstain threshold. **Recommended: `0.6`** with `local` (0.87 accuracy at 74% coverage out-of-fold) |
| `ROUTER_TIER_SEED` | unset | u64 seed. Tier selection becomes a pure function of `(seed, provider, agent, conv_id, query)` and the learned cells |
| `CLASSIFIER_COMPLEXITY_ROUTING` | `false` | Opt-in complexity prior shift + guardrail. With confidence ≥ the guard, c ≥ 4 never takes Tier 3 and c ≤ 2 never takes Tier 1. The bandit key stays `(tier, request_type)` |
| `CLASSIFIER_COMPLEXITY_GUARD_CONFIDENCE` | `0.6` | Minimum confidence for the guardrail |
| `CLASSIFIER_ENDPOINT` / `CLASSIFIER_MODEL` / `CLASSIFIER_API_KEY` | — | Hosted endpoint (base URL ending in `/v1`), model id, key. An empty key sends no auth header; it never falls back to platform keys |
| `CLASSIFIER_HOSTED_LOGPROBS` / `CLASSIFIER_HOSTED_DEFAULT_CONFIDENCE` | `true` / `0.7` | Hosted confidence source |
| `CLASSIFIER_ESCALATE_BELOW` | `0.6` | Cascade threshold |
| `ROUTER_DECISION_L1_CAPACITY` | `0` (off) | In-process sticky decision cache in front of Redis (or of no cache) |

With none of these set, routing behaves as before. The one intentional exception is that orchestrated tool-loop turns no longer reclassify on a cache miss.

```sh
curl -fsSL https://registry.nasiko.dev/r/nasiko/classifier-eval -o /tmp/classifier-eval.json
EVAL_SET=/tmp/classifier-eval.json OUT=/tmp/classifier-out.jsonl \
  cargo run --release -p nasiko-llm-router --example classifier_eval   # local by default; CLASSIFIER_BACKEND=regex = baseline
cargo run --release -p nasiko-llm-router --example classifier_report    # tier-mix simulation (EVAL_SET=...)
```

Measured on our held-out test split (219 synthetic items, never trained on):

| Backend | Accuracy | ECE |
|---|---|---|
| `local` | 0.758 | 0.072 |
| `regex` | 0.224 | 0.062 |

Complexity is within ±1 for 98.6% of items. Data, the trainer, reports and known limits are in [`training/request_classifier/`](training/request_classifier/README.md).

## Tests

```sh
cargo test -p nasiko-llm-router      # no external infra needed
```
Provider translation + streaming are tested against the REQUEST_JOURNEY fixtures using
`mockito`; the resolver/handler use a mockable `RegistryStore`, so the full path runs
without Postgres. Crypto byte-compatibility is guarded in `nasiko-secrets`.

## Manual end-to-end smoke

```sh
# 1. mint an agent token (agent_id = agents.id UUID, owner_id = users.id UUID)
export AGENT_JWT_SECRET=dev-secret
TOKEN=$(cargo run -q -p nasiko-llm-router --example mint_token -- <agent-uuid> <owner-uuid>)

# 2. insert an agents row with llm_config + a user_secrets row for the owner, then:
curl -s http://localhost:8080/v1/chat/completions \
  -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' \
  -d '{"model":"gpt-4o","messages":[{"role":"user","content":"hi"}]}'
```
Flipping the provider is a single `agents.llm_config` update (no redeploy); a new
`token_usage` row appears per call.

## Deviations from the original spec

This is the Path-2 (platform-integrated) adaptation of the Python-parity prompt:
Postgres not Mongo, AES-256-GCM (per-user HKDF) not Fernet, cost via the existing
`model_pricing` trigger not a static table, `token_usage` not `llm_usage`, a library
crate not a standalone container, hub-and-spoke IR + traits (multi-inbound-ready;
v1 ships OpenAI inbound only). Full list in `.context/llm-gateway/RUST_PLAN_V1.md` §9.
```

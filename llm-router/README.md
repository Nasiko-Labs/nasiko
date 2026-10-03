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
```

## Configuration (env)

`AGENT_JWT_SECRET` (required; fail-closed if empty), `AGENT_JWT_ALGORITHM` (HS256),
`DEFAULT_PROVIDER` (openai), `DEFAULT_MODEL` (gpt-4o-mini), `PLATFORM_OPENAI_API_KEY`,
`LLM_CONFIG_CACHE_TTL` (30s), `{OPENAI,ANTHROPIC,GEMINI}_API_BASE` (test overrides).
Reuses the platform's `SECRETS_ENCRYPTION_KEY` (per-user HKDF AES-256-GCM) and
`DATABASE_URL`.

Storage: `agents.llm_config` (JSONB; NULL → defaults), `user_secrets` (decrypt via
`SecretsCrypto::try_for_user`), `token_usage` (written), `model_pricing` (cost trigger).

## Tests

```sh
cargo test -p nasiko-llm-router      # no external infra needed
```
Provider translation + streaming are tested against the REQUEST_JOURNEY fixtures using
`mockito`; the resolver/handler use a mockable `RegistryStore`, so the full path runs
without Postgres. Crypto byte-compatibility is guarded in `nasiko-secrets`.

## Compact tool schemas experiment

Set `NASIKO_TOOL_COMPACTION=true` to opt into compact function declarations
for non-streaming chat requests. It defaults to `false`. The router applies it
after routing decisions and before provider dispatch, then validates and
converts compact responses back to standard tool calls. Streaming requests,
unsupported schemas, forced `tool_choice`, prior tool-result turns, and
provider-specific tool fields retain native tool handling. A malformed compact
call produces an upstream error; it is never guessed or passed through.
The protocol and supported schema subset are documented in
[`tool-compact/README.md`](../tool-compact/README.md).

Run the public evaluation sample without a model or API key:

```sh
curl -fsSL https://registry.nasiko.dev/r/nasiko/compact-tools-eval -o /tmp/compact-tools-eval.json
EVAL_SET=/tmp/compact-tools-eval.json OUT=/tmp/out.jsonl \
  cargo run --release -p nasiko-llm-router --example compact_tools_eval
```

The example writes one JSONL line per case and prints local `o200k_base`
request token totals to stderr. Offline runs are deterministic. For live
format-adherence checks, set `PROVIDER_BASE_URL`, `MODEL`, and optionally
`PROVIDER_API_KEY`; the example sends temperature-zero requests through an
OpenAI-compatible `/chat/completions` endpoint and adds `raw_output` and
`live_calls` to each case line. `REQUEST_TIMEOUT_SECS` defaults to 60.
The call grammar matches the public decoder cases, so their chunks are fed
to `StreamDecoder` unchanged; no case-specific conversion is needed.

## Phase 2 tool selection

Phase 2 optionally selects a smaller catalog before the Phase 1 encoder. Set
`TOOL_COMPACTION_ENABLED=true` (the existing `NASIKO_TOOL_COMPACTION` flag also
works), `TOOL_SELECTION_MODE=jev`, and provide `TYPESAFE_API_KEY` through the
process environment. Selection is off by default. Each Jev answer maps to an
existing native tool; local policy retains mandatory tools and dependencies,
enforces budgets, and falls back to compact-all on any invalid or failed decision.

For machines with Docker and no Cargo installation, execute the evaluator from
the repository root. The dataset must already exist; the public download command
is shown above. The runner builds the real Rust example and writes measured JSONL.

```sh
./llm-router/scripts/compact-tools-eval.sh off
./llm-router/scripts/compact-tools-eval.sh deterministic
# TYPESAFE_API_KEY must be exported for a live selector run.
TOOL_SELECTION_TIMEOUT_MS=10000 ./llm-router/scripts/compact-tools-eval.sh jev
```

`off` is the original offline P1 evaluation. `deterministic` is a lexical test
baseline, not a guarantee of recall. `jev` uses the actual API and keeps the
generating LLM offline unless `PROVIDER_BASE_URL` and `MODEL` are also set.
Results default to `target/compact-tools-<mode>.jsonl`; override `EVAL_SET` and
`OUT` as needed. Set `CARGO_NET_OFFLINE=true` once Cargo dependencies are cached.

See [tool-selection.md](tool-selection.md) for configuration, cost accounting,
router startup, test commands, and the reconciliation with the Phase 2 spec.

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

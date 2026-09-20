# nasiko-llm-router

A provider-agnostic, OpenAI-compatible **egress proxy** for user-uploaded agents.

Agents are deployed pointed at this router (`OPENAI_BASE_URL`) with a Nasiko identity
JWT as their `OPENAI_API_KEY` (not a real provider key). The router verifies the JWT,
looks up the agent's provider/model/key in Postgres, decrypts the owner's real key,
and forwards the call to OpenAI / Anthropic / Gemini / OpenRouter / Groq / Mistral /
Ollama / Azure OpenAI / NVIDIA NIM — translating both directions so
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
| `POST /v1/embeddings` | OpenAI embeddings (OpenAI / Gemini / Mistral / Ollama / Azure; Anthropic / Groq 501) |
| `POST /v1/route` | DronaHQ contract: tier hint → provider cascade (platform keys). Auth: agent JWT **or** `NASIKO_ROUTE_TOKEN` (stub skips auth). With agent JWT + `agents.tier_cascade=false`, uses the agent's selected llm_config / BYOK provider instead of the cascade. |
| `GET /v1/models` | static provider/model catalog (public) |
| `GET /v1/health` | liveness (`{"status":"ok"}`) |

Errors on the agent JWT surfaces are `{"detail": "<msg>"}` with the right status
(401 auth / 400 client / 500 internal / 502 upstream-after-fallbacks).
`/v1/route` uses `{ "error": { "code", "message", "provider?" } }` per the DronaHQ contract.

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
  providers/    ProviderClient + openai / anthropic / gemini / openrouter / groq / mistral / ollama / azure / nvidia, sse, fallback
  usage.rs      token_usage writer (fire-and-forget; cost via DB trigger)
  handlers/     chat / embeddings / models / route / health
  routing/      model router + DronaHQ `tier.rs` contract cascade
examples/mint_token.rs   dev/test JWT minter
```

## Configuration (env)

`AGENT_JWT_SECRET` (required; fail-closed if empty), `AGENT_JWT_ALGORITHM` (HS256),
`DEFAULT_PROVIDER` (openai), `DEFAULT_MODEL` (gpt-4o-mini), `PLATFORM_OPENAI_API_KEY`,
`PLATFORM_ANTHROPIC_API_KEY`, `PLATFORM_GEMINI_API_KEY`, `PLATFORM_OPENROUTER_API_KEY`,
`PLATFORM_GROQ_API_KEY` (also accepts bare `GROQ_API_KEY`), `PLATFORM_MISTRAL_API_KEY`,
`PLATFORM_OLLAMA_API_KEY`, `PLATFORM_AZURE_OPENAI_API_KEY` / `AZURE_OPENAI_API_KEY`,
`PLATFORM_NVIDIA_API_KEY` / `NVIDIA_API_KEY` (`nvapi-…`),
`NASIKO_ROUTE_STUB=1` (optional; `/v1/route` returns contract-shaped stub replies),
`NASIKO_ROUTE_TOKEN` (required for non-stub `/v1/route` unless a valid agent JWT is presented),
or a valid agent JWT is presented), `NASIKO_OLLAMA=1` (opt-in Ollama in cascades;
also enabled when `OLLAMA_API_BASE` / `PLATFORM_OLLAMA_API_KEY` is set),
provider bases (`GROQ_API_BASE`, `MISTRAL_API_BASE`, `OLLAMA_API_BASE` default
`http://127.0.0.1:11434/v1`, `AZURE_OPENAI_API_BASE` + `AZURE_OPENAI_API_VERSION`,
`NVIDIA_API_BASE` default `https://integrate.api.nvidia.com/v1`), …

Storage: `agents.llm_config` (JSONB; NULL → defaults), `user_secrets` (decrypt via
`SecretsCrypto::try_for_user`), `token_usage` (written), `model_pricing` (cost trigger).

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

# nasiko-llm-router

A provider-agnostic, OpenAI-compatible **egress proxy** for user-uploaded agents.

Agents are deployed pointed at this router (`OPENAI_BASE_URL`) with a Nasiko identity
JWT as their `OPENAI_API_KEY` (not a real provider key). The router verifies the JWT,
looks up the agent's provider/model/key in Postgres, decrypts the owner's real key,
and forwards the call to OpenAI / Anthropic / Gemini — translating both directions so
the agent never knows which provider answered. Provider + model are a **runtime config
change** (one `agents.llm_config` update), with no agent redeploy.

It is a **library crate** mounted in-process by `nasiko-server` (it has no dependency
on the server — everything it needs comes via [`LlmRouterCtx`]); `src/bin/llm-router.rs`
also runs it standalone.

## Request path

```
agent (OpenAI SDK)
  → POST {OPENAI_BASE_URL}/chat/completions   Authorization: Bearer <nasiko-JWT>
  → the server mounts this router at /v1
  → verify JWT → (agent_id, owner_id)                     [auth.rs]
  → resolve provider/model/key (request.model DISCARDED)  [resolver/]   (TTL-cached)
  → translate + call provider, with ordered fallbacks     [providers/] [providers/fallback.rs]
  → return OpenAI shape (JSON or SSE stream)               [inbound/openai.rs]
  → fire-and-forget usage row → trace_usage               [usage.rs]
```

The request's hardcoded `model` is **always ignored** (C4); the registry/default model
is authoritative on every path (chat, stream, embeddings).

## HTTP surface

| Route | Notes |
|---|---|
| `POST /v1/chat/completions` | OpenAI Chat Completions, streaming + non-streaming |
| `POST /v1/responses` | OpenAI Responses |
| `POST /v1/messages`, `POST /v1/messages/count_tokens` | Anthropic Messages (`ANTHROPIC_BASE_URL`) |
| `POST /v1beta/models/{model}:generateContent` / `:streamGenerateContent` | Gemini (`GOOGLE_GEMINI_BASE_URL`) |
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
`LLM_ROUTER_GENERIC_KEY_FALLBACK` (true; when false, the platform key an agent's call may
fall back to comes only from `PLATFORM_*` names, not the generic provider vars like
`OPENAI_API_KEY`; the platform's own calls and the calls a payer sponsors still use the
generic vars),
`LLM_CONFIG_CACHE_TTL` (30s), `OPENAI_BASE_URL`, `ANTHROPIC_BASE_URL`, `GOOGLE_GEMINI_BASE_URL`, `OPENROUTER_BASE_URL` (upstream overrides), `GOOGLE_CLOUD_PROJECT` / `GOOGLE_CLOUD_LOCATION` / `VERTEX_CREDENTIALS_JSON` (Vertex).
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
`trace_usage` row appears per call.

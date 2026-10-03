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
  compact_tools.rs  opt-in compact tool definitions (plan / apply / finalize), see below
examples/mint_token.rs            dev/test JWT minter
examples/compact_tools_eval.rs    evaluation output for compact tool definitions (offline / live)
examples/compact_tools_measure.rs token measurement report: native vs TOON vs compact
examples/support/                 shared code for the two examples (not part of the library)
examples/fixtures/compact-tools/  development and held-out evaluation sets
```

## Configuration (env)

`AGENT_JWT_SECRET` (required; fail-closed if empty), `AGENT_JWT_ALGORITHM` (HS256),
`DEFAULT_PROVIDER` (openai), `DEFAULT_MODEL` (gpt-4o-mini), `PLATFORM_OPENAI_API_KEY`,
`LLM_CONFIG_CACHE_TTL` (30s), `{OPENAI,ANTHROPIC,GEMINI}_API_BASE` (test overrides).
Reuses the platform's `SECRETS_ENCRYPTION_KEY` (per-user HKDF AES-256-GCM) and
`DATABASE_URL`. `TOKEN_COMPACT_TOOLS` (default `false`) turns on compact tool definitions
(below), and `TOKEN_COMPACT_TOOLS_NATIVE_RETRY` (default `true`) re-sends a request natively once
when its compacted reply cannot be decoded; like every `TOKEN_*` flag they accept `true`/`1` and
`false`/`0`.

Storage: `agents.llm_config` (JSONB; NULL → defaults), `user_secrets` (decrypt via
`SecretsCrypto::try_for_user`), `token_usage` (written), `model_pricing` (cost trigger).

## Compact tool calling (opt-in)

With `TOKEN_COMPACT_TOOLS=true`, a non-streaming OpenAI-inbound chat request that carries
function tools is rewritten before dispatch: `tools`, `tool_choice` and `parallel_tool_calls`
are removed and one system message is inserted after the leading system messages, carrying the
tools as compact signature lines plus a short call protocol (see `tool-compact/README.md` for
the grammar). The model answers with `<<call name {json}>>`; the router decodes that, validates
every call against the **original** JSON Schema and rebuilds native `tool_calls` with fresh
`call_…` ids, so the client sees an ordinary chat completion. `id`, `model`, `usage` and
`created` are the provider's.

Everything else keeps today's native path, decided **before** dispatch and recorded as a label:

| bypass label | when |
|---|---|
| `feature_disabled` | the flag is off (the usage row then carries no `compact_tools` block at all) |
| `non_openai_inbound` | Anthropic or Gemini SDK surface |
| `streaming` | `stream: true` |
| `no_tools`, `unsupported_tool_kind` | no function tools |
| `tool_choice_none`, `tool_choice_forced` | only `auto`/absent proceeds |
| `parallel_tool_calls_disabled` | explicit `false` (the grammar cannot cap the number of calls) |
| `response_format` | anything but `{"type":"text"}` |
| `multiple_choices` | `n > 1` |
| `strict_or_unknown_tool_keys` | `function.strict` or any other key the IR would drop (read off the raw body) |
| `history_has_tool_calls_or_results` | prior assistant tool calls or tool results in the transcript |
| `unsupported_schema`, `invalid_catalog`, `limit_exceeded` | from `nasiko-tool-compact` |
| `no_byte_saving` | the compact message is more than ¾ the size of the tools JSON (`compact_tools::MIN_SAVING_RATIO`) |

After dispatch the reply is judged by its representation:

- `<<call …>>` text needs `finish_reason: "stop"`; it is decoded and rebuilt natively.
- Native `tool_calls` (a provider may still emit them) need `finish_reason: "tool_calls"` and are
  validated against the original schemas under the decoder's limits (per-call size, call count,
  aggregate size, all checked before parsing); valid ones are kept as is, any violation rejects
  the whole reply.
- Mixing both, more than one choice, a truncated/filtered/missing completion with executable
  output, an unknown tool, invalid or malformed arguments: the compacted reply is refused. Usage
  is recorded for that billed call first (`decode: <kind>`). Then, with
  `TOKEN_COMPACT_TOOLS_NATIVE_RETRY` on (the default), the original uncompacted request is re-sent
  once through the same fallback chain and logged as a second usage row (`bypass: "native_retry"`,
  `retry_after: <kind>`). Its reply goes through the same finalization against the same original
  schemas, so an invalid native call is still refused, and there is no second retry. With the
  retry off, or when the retried reply is refused, the result is **502**
  `{"detail":"compact tool call decoding failed: <kind>"}`; the body carries only the kind, never
  model text.
- A plain text reply passes through untouched, whatever its finish reason.

The usage row's `metadata.compact_tools` block is present whenever the flag is on:
`{applied, bypass, tool_count, definitions_bytes_in, definitions_bytes_out, decode,
representation, calls}` (sizes are bytes, not tokens).

Not covered yet: streaming through the router, Anthropic/Gemini inbound, conversation history
with earlier tool calls, forced `tool_choice`, the `/v1/responses` surface. Enablement needs both
the fleet flag and the agent's own opt-in (`agents.compact_tools_enabled`, the "Compact tool
definitions" switch on its Settings tab); the per-agent `compress_enabled` switch is deliberately
not reused as consent.

### Evaluation and measurement

```sh
# offline (default): deterministic, no network, one JSONL line per case
EVAL_SET=/tmp/compact-tools-eval.json OUT=/tmp/out.jsonl \
cargo run --release -p nasiko-llm-router --example compact_tools_eval

# live: adds raw_output / live_calls per case, one request at a time
PROVIDER_BASE_URL=https://host/v1 MODEL=some-model PROVIDER_API_KEY=… \
EVAL_SET=… OUT=… cargo run --release -p nasiko-llm-router --example compact_tools_eval

# token measurement report (dev aid): native vs TOON vs compact, o200k_base
OUT=/tmp/measure.md EVAL_SET=/tmp/compact-tools-eval.json \
cargo run --release -p nasiko-llm-router --example compact_tools_measure
```

In live mode each reply is judged by the production finalization: a missing, truncated or
filtered completion that carries call output is `incomplete_completion`, and one with no
executable output counts as a reply with zero calls, exactly as the router passes it through. The
measurement
report's adherence rate is matched divided by every attempted case (matched, mismatched and output
failures); transport errors and skipped cases are listed beside it, not inside it.

Optional env: `MODEL` (also the offline request's `model`), `MAX_OUTPUT_TOKENS` (1024),
`LIVE_TIMEOUT_SECS` (60), `BEDROCK_API_KEY` as an alias of `PROVIDER_API_KEY`, `FIXTURES_DIR`
and `LIVE_VARIANTS` (`native,compact`) for the measurement example. The examples run the same
`compact_tools::plan`/`apply`/`finalize` the handler runs; `compact_request` is the exact body a
live run POSTs. Tests for the examples live beside them and run with
`cargo test -p nasiko-llm-router --examples`.

## Tests

```sh
cargo test -p nasiko-llm-router             # library + handler tests, no external infra needed
cargo test -p nasiko-llm-router --examples  # evaluation/measurement example support
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

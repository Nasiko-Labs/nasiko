# LLM Gateway (Track 2 — LiteLLM integration)

Nasiko now ships with a platform-managed LLM gateway built on
[LiteLLM](https://github.com/BerriAI/litellm). Every agent receives a gateway
URL and a virtual key at deploy time instead of raw provider credentials.

> **Do not hardcode provider API keys** (OpenAI, Groq, OpenRouter, MiniMax,
> Anthropic, …) in agent source, Dockerfiles, or agent-level `.env` files.
> Those keys live **only** inside the gateway container. Agents call the
> gateway; the gateway calls the provider.

---

## Why a gateway?

| Pain without gateway | Gateway fix |
|---|---|
| Provider keys baked into each agent image | One set of keys, held in the gateway only |
| Switching provider = edit every agent | Swap provider in `litellm/config.yaml`, no agent changes |
| No central audit / rate-limit / cost surface | Single choke point the platform can observe |
| Keys leak through `docker inspect`, logs, crash dumps | Agents carry a revocable *virtual* key, not the real one |

---

## LiteLLM vs. Portkey — why LiteLLM

| | LiteLLM | Portkey |
|---|---|---|
| License | Apache 2.0, fully OSS | Managed SaaS (self-host is paid) |
| Providers supported | 100+ | 250+ |
| Self-host friction | Single container, YAML config | Requires account + managed plane |
| Offline / hackathon fit | Runs with no external account | Needs Portkey cloud or enterprise |
| OTEL support | Native OpenTelemetry exporter | Native, but tied to Portkey observability |

LiteLLM was chosen because it is a single self-hosted container, speaks the
OpenAI API shape (zero-SDK-change migration for existing agents), and keeps
the entire data path inside the cluster.

---

## Architecture

```
┌────────────┐   OpenAI-shape    ┌──────────────┐    real key    ┌──────────┐
│  Agent     │ ────────────────► │  LiteLLM     │ ─────────────► │ Groq /   │
│ container  │   virtual key     │  gateway     │                │ OpenAI / │
└────────────┘                   └──────────────┘                │ …        │
       ▲                                │                        └──────────┘
       │                                ▼
       │                        ┌──────────────┐
       └─ OTEL spans ─────────► │   Phoenix    │
                                └──────────────┘
```

- Network: `app-network` + `agents-net` so both platform services and agent
  containers can reach it.
- Default internal URL: `http://litellm:4000`.
- Default host port (for debugging): `4100` (configurable via
  `NASIKO_PORT_LITELLM`).

---

## Using the gateway from an agent

Agents get these env vars injected by the orchestrator's
`_deploy_agent_container` flow:

| Var | Example | Purpose |
|---|---|---|
| `LITELLM_BASE_URL` | `http://litellm:4000` | Gateway endpoint |
| `LITELLM_VIRTUAL_KEY` | `sk-nasiko-…` | Per-agent credential |
| `LITELLM_DEFAULT_MODEL` | `gpt-4o-mini` | Logical model name (mapped by the gateway) |
| `OPENAI_BASE_URL` | `http://litellm:4000` | Alias — lets untouched `openai` SDK code route through the gateway |
| `OPENAI_API_KEY` | `sk-nasiko-…` | Alias of the virtual key (only when the agent has no legacy OpenAI key) |

### New agents — preferred pattern

```python
import os
from openai import AsyncOpenAI

client = AsyncOpenAI(
    api_key=os.environ["LITELLM_VIRTUAL_KEY"],
    base_url=os.environ["LITELLM_BASE_URL"],
)

resp = await client.chat.completions.create(
    model=os.getenv("LITELLM_DEFAULT_MODEL", "gpt-4o-mini"),
    messages=[{"role": "user", "content": "hello"}],
)
```

### Legacy agents — zero code changes

Because the orchestrator also sets `OPENAI_BASE_URL` and (when empty)
`OPENAI_API_KEY` to the virtual key, an agent that instantiates
`openai.OpenAI()` with no arguments transparently hits the gateway. No
source edits required.

### Opting out (legacy direct-provider path)

Set `LITELLM_ENABLED=false` on `nasiko-redis-listener`. The orchestrator
then stops injecting gateway vars and falls back to the previous
provider-key injection. Existing agents that still carry
`OPENROUTER_API_KEY` / `MINIMAX_API_KEY` / `OPENAI_API_KEY` continue to
function unchanged (Track 2 must-not-impact rule).

---

## Provider rotation — gateway-config only

To swap Groq for OpenAI on `gpt-4o-mini`, edit `litellm/config.yaml`:

```diff
  - model_name: gpt-4o-mini
    litellm_params:
-     model: groq/llama-3.1-8b-instant
-     api_key: os.environ/GROQ_API_KEY
+     model: openai/gpt-4o-mini
+     api_key: os.environ/OPENAI_API_KEY
```

Restart the gateway container. No agent image rebuild. No agent env change.

---

## Virtual key provisioning design

The hackathon problem statement leaves the virtual-key lifecycle as a design
deliverable. Track 2 ships this contract:

- **Minting** — `RedisStreamListener._mint_virtual_key_for_agent()` is the
  single provisioning point, called on every agent deployment from
  `_deploy_agent_container`. Today it returns the platform-configured
  `LITELLM_VIRTUAL_KEY` (master-key mode, DB-less). The method signature
  (`agent_name`, `owner_id`) is already shaped for per-tenant minting.
- **Storage** — planned: minted keys stored in Redis under
  `nasiko:litellm:keys:{agent_name}`, with the metadata
  `{owner_id, model_allowlist, minted_at}`. The master key never leaves
  the orchestrator and the gateway.
- **Rotation** — planned: regenerate on each deploy. A redeploy invalidates
  the previous key via LiteLLM's `/key/delete` endpoint before minting a
  replacement, so a leaked key has at most one deploy-cycle of lifetime.
- **Revocation** — planned: on agent teardown (`_cleanup_existing_container`)
  call `/key/delete` so a ghost container can't keep burning spend.

Switching from MVP to the target path is one method body change in
`_mint_virtual_key_for_agent`; no other call sites need to know.

---

## Observability

LiteLLM is configured with `OTEL_EXPORTER_OTLP_ENDPOINT` pointing at
Phoenix. The gateway emits a span per upstream provider call and correlates
with the agent's trace context via the inbound request headers. In the
Phoenix UI you will see:

```
agent-translator (span)
 └── POST /v1/chat/completions  → litellm-gateway (span)
      └── groq/llama-3.1-8b-instant  (upstream provider span)
```

---

## Failure behaviour

Per the hackathon fixed-design decision: **if the gateway is down, model
requests fail clearly.** There is no silent fallback to direct-provider
keys and no client-side queueing. Agents surface the gateway error to the
caller so operators can page on it.

---

## Related files

- `litellm/config.yaml` — model routing and virtual keys
- `docker-compose.local.yml` — `litellm` service definition
- `orchestrator/config.py` — `LITELLM_*` configuration
- `orchestrator/redis_stream_listener.py` — `_deploy_agent_container`,
  `_mint_virtual_key_for_agent`
- `agents/a2a-translator/src/__main__.py` — reference agent migration
- `tests/integration/test_litellm_gateway.py` — acceptance tests

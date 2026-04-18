# Nasiko LLM Gateway

> **Do NOT hardcode model provider API keys in your agent source code.**
> Use the platform-managed LLM gateway instead.

## Why the gateway exists

Embedding `OPENAI_API_KEY`, `ANTHROPIC_API_KEY`, or any direct provider credential in an agent zip creates three problems:

1. **Security** — keys leak in container images, logs, and git history.
2. **Operational cost** — rotating a key requires rebuilding every agent.
3. **Lock-in** — changing provider means touching every agent codebase.

The LLM gateway solves all three: provider credentials live in exactly one place (the gateway config), and agents receive a short-lived *virtual key* that the platform issues and rotates automatically.

---

## Architecture

```
Agent container
  │  LLM_GATEWAY_URL=http://llm-gateway:4000  (injected by orchestrator)
  │  LLM_VIRTUAL_KEY=sk-...                   (per-agent, rotated on redeploy)
  ▼
llm-gateway (LiteLLM proxy)  ←── litellm_config.yaml  ←── .nasiko-local.env
  │  reads OPENAI_API_KEY / OPENROUTER_API_KEY / MINIMAX_API_KEY
  ▼
LLM provider (OpenAI / OpenRouter / MiniMax / …)
  │
  ▼  OTLP spans
phoenix-observability:4318
```

**Gateway** — [LiteLLM proxy](https://docs.litellm.ai/docs/proxy/quick_start) running as `llm-gateway` in the platform Docker network.  
**Trade-off vs Portkey:** LiteLLM is fully self-hostable and open-source; Portkey's advanced features require a SaaS account. For an on-prem platform like Nasiko, LiteLLM is the better fit.

---

## How agents use the gateway

### 1. Read gateway credentials from the environment

```python
import os
from openai import AsyncOpenAI

# Injected by the Nasiko orchestrator — never hardcode these
gateway_url  = os.environ["LLM_GATEWAY_URL"]   # http://llm-gateway:4000
virtual_key  = os.environ["LLM_VIRTUAL_KEY"]   # sk-...
model        = os.environ.get("LLM_MODEL", "gpt-4o-mini")

client = AsyncOpenAI(api_key=virtual_key, base_url=f"{gateway_url}/v1")
```

### 2. Make LLM calls normally

```python
response = await client.chat.completions.create(
    model=model,
    messages=[{"role": "user", "content": user_input}],
)
```

No other code changes are needed. The gateway speaks the OpenAI Chat Completions API so any OpenAI-compatible SDK works out of the box.

### 3. See the working example

`agents/a2a-gateway-demo/` is a complete, runnable agent that follows this pattern.

---

## How the platform deploys the gateway

The gateway starts automatically with the rest of the platform:

```bash
docker compose -f docker-compose.local.yml up -d
# or
make start-nasiko
```

The `llm-gateway` service is defined in `docker-compose.local.yml` between the Router and Observability layers.

---

## Configuration

### Provider credentials (`.nasiko-local.env.example`)

```env
# Gateway master key — used by the orchestrator to mint per-agent virtual keys
LLM_GATEWAY_MASTER_KEY=sk-nasiko-master-key   # CHANGE IN PRODUCTION

# Provider keys — stored only here, not in any agent
OPENAI_API_KEY=sk-...
OPENROUTER_API_KEY=sk-or-...
MINIMAX_API_KEY=...
```

### Switching providers (`llm-gateway/litellm_config.yaml`)

To switch every agent from OpenAI to OpenRouter, change only the gateway config:

```yaml
router_settings:
  model_group_alias:
    default: ["openrouter/nemotron"]   # was ["gpt-4o-mini", "openrouter/nemotron"]
```

Restart the gateway service:

```bash
docker compose -f docker-compose.local.yml restart llm-gateway
```

No agent code change required.

### Adding a new provider

1. Add the provider's API key to `.nasiko-local.env.example` (and your local `.env`).
2. Add a new `model_list` entry in `llm-gateway/litellm_config.yaml`.
3. Restart `llm-gateway`.

---

## Virtual key lifecycle

| Event | Action |
|-------|--------|
| Agent deployed / updated | Orchestrator calls `POST /key/generate` on the gateway → mints `LLM_VIRTUAL_KEY` |
| Agent redeployed | Old key deleted from gateway + Redis; fresh key minted (rotation) |
| Agent torn down | `GatewayKeyManager.revoke_key()` deletes the key |

Keys are stored in Redis under `agent:virtual_key:{agent_name}` so the orchestrator can locate and revoke them on redeploy.

---

## Observability

Every LLM call routed through the gateway produces an OTLP span exported to Phoenix (`http://phoenix-observability:4318`).  
The span is correlated with the calling agent's trace via standard W3C `traceparent` propagation — you will see a single unified trace from *agent turn → gateway call → provider response* in the Phoenix UI.

---

## Legacy agents (backward compatibility)

Agents that still set `OPENAI_API_KEY` directly continue to work unchanged — the orchestrator still injects the raw key for backward compatibility.  
However, **new agents must not embed provider keys**. Use the gateway pattern shown above.

---

## Troubleshooting

| Symptom | Likely cause |
|---------|--------------|
| `LLM_VIRTUAL_KEY is not set` error at agent startup | Agent deployed before gateway was running; redeploy the agent |
| `401 Unauthorized` from gateway | Virtual key expired or rotated; redeploy the agent |
| Gateway health check failing | Check `docker logs llm-gateway`; ensure provider key env vars are set |
| No spans in Phoenix | Confirm `OTEL_ENDPOINT` in `litellm_config.yaml` points to `http://phoenix-observability:4318` |

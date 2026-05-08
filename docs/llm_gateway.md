# LLM Gateway (Track 2)

Nasiko routes all agent LLM calls through a centralized **LiteLLM** gateway.  
Agents never hold real provider API keys — the platform manages them.

---

## Why the Gateway Exists

| Problem | Solution |
|---|---|
| Every agent stores its own `OPENAI_API_KEY` | One secret in gateway config, zero in agent code |
| Switching providers requires code changes | Edit `litellm_config.yaml`, restart gateway |
| No visibility into LLM calls per agent | Every call produces an OpenTelemetry span in Phoenix |

---

## Architecture

```
Agent container
  └─► http://litellm:4000/v1   (virtual-key auth)
        └─► LiteLLM Gateway
              ├─► OpenAI   (gpt-4o, gpt-4o-mini, gpt-4)
              └─► Anthropic (claude-3-opus, claude-3-5-sonnet)

Traces:
  Agent Span
    └─► Gateway Span  (emitted by LiteLLM → Phoenix via OTEL)
          └─► Provider Call
```

---

## How to Use the Gateway in an Agent

```python
import os
from openai import OpenAI

# Both env vars are injected automatically by the Nasiko orchestrator.
# No real API key is needed in agent source code.
client = OpenAI(
    base_url=os.getenv("OPENAI_BASE_URL", "http://litellm:4000/v1"),
    api_key=os.getenv("OPENAI_API_KEY", "virtual-key"),
)

response = client.chat.completions.create(
    model="gpt-4o-mini",
    messages=[{"role": "user", "content": "Hello"}],
)
```

The orchestrator injects the following into every deployed agent container:

```
OPENAI_BASE_URL=http://litellm:4000/v1
OPENAI_API_KEY=virtual-key
```

Agents that already set these variables keep their own values (setdefault behaviour).

---

## Provider Switching

To switch a model to a different provider:

1. Edit `config/litellm_config.yaml`:

```yaml
- model_name: gpt-4o        # alias agents use
  litellm_params:
    model: anthropic/claude-3-5-sonnet-20241022   # new backend
    api_key: os.environ/ANTHROPIC_API_KEY
```

2. Restart the gateway:

```bash
docker compose -f docker-compose.local.yml restart litellm
```

**No agent code change required.**

---

## Observability

LiteLLM emits an OpenTelemetry span for every LLM call to Phoenix:

- Configured via `success_callbacks: ["otel"]` in `litellm_config.yaml` (appears as `OpenTelemetry` in the health endpoint)
- Endpoint: `http://phoenix-observability:6006/v1/traces` (Phoenix serves OTLP HTTP on its main port, not 4318)
- Spans link to the calling agent's active trace automatically

View traces at: `http://localhost:6006`

For agents that want to wrap gateway calls in an explicit child span:

```python
from app.utils.observability import gateway_span

with gateway_span(model="gpt-4o-mini", attributes={"session.id": "abc"}):
    response = client.chat.completions.create(...)
```

---

## Per-Agent Virtual Key Provisioning

Every deployed agent gets its **own** LiteLLM virtual key — not a shared platform key.

```
Deploy agent
  → GatewayKeyManager.mint_key_for_agent(agent_name, owner_id)
      → POST /key/generate  { models, max_budget: $1.00, metadata: {agent_name, owner_id} }
      → Store key in Redis: nasiko:litellm:keys:{agent_name}  (TTL: 24h)
  → Inject LITELLM_VIRTUAL_KEY + OPENAI_API_KEY = <per-agent key>

Teardown agent
  → GatewayKeyManager.revoke_key_for_agent(agent_name)
      → POST /key/delete
      → Remove from Redis
```

**Why this matters:**
- One compromised agent cannot spend another agent's budget
- Per-agent spend is tracked and visible in the LiteLLM dashboard
- Model allowlisting: each agent can only call the models it was provisioned for
- Key rotation happens automatically on every redeploy

**Fallback:** if the gateway is unreachable at deploy time, the shared `virtual-key` is used so deployments never block.

**Toggle off:** set `LITELLM_ENABLED=false` to disable gateway injection entirely — agents receive direct provider keys as before.

---

## Security Policy

- Real provider keys (`OPENAI_API_KEY`, `ANTHROPIC_API_KEY`) exist **only** in `config/litellm_config.yaml` and the host environment.
- Agent source code must never contain real provider keys.
- Agents authenticate to the gateway with `virtual-key` — a non-secret token.
- `config/litellm_config.yaml` is mounted read-only into the gateway container.

---

## Local Development

The gateway starts automatically with:

```bash
make start-nasiko
```

Gateway is reachable at:
- From agent containers: `http://litellm:4000/v1`
- From the host machine: `http://localhost:4001/v1`

Health check: `GET http://localhost:4001/health`

---

## Sample Agent

See [examples/agents/gateway-llm-agent/](../examples/agents/gateway-llm-agent/) for a minimal agent that uses the gateway with no provider keys in source.

# LLM Router Gateway Guide

## What Is the LLM Gateway?

The **LLM Gateway** is a centralized proxy (powered by [LiteLLM](https://github.com/BerriAI/litellm)) that sits between your agents and LLM providers (OpenRouter, OpenAI, Anthropic, MiniMax, etc.).

Instead of hardcoding API keys and provider URLs in your agent code, agents send requests to the gateway using a **virtual key**. The gateway routes requests to the configured provider.

```
Agent → LLM Gateway → OpenRouter / OpenAI / MiniMax / ...
```

### Benefits

| Without Gateway | With Gateway |
|---|---|
| Each agent needs its own API key | One platform key for all agents |
| Changing providers = code change | Changing providers = config change only |
| Keys scattered across agent code | Keys centralized in gateway config |
| No centralized spend tracking | Per-key spend limits and tracking |
| No LLM-level tracing | All LLM calls traced via OpenTelemetry → Phoenix |

> [!CAUTION]
> **Do NOT hardcode LLM provider API keys in your agent source code.** Use the gateway pattern instead. Hardcoded keys are a security risk and make provider switching painful.

---

## Architecture

```
┌─────────────────────────────────────────────────┐
│              Agent Container                     │
│                                                 │
│  LITELLM_GATEWAY_URL=http://litellm-gateway:4000│
│  LITELLM_API_KEY=sk-nasiko-xxxx                 │
│                                                 │
│  client = OpenAI(                               │
│      base_url=LITELLM_GATEWAY_URL + "/v1",      │
│      api_key=LITELLM_API_KEY                    │
│  )                                              │
└──────────────────────┬──────────────────────────┘
                       │ OpenAI-compatible API
                       ▼
┌──────────────────────────────────────────────────┐
│           LiteLLM Gateway (port 4000)            │
│                                                  │
│  • OpenAI-compatible /v1/chat/completions        │
│  • Virtual key validation                        │
│  • Model routing (openrouter/gpt-4o, etc.)       │
│  • OTEL traces → Phoenix                         │
│                                                  │
│  Provider keys stored in gateway config only     │
└──────────────────────┬───────────────────────────┘
                       │
                       ▼
            ┌──────────────────┐
            │  OpenRouter /    │
            │  OpenAI / etc.   │
            └──────────────────┘
```

---

## How to Use the Gateway in Your Agent

### Step 1: Read Environment Variables

The orchestrator automatically injects these into every deployed agent:

| Variable | Description |
|---|---|
| `LITELLM_GATEWAY_URL` | Gateway URL (e.g., `http://litellm-gateway:4000`) |
| `LITELLM_API_KEY` | Virtual key for authentication |

### Step 2: Use the OpenAI SDK (or any OpenAI-compatible client)

```python
import os
from openai import OpenAI

# The gateway URL and key are injected by the orchestrator
client = OpenAI(
    base_url=os.getenv("LITELLM_GATEWAY_URL", "http://litellm-gateway:4000") + "/v1",
    api_key=os.getenv("LITELLM_API_KEY"),
)

response = client.chat.completions.create(
    model="gpt-4o-mini",  # Model name from gateway config
    messages=[{"role": "user", "content": "Hello!"}],
)
print(response.choices[0].message.content)
```

### Step 3: Add Fallback (Optional but Recommended)

For backward compatibility, check for gateway vars first, then fall back to direct keys:

```python
import os
from openai import OpenAI

gateway_url = os.getenv("LITELLM_GATEWAY_URL")
gateway_key = os.getenv("LITELLM_API_KEY")

if gateway_url and gateway_key:
    # Recommended: use the LLM Gateway
    client = OpenAI(base_url=f"{gateway_url}/v1", api_key=gateway_key)
    model = "gpt-4o-mini"
elif os.getenv("OPENROUTER_API_KEY"):
    # Fallback: direct provider key
    client = OpenAI(
        base_url="https://openrouter.ai/api/v1",
        api_key=os.getenv("OPENROUTER_API_KEY"),
    )
    model = "nvidia/nemotron-3-super-120b-a12b:free"
else:
    raise ValueError("No LLM configuration found")
```

---

## Changing LLM Providers

Provider switching is gateway-config only — **no agent code changes needed**.

### 1. Edit `gateway/litellm_config.yaml`

Uncomment or add the provider you want:

```yaml
model_list:
  # Switch to direct OpenAI
  - model_name: "gpt-4o-mini"
    litellm_params:
      model: "gpt-4o-mini"
      api_key: "os.environ/OPENAI_API_KEY"
```

### 2. Restart the gateway

```bash
docker compose -f docker-compose.local.yml --env-file .nasiko-local.env restart litellm-gateway
```

### 3. Done!

Agents continue using the same `LITELLM_GATEWAY_URL` and `LITELLM_API_KEY` — they don't know or care which provider is behind the gateway.

---

## Virtual Key Management

### Auto-generated at Boot

When you start the platform, the `litellm-key-init` service automatically:
1. Waits for the gateway to be healthy
2. Generates a virtual key via the LiteLLM API
3. Stores it in Redis for the orchestrator to read
4. Saves a backup to `gateway/agent_virtual_key.json`

### Manual Key Generation

```bash
# Generate a new key using the provisioner script
uv run gateway/key_provisioner.py --gateway-url http://localhost:4000

# Or use the Makefile shortcut
make gateway-key
```

### Key Rotation

1. Run the key provisioner to generate a new key
2. Update `LITELLM_API_KEY` in `.nasiko-local.env`
3. Redeploy agents to pick up the new key

---

## Viewing Gateway Traces

All LLM calls through the gateway are automatically traced via OpenTelemetry and sent to **Arize Phoenix**.

### In Phoenix UI (http://localhost:6006)

- Filter by service name: `litellm-gateway`
- See model, tokens, latency, and cost per request
- Traces are correlated with agent traces via W3C trace context propagation

### Trace Attributes

| Attribute | Description |
|---|---|
| `gen_ai.request.model` | Model that was called |
| `gen_ai.usage.prompt_tokens` | Input token count |
| `gen_ai.usage.completion_tokens` | Output token count |
| `litellm.api_key_alias` | Which virtual key was used |

---

## Troubleshooting

### Gateway won't start

```bash
# Check gateway logs
docker logs litellm-gateway

# Check if the database is ready
docker logs litellm-db

# Verify config file syntax
cat gateway/litellm_config.yaml
```

### Agent can't reach gateway

```bash
# Verify gateway is healthy
curl http://localhost:4000/health/liveliness

# Check the agent has gateway env vars
docker inspect agent-<name> | grep LITELLM
```

### "Invalid API Key" errors

```bash
# Check what key is in Redis
docker exec redis redis-cli GET litellm:agent_key

# Regenerate the key
uv run gateway/key_provisioner.py
```

### Gateway is down — what happens?

Per the design spec: **model requests fail clearly with no fallback or queueing.** This is intentional. If the gateway is down, agents will get connection errors. Fix the gateway, and agents recover automatically.

---

## Configuration Reference

### Environment Variables

| Variable | Default | Description |
|---|---|---|
| `LITELLM_MASTER_KEY` | `sk-nasiko-master-key` | Admin key for gateway management |
| `LITELLM_SALT_KEY` | `nasiko-salt-key` | Encryption salt for stored credentials |
| `LITELLM_API_KEY` | (auto-generated) | Virtual key for agents |
| `LITELLM_GATEWAY_URL` | `http://litellm-gateway:4000` | Gateway internal URL |
| `NASIKO_PORT_LITELLM` | `4000` | Host port for gateway |
| `OPENROUTER_API_KEY` | (required) | Primary provider key |
| `OPENAI_API_KEY` | (optional) | Additional provider key |
| `MINIMAX_API_KEY` | (optional) | Additional provider key |

### Ports

| Service | Port | Purpose |
|---|---|---|
| LiteLLM Gateway | 4000 | LLM API endpoint |
| LiteLLM DB | (internal) | Virtual key storage |

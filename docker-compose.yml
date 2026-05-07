# How to Use the Nasiko LLM Gateway

This guide explains how to use the platform-managed LLM gateway so your agents
do not need hardcoded model provider API keys.

> **⚠️ Do NOT hardcode model provider API keys in your agent source code.**
> Use the gateway instead.

## Overview

Nasiko runs a [LiteLLM](https://github.com/BerriAI/litellm) proxy as a
platform-managed LLM gateway. Instead of embedding `OPENAI_API_KEY`,
`ANTHROPIC_API_KEY`, etc. in your agent, you point your LLM client at the
gateway.

### Benefits

- **Single endpoint**: `http://llm-gateway:4000` for all providers
- **Virtual key**: `nasiko-virtual-proxy-key` — no real API keys in source
- **Provider switching**: Change OpenAI → Anthropic by editing gateway config,
  not your agent
- **Observability**: Gateway requests are traced and correlated with agent spans
  in Arize Phoenix
- **Security**: Real API keys stay in the gateway's config/secret store

## How It Works

```
┌──────────┐     ┌──────────────┐     ┌──────────────┐
│  Agent    │────>│  LLM Gateway │────>│  OpenAI API  │
│  (your   │     │  (LiteLLM)   │     │  (or any     │
│   code)  │     │              │     │   provider)  │
└──────────┘     └──────────────┘     └──────────────┘
     │                                       │
     └───── traces ──────────────────────────┘
                    Arize Phoenix
```

The gateway:

1. Receives LLM requests from your agent using the virtual key
2. Translates them to the real provider API (OpenAI, Anthropic, etc.)
3. Forwards the response back to your agent
4. Emits traces to Phoenix for observability

## Agent Setup

### LangChain

```python
import os
from langchain_openai import ChatOpenAI

llm = ChatOpenAI(
    base_url=os.environ.get("OPENAI_API_BASE", "http://llm-gateway:4000"),
    api_key=os.environ.get("OPENAI_API_KEY", "nasiko-virtual-proxy-key"),
    model="platform-default-model",
    temperature=0,
)
```

### CrewAI

CrewAI uses LangChain under the hood, so the same environment variables work:

```python
import os
os.environ["OPENAI_API_BASE"] = "http://llm-gateway:4000"
os.environ["OPENAI_API_KEY"] = "nasiko-virtual-proxy-key"

from crewai import Agent, Task, Crew

agent = Agent(
    role="researcher",
    goal="Research topics",
    llm="platform-default-model",
)
```

### Environment Variables

The platform injects these env vars at deploy time. For local dev, set them
manually or use docker-compose:

| Variable | Value | Description |
|----------|-------|-------------|
| `OPENAI_API_BASE` | `http://llm-gateway:4000` | Gateway endpoint |
| `OPENAI_BASE_URL` | `http://llm-gateway:4000` | Alias for some SDKs |
| `OPENAI_API_KEY` | `nasiko-virtual-proxy-key` | Virtual key (not a real key) |
| `ANTHROPIC_API_KEY` | `nasiko-virtual-proxy-key` | Virtual key for Anthropic SDKs |

## Gateway Configuration

The gateway is configured via `litellm-config.yaml`:

```yaml
model_list:
  - model_name: platform-default-model
    litellm_params:
      model: "gpt-4o-mini"
      api_key: "os.environ/OPENAI_API_KEY"

litellm_settings:
  success_callback: ["arize_phoenix"]
  failure_callback: ["arize_phoenix"]
```

### Switching Providers

To switch from OpenAI to Anthropic, change only the gateway config:

```yaml
model_list:
  - model_name: platform-default-model
    litellm_params:
      model: "anthropic/claude-sonnet-4-20250514"
      api_key: "os.environ/ANTHROPIC_API_KEY"
```

No agent code changes needed.

## Deployment

The gateway is deployed automatically as part of `docker-compose.local.yml`:

```bash
docker compose -f nasiko/docker-compose.local.yml up -d
```

This starts:
- `llm-gateway` on port 4000
- `phoenix-observability` on port 6006
- `kong` on ports 8001/8002
- `nasiko-server` on port 8000

## Backward Compatibility

Existing agents that still use direct provider keys will continue to work
unchanged. The gateway is an **alternative**, not a forced migration. However,
new agents should use the gateway pattern for better security and flexibility.

## LiteLLM vs Portkey: Trade-off Analysis

We chose **LiteLLM** for the gateway implementation based on these factors:

| Factor | LiteLLM | Portkey |
|--------|---------|---------|
| Open source | ✅ Fully OSS | ⚠️ OSS core, paid features |
| Self-hosted | ✅ Docker image available | ✅ Docker image available |
| Provider support | ✅ 100+ providers | ✅ 200+ providers |
| Phoenix integration | ✅ Native callbacks | ⚠️ Requires custom setup |
| Complexity | ✅ Simple YAML config | ⚠️ More config options |
| Community | ✅ Large, active | ✅ Growing |

LiteLLM was chosen because it integrates natively with Arize Phoenix via
callbacks, is fully open source, and can be configured with a single YAML file.

# LLM Gateway — Developer Guide

## Overview
Nasiko includes a platform-managed LLM gateway (LiteLLM) so agents do not need to hardcode provider API keys.

## ⚠️ Do NOT hardcode API keys in your agent code
Never do this:
```python
client = OpenAI(api_key="sk-...")  # ❌ Wrong
```

Always do this:
```python
import os
from openai import OpenAI

client = OpenAI(
    api_key=os.environ.get("LLM_VIRTUAL_KEY"),
    base_url=os.environ.get("LLM_GATEWAY_URL"),
)
```

## How it works
- The gateway runs at `http://nasiko-llm-gateway:4000` inside the platform
- Every agent automatically receives these env vars at startup:
  - `LLM_GATEWAY_URL` — the gateway endpoint
  - `LLM_VIRTUAL_KEY` — your virtual API key

## Switching providers
To switch from NVIDIA to OpenAI, only change `litellm_config.yaml` — no agent code changes needed.

## See also
- Sample agent: `sample-agents/gateway-demo-agent/`
- Integration tests: `core/tests/integration/test_track2_llm_gateway.py

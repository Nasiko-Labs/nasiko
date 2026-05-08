# Nasiko LLM Gateway Integration Guide

This guide explains how agent developers can opt into using the centralized LLM Gateway (powered by LiteLLM) rather than requiring users to manually configure their own API keys. 

By using the Gateway, your agent gets:
- **Centralized Provider Key Management:** API keys (OpenAI, Anthropic, OpenRouter) are managed securely by the platform, reducing the risk of token leakage.
- **Improved Cost Observability:** Token spend and performance are automatically tracked per agent alias.
- **Trace Correlation:** Out-of-the-box telemetry hooks link LLM calls to your agent's broader OpenTelemetry traces inside Arize Phoenix.

> [!WARNING]
> **DO NOT HARDCODE PROVIDER KEYS.** Never hardcode `OPENAI_API_KEY`, `ANTHROPIC_API_KEY`, or any other direct model provider credentials inside your agent's source code or ship them hardcoded into your environment configurations. Doing so will result in security violations and token leakage. Always leverage the `LITELLM_VIRTUAL_KEY` injected by the gateway at runtime.

## Architecture Choice: LiteLLM vs. Portkey
During design, we evaluated both LiteLLM and Portkey (OSS) for integration. 
- **Portkey** offers an incredible feature set for enterprise load-balancing and granular observability pipelines but skews heavier in deployment architecture, often encouraging a tether back to their cloud deployment tier.
- **LiteLLM** was chosen because it natively matches our Python-centric operational stack, deploys effortlessly as a standalone proxy inside `docker-compose`, provides dynamic Virtual Keys via a simple administrative REST endpoint perfectly suited for our Orchestrator pattern, and binds seamlessly to OpenTelemetry out-of-the-box (meaning traces automatically connect inside our existing Arize Phoenix layer). 

## How It Works

When your agent is built and deployed by the Nasiko Orchestrator (`make start-nasiko`), the orchestrator dynamically intercepts the deployment and requests a unique **Virtual Key** from the LiteLLM Gateway Admin API.

Your agent container is then injected with these two environment variables:
1. `LITELLM_VIRTUAL_KEY` - A short-lived, dynamically minted proxy key.
2. `GATEWAY_BASE_URL` - The internal network address of the LiteLLM gateway (usually `http://litellm-gateway:4000`).

## Updating Your Agent Code

To use the gateway, simply configure your preferred LLM Client (OpenAI SDK, LangChain, etc.) to securely fall back to the gateway environment variables if standard provider keys aren't explicitly provided.

### Example: OpenAI SDK Integration

```python
import os
from openai import OpenAI

# 1. Determine priority: LITELLM_VIRTUAL_KEY > Provider Keys
virtual_key = os.getenv("LITELLM_VIRTUAL_KEY")
if virtual_key:
    api_key = virtual_key
    # Defaulting to the docker internal DNS if GATEWAY_BASE_URL is missing
    base_url = os.getenv("GATEWAY_BASE_URL", "http://litellm-gateway:4000")
    if not base_url.endswith("/v1"):
        base_url += "/v1"
    model = os.getenv("ROUTER_LLM_MODEL", "gpt-4o-mini")
else:
    # Legacy direct API key approach
    api_key = os.getenv("OPENAI_API_KEY")
    base_url = "https://api.openai.com/v1"
    model = "gpt-4o-mini"
    
if not api_key:
    raise ValueError("No Valid LLM Key Found - Ensure you have deployed via Orchestrator or set OPENAI_API_KEY")

# 2. Initialize your client
client = OpenAI(
    api_key=api_key,
    base_url=base_url
)

# 3. Use it exactly as normal
response = client.chat.completions.create(
    model=model,
    messages=[{"role": "user", "content": "Hello world!"}]
)
```

## Security & Volumes
- When spinning up a fresh environment via `make start-nasiko`, you will note the `start.sh` script currently wipes the litellm postgres volume (`docker volume rm nasiko_litellm-db-data`).
- This means **any previously minted Virtual Key is intentionally deleted/revoked locally on a fresh start**.
- The Orchestrator automatically re-mints a new series of valid agent keys immediately after. This provides a completely clean execution environment and guarantees idempotency. All agent developers just need to restart an agent via the standard flow or use hot-reload configs to sync the new environment arrays.

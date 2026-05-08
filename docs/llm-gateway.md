# LLM Gateway (LiteLLM) in Nasiko

Nasiko ships with a **platform-managed LLM gateway** (`llm-gateway`) powered by LiteLLM.

> **Policy:** Do not hardcode provider API keys inside agent code or agent source bundles.
> Use the platform gateway (`LLM_GATEWAY_URL` + `LLM_VIRTUAL_KEY`) instead.

## Why this exists

Without a gateway, each agent needs direct provider credentials (`OPENAI_API_KEY`, `ANTHROPIC_API_KEY`, etc.).
With the gateway:

- agents talk to one OpenAI-compatible endpoint,
- providers/credentials are centralized in platform config,
- providers can be swapped by config changes (no agent rebuild).

## Gateway choice: LiteLLM vs Portkey

For this hackathon track, Nasiko uses **LiteLLM** because:

- it's self-hostable in our existing compose/orchestrator flow,
- it exposes a familiar OpenAI-compatible API surface,
- it keeps deployment simple (single container + config).

Portkey has strong managed features, but would introduce additional control-plane dependencies for this delivery.

## Local deployment

`docker-compose.local.yml` now includes:

- `llm-gateway` service (LiteLLM),
- config mount from `orchestrator/litellm_config.yaml`,
- provider credentials from env (`OPENAI_API_KEY`, `ANTHROPIC_API_KEY`).

Default internal URL:

```bash
http://llm-gateway:4000
```

## Agent runtime contract

Nasiko injects these environment variables into deployed agents:

- `LLM_GATEWAY_URL` (default `http://llm-gateway:4000`)
- `LLM_VIRTUAL_KEY` (gateway virtual key)
- `LLM_GATEWAY_MODEL` (virtual model alias, default `platform-default`)

Legacy provider variables are still injected for backward compatibility.

## Example client usage (OpenAI SDK)

```python
from openai import AsyncOpenAI
import os

client = AsyncOpenAI(
    api_key=os.environ["LLM_VIRTUAL_KEY"],
    base_url=os.environ["LLM_GATEWAY_URL"],
)

resp = await client.chat.completions.create(
    model=os.getenv("LLM_GATEWAY_MODEL", "platform-default"),
    messages=[{"role": "user", "content": "hello"}],
)
```

## Provider rotation (config-only)

To switch providers without changing agents:

1. Update `orchestrator/litellm_config.yaml` model mapping (for example `openai/...` to `anthropic/...`).
2. Ensure the target provider key is present in gateway env.
3. Restart only the gateway service.

Agent code/images remain unchanged.

## Observability correlation

Sample gateway-enabled agents attach W3C trace headers (`traceparent`/`baggage`) on gateway calls.
This keeps gateway requests correlated with the calling agent trace tree while preserving existing span formats.


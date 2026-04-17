# Agent Authoring Guide

## LLM API Keys — Do Not Hardcode

> **WARNING:** Never hardcode `OPENAI_API_KEY`, `ANTHROPIC_API_KEY`, or any
> other provider API key in your agent source code, `Dockerfile`, or
> `docker-compose.yml`. This is a security violation that will be flagged
> in PR review and rejected.

The Nasiko platform provides a centralized LLM gateway. At deploy time, the orchestrator automatically injects two environment variables into every agent container:

| Variable | Value at runtime | Purpose |
|---|---|---|
| `OPENAI_BASE_URL` | `http://llm-gateway:4000` | Points the OpenAI SDK at the gateway |
| `OPENAI_API_KEY` | `sk-virt-xxxx...` (virtual key) | Per-agent scoped key issued by the gateway |

Your agent code reads these at startup. The gateway translates calls to the real provider (OpenAI, Anthropic, etc.) without your agent ever seeing the provider credential.

---

## The Anti-Pattern (do not do this)

```python
# BAD — hardcoded provider key
import os
from openai import AsyncOpenAI

client = AsyncOpenAI(
    api_key="sk-proj-AbCdEfGhIj...",  # DO NOT DO THIS
)
```

This pattern leaks credentials into source code, container images, and logs. Even if the key is read from an env var but the env var name is a raw provider key (`OPENAI_API_KEY=sk-proj-...` set by the developer), a single misconfigured deployment exposes the full-cost blast radius of that provider account.

---

## The Correct Pattern

```python
import os
from openai import AsyncOpenAI

gateway_url = os.environ.get("OPENAI_BASE_URL") or os.environ.get("LLM_GATEWAY_URL")
virtual_key = os.environ.get("OPENAI_API_KEY")  # injected as virtual key by orchestrator

if not gateway_url or not virtual_key:
    raise ValueError(
        "This agent requires OPENAI_BASE_URL and OPENAI_API_KEY to be injected "
        "by the Nasiko orchestrator. Do not hardcode provider keys."
    )

client = AsyncOpenAI(base_url=gateway_url, api_key=virtual_key)

# All model calls use "default-model" — the gateway resolves the actual provider.
response = await client.chat.completions.create(
    model="default-model",
    messages=[{"role": "user", "content": user_input}],
)
```

See `agents/a2a-gateway-demo/src/gateway_agent_executor.py` for a complete reference implementation including A2A task handling.

---

## Why This Matters

A raw provider key (e.g. `sk-proj-...`) grants the holder full access to your provider account: billing, usage, and all existing projects. If such a key leaks from an agent zip, a log file, or a container image layer, the blast radius is your entire provider account.

A virtual key issued by the Nasiko gateway is scoped to a single agent, has a monthly budget cap (default: $5 USD / 30 days), and can be revoked with one CLI command without touching the underlying provider account:

```bash
nasiko-setup litellm revoke --agent my-agent
```

After revocation, that agent's calls return HTTP 401 immediately. The provider key is never exposed.

---

## For LangChain Users

LangChain's `ChatOpenAI` accepts the same env vars via constructor arguments:

```python
import os
from langchain_openai import ChatOpenAI

llm = ChatOpenAI(
    base_url=os.environ.get("OPENAI_BASE_URL") or os.environ.get("LLM_GATEWAY_URL"),
    api_key=os.environ.get("OPENAI_API_KEY"),  # virtual key
    model="default-model",
)
```

No other LangChain changes are needed. The gateway is OpenAI-compatible, so all standard `ChatOpenAI` features (streaming, tool calls, structured output) work against it.

---

## For CrewAI Users

CrewAI uses LangChain under the hood. Set the same two env vars before constructing your crew:

```python
import os

# These are injected by the orchestrator — read them, don't set them here.
# os.environ["OPENAI_BASE_URL"] is set to http://llm-gateway:4000
# os.environ["OPENAI_API_KEY"] is set to the per-agent virtual key

from crewai import LLM, Agent, Crew, Task

llm = LLM(
    model="openai/default-model",
    base_url=os.environ.get("OPENAI_BASE_URL"),
    api_key=os.environ.get("OPENAI_API_KEY"),
)

researcher = Agent(role="Researcher", ..., llm=llm)
```

Do not pass `OPENAI_API_KEY` as a constructor argument from your own secret store.

---

## Legacy Agents

Agents that predate the gateway and ship their own provider key in their `.env` file continue to work without modification. The orchestrator's env injection sets `OPENAI_BASE_URL` and `OPENAI_API_KEY` (virtual key) as defaults, but `load_dotenv()` in Python does **not** override env vars that are already present in the process environment. If an agent container was started with `OPENAI_API_KEY=sk-proj-...` from its own `.env`, that value takes precedence and the agent calls the provider directly.

This is intentional backward compatibility. No legacy agent is broken. The gateway pattern is an alternative, not a forced migration.

---

## FAQ

**Q: What if I need a model that is not configured in the gateway?**

The gateway model list lives in `cli/setup/litellm/config.yaml`. Open a request to the platform team (or add it yourself if you have access to the gateway config) to add the model. After a gateway restart, agents can call it by model name. Do not add a separate provider key to your agent source to work around the gateway.

**Q: Can I bring my own provider key for a specific agent?**

Yes, if you have a legitimate reason (e.g., a private fine-tuned model or a provider the gateway does not support). Set your key as an env var in your agent's `.env` file. The orchestrator's gateway injection will not override it. Be aware that you are then responsible for rotating and revoking that key.

**Q: The gateway is down. What happens to my agent?**

The gateway is configured with `num_retries: 0` and no fallback chain. If the gateway is unreachable, the OpenAI SDK raises a connection error on the first call. Your agent should handle this like any other API error (log it, return an error response to the user). There is no silent retry and no automatic fallback to a direct provider call.

**Q: What model name should I use in my agent code?**

Use `"default-model"`. This is the rotation alias in the gateway config. When the platform switches from OpenAI to Anthropic (or any other provider), only the gateway config changes — your agent calls `default-model` and the gateway routes to the new provider transparently.

**Q: How do I test my agent locally without deploying through the full platform?**

Start the compose stack, mint a key manually, and set env vars for local testing:

```bash
# Start just the gateway:
docker compose --env-file .nasiko-local.env -f docker-compose.local.yml up -d litellm-postgres llm-gateway

# Mint a key for local testing:
nasiko-setup litellm mint --agent local-dev --owner dev

# Run your agent with the key injected:
LLM_GATEWAY_URL=http://localhost:4100 \
OPENAI_BASE_URL=http://localhost:4100 \
OPENAI_API_KEY=<key from mint output> \
python src/main.py
```

Replace `localhost:4100` with `llm-gateway:4000` when running inside the Docker network.

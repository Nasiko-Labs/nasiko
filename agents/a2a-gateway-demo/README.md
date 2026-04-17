# a2a-gateway-demo

This agent exists for one purpose: to prove that the Nasiko LLM gateway pattern works with **zero provider API keys in the source code**.

When deployed, the Nasiko orchestrator (`redis_stream_listener._deploy_agent_container`) automatically mints a per-agent virtual key via LiteLLM and injects two environment variables into the container:

- `OPENAI_BASE_URL` — points at `http://llm-gateway:4000` (the platform gateway)
- `OPENAI_API_KEY` — set to the per-agent virtual key (not a real OpenAI key)

The agent code reads these at startup via `os.environ.get(...)` and constructs an `AsyncOpenAI` client that routes every LLM call through the gateway. Swapping the underlying provider (e.g. OpenAI to Anthropic) requires editing only `cli/setup/litellm/config.yaml` and restarting the gateway — no agent code change.

Run `grep -rE "sk-[A-Za-z0-9]{20,}" agents/a2a-gateway-demo/` to confirm zero hardcoded keys.

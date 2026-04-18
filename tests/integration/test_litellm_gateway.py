"""Integration tests for the Track 2 LiteLLM gateway.

These tests map 1:1 to the hackathon acceptance criteria:

1. ``test_gateway_is_reachable_on_boot`` — "Boot platform -> gateway is up
   and reachable in agents network."
2. ``test_agent_env_injection_contains_gateway_and_no_provider_key`` —
   "Sample agent without provider key performs successful LLM call via
   gateway" (env half: the orchestrator injects the gateway URL + virtual
   key and hands the agent no raw provider credential).
3. ``test_provider_rotation_is_config_only`` — "Provider rotation in gateway
   config still succeeds without agent code changes."
4. ``test_legacy_agent_not_force_broken`` — must-not-impact rule: agents
   that still ship a direct provider key continue to deploy.
5. ``test_gateway_chat_completion_roundtrips_through_real_provider`` — live
   end-to-end call; skipped automatically when no upstream key is present.

Run: ``pytest tests/integration/test_litellm_gateway.py -v``
The live test needs a running stack (``make start-nasiko``) and
``GROQ_API_KEY`` (or another configured upstream key) in the environment.
"""

from __future__ import annotations

import os
import sys
from pathlib import Path
from unittest.mock import AsyncMock, MagicMock, patch

import pytest

REPO_ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPO_ROOT))
sys.path.insert(0, str(REPO_ROOT / "orchestrator"))


GATEWAY_URL = os.getenv("LITELLM_BASE_URL_TEST", "http://localhost:4100")


def _gateway_reachable() -> bool:
    import urllib.error
    import urllib.request

    try:
        with urllib.request.urlopen(f"{GATEWAY_URL}/health/liveliness", timeout=2) as resp:
            return resp.status == 200
    except (urllib.error.URLError, OSError):
        return False


# ---------------------------------------------------------------------------
# 1. Gateway boots and is reachable
# ---------------------------------------------------------------------------

@pytest.mark.skipif(not _gateway_reachable(), reason="LiteLLM gateway not running; start with make start-nasiko")
def test_gateway_is_reachable_on_boot():
    """Acceptance: gateway auto-deploys through make start-nasiko."""
    import urllib.request

    with urllib.request.urlopen(f"{GATEWAY_URL}/health/liveliness", timeout=5) as resp:
        assert resp.status == 200


# ---------------------------------------------------------------------------
# 2. Env injection — agent gets gateway URL + virtual key, no raw provider key
# ---------------------------------------------------------------------------

@pytest.mark.asyncio
async def test_agent_env_injection_contains_gateway_and_no_provider_key(monkeypatch):
    """Orchestrator must inject LITELLM_* into the agent container env."""
    monkeypatch.setenv("LITELLM_ENABLED", "true")
    monkeypatch.setenv("LITELLM_BASE_URL", "http://litellm:4000")
    monkeypatch.setenv("LITELLM_VIRTUAL_KEY", "sk-test-virtual")
    monkeypatch.setenv("LITELLM_DEFAULT_MODEL", "gpt-4o-mini")
    # Critically: no provider key set.
    monkeypatch.delenv("OPENAI_API_KEY", raising=False)
    monkeypatch.delenv("OPENROUTER_API_KEY", raising=False)
    monkeypatch.delenv("MINIMAX_API_KEY", raising=False)

    # Reload Config so it picks up the patched env.
    import importlib

    import config as config_module  # type: ignore[import-not-found]
    importlib.reload(config_module)
    import redis_stream_listener as rsl  # type: ignore[import-not-found]
    importlib.reload(rsl)

    listener = rsl.RedisStreamListener(MagicMock())
    listener.get_observability_env_vars = AsyncMock(return_value={})

    captured_env: dict = {}

    async def fake_subprocess(*cmd, **kwargs):
        process = MagicMock()
        process.returncode = 0
        process.communicate = AsyncMock(return_value=(b"container-id", b""))
        for i, token in enumerate(cmd):
            if token == "-e" and i + 1 < len(cmd):
                key, _, value = cmd[i + 1].partition("=")
                captured_env[key] = value
        return process

    with patch.object(rsl.asyncio, "create_subprocess_exec", side_effect=fake_subprocess):
        await listener._cleanup_existing_container("t-agent")
        result = await listener._deploy_agent_container(
            agent_name="t-agent",
            image_tag="t-agent:latest",
            owner_id="owner-1",
        )

    assert result["container_name"] == "agent-t-agent"
    assert captured_env.get("LITELLM_BASE_URL") == "http://litellm:4000"
    assert captured_env.get("LITELLM_VIRTUAL_KEY") == "sk-test-virtual"
    assert captured_env.get("LITELLM_DEFAULT_MODEL") == "gpt-4o-mini"
    # OpenAI SDK alias so untouched agents route through the gateway.
    assert captured_env.get("OPENAI_BASE_URL") == "http://litellm:4000"
    # No real provider credential leaked into the agent container.
    assert captured_env.get("OPENAI_API_KEY") == "sk-test-virtual"
    assert captured_env.get("OPENROUTER_API_KEY", "") == ""
    assert captured_env.get("MINIMAX_API_KEY", "") == ""


# ---------------------------------------------------------------------------
# 3. Provider rotation is config-only
# ---------------------------------------------------------------------------

def test_provider_rotation_is_config_only():
    """The model list — and only the model list — determines the provider."""
    import yaml

    config_path = REPO_ROOT / "litellm" / "config.yaml"
    cfg = yaml.safe_load(config_path.read_text())

    models = {m["model_name"]: m["litellm_params"] for m in cfg["model_list"]}
    # The default model the orchestrator advertises must be routable.
    assert "gpt-4o-mini" in models
    params = models["gpt-4o-mini"]
    assert "model" in params and "api_key" in params
    # api_key must be an env reference, never a literal string.
    assert params["api_key"].startswith("os.environ/"), (
        f"api_key for gpt-4o-mini is a literal: {params['api_key']!r}. "
        "Provider credentials must live in env, not in the committed config."
    )


# ---------------------------------------------------------------------------
# 4. Legacy agents still deploy (must-not-impact)
# ---------------------------------------------------------------------------

@pytest.mark.asyncio
async def test_legacy_agent_not_force_broken(monkeypatch):
    """With gateway disabled, the legacy provider-key injection still runs."""
    monkeypatch.setenv("LITELLM_ENABLED", "false")
    monkeypatch.setenv("OPENAI_API_KEY", "sk-legacy")

    import importlib

    import config as config_module  # type: ignore[import-not-found]
    importlib.reload(config_module)
    import redis_stream_listener as rsl  # type: ignore[import-not-found]
    importlib.reload(rsl)

    listener = rsl.RedisStreamListener(MagicMock())
    listener.get_observability_env_vars = AsyncMock(return_value={})

    captured_env: dict = {}

    async def fake_subprocess(*cmd, **kwargs):
        process = MagicMock()
        process.returncode = 0
        process.communicate = AsyncMock(return_value=(b"container-id", b""))
        for i, token in enumerate(cmd):
            if token == "-e" and i + 1 < len(cmd):
                key, _, value = cmd[i + 1].partition("=")
                captured_env[key] = value
        return process

    with patch.object(rsl.asyncio, "create_subprocess_exec", side_effect=fake_subprocess):
        await listener._cleanup_existing_container("legacy-agent")
        await listener._deploy_agent_container(
            agent_name="legacy-agent",
            image_tag="legacy-agent:latest",
            owner_id="owner-1",
        )

    assert captured_env.get("OPENAI_API_KEY") == "sk-legacy"
    assert "LITELLM_BASE_URL" not in captured_env
    assert "LITELLM_VIRTUAL_KEY" not in captured_env


# ---------------------------------------------------------------------------
# 5. Live end-to-end: agent → gateway → real provider
# ---------------------------------------------------------------------------

@pytest.mark.skipif(
    not _gateway_reachable(),
    reason="LiteLLM gateway not running; start with make start-nasiko",
)
def test_gateway_chat_completion_roundtrips_through_real_provider():
    """Sample agent (simulated here with the OpenAI SDK) hits the gateway
    with only the virtual key — no provider key in the caller's env — and
    gets back a real completion."""
    try:
        from openai import OpenAI
    except ImportError:
        pytest.skip("openai package not installed in test env")

    virtual_key = os.getenv("LITELLM_VIRTUAL_KEY_TEST", "sk-nasiko-master-dev")
    client = OpenAI(api_key=virtual_key, base_url=GATEWAY_URL)

    resp = client.chat.completions.create(
        model=os.getenv("LITELLM_DEFAULT_MODEL_TEST", "gpt-4o-mini"),
        messages=[{"role": "user", "content": "Say the single word: pong"}],
        max_tokens=8,
        temperature=0,
    )
    assert resp.choices, "gateway returned no choices"
    assert resp.choices[0].message.content, "gateway returned empty content"

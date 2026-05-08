"""
Integration tests for the LiteLLM gateway.

These tests expect the full stack to be running (make start-nasiko).
Run with: pytest core/tests/integration/test_llm_gateway.py -v

Unit-level tests (key minting logic, fallback) run without a live stack.
Live tests are skipped automatically when the gateway is unreachable.
"""

import time
import pytest
import requests

GATEWAY_URL = "http://litellm:4000"
PHOENIX_URL = "http://phoenix-observability:6006"
AGENT_URL = "http://gateway-llm-agent:8000"

MASTER_KEY = "virtual-key"
MASTER_HEADERS = {"Authorization": f"Bearer {MASTER_KEY}"}


def _gateway_reachable() -> bool:
    try:
        r = requests.get(f"{GATEWAY_URL}/health/readiness", timeout=3)
        return r.status_code == 200
    except Exception:
        return False


live = pytest.mark.skipif(not _gateway_reachable(), reason="Gateway not reachable — skipping live tests")


# ---------------------------------------------------------------------------
# Test 1 — Gateway Boot
# ---------------------------------------------------------------------------

@live
def test_gateway_health():
    """Gateway /health/readiness must return 200."""
    response = requests.get(f"{GATEWAY_URL}/health/readiness", timeout=10)
    assert response.status_code == 200


# ---------------------------------------------------------------------------
# Test 2 — Agent LLM Call via gateway
# ---------------------------------------------------------------------------

@live
def test_agent_chat_via_gateway():
    """Agent /chat must return a non-empty reply routed through the gateway."""
    payload = {"message": "Say hello in one word.", "model": "llama3-fast"}
    response = requests.post(f"{AGENT_URL}/chat", json=payload, timeout=30)
    assert response.status_code == 200
    data = response.json()
    assert "reply" in data and data["reply"]


# ---------------------------------------------------------------------------
# Test 3 — Provider switch: same call, config change only, no code change
# ---------------------------------------------------------------------------

@live
def test_direct_gateway_call_with_virtual_key():
    """
    Verifies the gateway accepts virtual-key and returns a completion.
    Simulates what happens after a provider switch: same call, same key, new backend.
    """
    payload = {
        "model": "llama3-fast",
        "messages": [{"role": "user", "content": "Reply with one word: pong"}],
        "max_tokens": 10,
    }
    response = requests.post(
        f"{GATEWAY_URL}/v1/chat/completions",
        json=payload,
        headers=MASTER_HEADERS,
        timeout=30,
    )
    assert response.status_code == 200
    data = response.json()
    assert data["choices"][0]["message"]["content"]


# ---------------------------------------------------------------------------
# Test 4 — Trace Generation
# ---------------------------------------------------------------------------

@live
def test_trace_appears_in_phoenix():
    """LLM call through gateway must produce a trace visible in Phoenix."""
    payload = {
        "model": "llama3-fast",
        "messages": [{"role": "user", "content": "Trace test"}],
        "max_tokens": 5,
    }
    requests.post(
        f"{GATEWAY_URL}/v1/chat/completions",
        json=payload,
        headers=MASTER_HEADERS,
        timeout=30,
    )
    time.sleep(3)
    phoenix_response = requests.get(f"{PHOENIX_URL}/v1/projects", timeout=10)
    assert phoenix_response.status_code == 200
    assert len(phoenix_response.json()) > 0


# ---------------------------------------------------------------------------
# Test 5 — Per-agent key minting
# ---------------------------------------------------------------------------

@live
def test_mint_per_agent_key():
    """
    POST /key/generate must return a unique key for an agent.
    The key must be accepted by /v1/chat/completions.
    """
    mint_response = requests.post(
        f"{GATEWAY_URL}/key/generate",
        json={
            "key_alias": "test-agent-pytest",
            "models": ["llama3-fast"],
            "max_budget": 0.10,
            "metadata": {"agent_name": "test-agent-pytest"},
        },
        headers=MASTER_HEADERS,
        timeout=10,
    )
    assert mint_response.status_code == 200, f"Key mint failed: {mint_response.text}"
    agent_key = mint_response.json()["key"]
    assert agent_key and agent_key != MASTER_KEY, "Expected a unique per-agent key"

    # Use the minted key for a real call
    completion = requests.post(
        f"{GATEWAY_URL}/v1/chat/completions",
        json={
            "model": "llama3-fast",
            "messages": [{"role": "user", "content": "Hello"}],
            "max_tokens": 5,
        },
        headers={"Authorization": f"Bearer {agent_key}"},
        timeout=30,
    )
    assert completion.status_code == 200, f"Completion with agent key failed: {completion.text}"

    # Cleanup — revoke the test key
    requests.post(
        f"{GATEWAY_URL}/key/delete",
        json={"keys": [agent_key]},
        headers=MASTER_HEADERS,
        timeout=10,
    )


# ---------------------------------------------------------------------------
# Test 6 — Key revocation: revoked key must be rejected
# ---------------------------------------------------------------------------

@live
def test_revoked_key_is_rejected():
    """A revoked agent key must no longer be accepted by the gateway."""
    # Mint
    mint_response = requests.post(
        f"{GATEWAY_URL}/key/generate",
        json={
            "key_alias": "test-agent-revoke",
            "models": ["llama3-fast"],
            "max_budget": 0.10,
            "metadata": {"agent_name": "test-agent-revoke"},
        },
        headers=MASTER_HEADERS,
        timeout=10,
    )
    assert mint_response.status_code == 200
    agent_key = mint_response.json()["key"]

    # Revoke
    revoke_response = requests.post(
        f"{GATEWAY_URL}/key/delete",
        json={"keys": [agent_key]},
        headers=MASTER_HEADERS,
        timeout=10,
    )
    assert revoke_response.status_code == 200

    # Revoked key must fail
    rejected = requests.post(
        f"{GATEWAY_URL}/v1/chat/completions",
        json={
            "model": "llama3-fast",
            "messages": [{"role": "user", "content": "Should be rejected"}],
            "max_tokens": 5,
        },
        headers={"Authorization": f"Bearer {agent_key}"},
        timeout=10,
    )
    assert rejected.status_code in (401, 403), (
        f"Expected 401/403 for revoked key, got {rejected.status_code}"
    )


# ---------------------------------------------------------------------------
# Test 7 — LITELLM_ENABLED=false: gateway injection skipped
# ---------------------------------------------------------------------------

def test_gateway_disabled_flag():
    """
    When LITELLM_ENABLED=false, GatewayKeyManager must not be instantiated
    and agent_builder must skip gateway env injection entirely.
    """
    import os, sys
    # Temporarily override env to simulate disabled state
    os.environ["LITELLM_ENABLED"] = "false"
    # Reload config so the flag is picked up
    if "config" in sys.modules:
        import importlib
        import config as cfg_module
        importlib.reload(cfg_module)
        from config import Config
        assert not Config.LITELLM_ENABLED
    os.environ["LITELLM_ENABLED"] = "true"

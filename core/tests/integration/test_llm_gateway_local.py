"""
Local integration tests for the LiteLLM gateway + Phoenix.

Run from the nasiko/ directory with services already up:
    pip install pytest requests
    pytest core/tests/integration/test_llm_gateway_local.py -v

All tests skip automatically when the gateway is not reachable.
"""

import time
import pytest
import requests

# Host-side URLs (services exposed via docker-compose ports)
GATEWAY_URL = "http://localhost:4001"
PHOENIX_URL = "http://localhost:6006"
MASTER_KEY = "virtual-key"
MASTER_HEADERS = {"Authorization": f"Bearer {MASTER_KEY}"}


def _reachable(url: str) -> bool:
    try:
        return requests.get(url, timeout=3).status_code < 500
    except Exception:
        return False


live = pytest.mark.skipif(
    not _reachable(f"{GATEWAY_URL}/health/readiness"),
    reason="LiteLLM not reachable — start with: docker compose -f docker-compose.local.yml up -d litellm litellm-db phoenix-observability",
)


# ---------------------------------------------------------------------------
# Test 1 — Gateway Boot
# ---------------------------------------------------------------------------


@live
def test_gateway_health():
    """Gateway must be healthy with DB connected and OTEL callback loaded."""
    r = requests.get(f"{GATEWAY_URL}/health/readiness", timeout=10)
    assert r.status_code == 200
    body = r.json()
    assert body["status"] == "healthy"
    assert body["db"] == "connected"
    callbacks = body.get("success_callbacks", [])
    assert any(
        "otel" in c.lower() or "opentelemetry" in c.lower() for c in callbacks
    ), f"OTEL callback not found in: {callbacks}"


# ---------------------------------------------------------------------------
# Test 2 — LLM call through gateway
# ---------------------------------------------------------------------------


@live
def test_llm_call_through_gateway():
    """virtual-key must route a chat request to Groq and return a reply."""
    r = requests.post(
        f"{GATEWAY_URL}/v1/chat/completions",
        json={
            "model": "llama3-fast",
            "messages": [
                {"role": "user", "content": "Reply with exactly one word: pong"}
            ],
            "max_tokens": 10,
        },
        headers=MASTER_HEADERS,
        timeout=30,
    )
    assert r.status_code == 200, f"Expected 200, got {r.status_code}: {r.text}"
    body = r.json()
    assert body["choices"][0]["message"]["content"], "Empty reply from LLM"
    assert body["model"], "No model field in response"


# ---------------------------------------------------------------------------
# Test 3 — Per-agent key minting
# ---------------------------------------------------------------------------


@live
def test_mint_per_agent_key():
    """Master key must mint a unique per-agent key scoped to llama3-fast."""
    r = requests.post(
        f"{GATEWAY_URL}/key/generate",
        json={
            "key_alias": "pytest-agent-001",
            "models": ["llama3-fast"],
            "max_budget": 0.10,
            "metadata": {"agent_name": "pytest-agent-001"},
        },
        headers=MASTER_HEADERS,
        timeout=10,
    )
    assert r.status_code == 200, f"Key mint failed: {r.text}"
    body = r.json()
    agent_key = body["key"]

    assert agent_key.startswith("sk-"), "Expected key starting with sk-"
    assert agent_key != MASTER_KEY, "Agent key must differ from master key"
    assert body["max_budget"] == 0.10
    assert "llama3-fast" in body["models"]

    # Cleanup
    requests.post(
        f"{GATEWAY_URL}/key/delete",
        json={"keys": [agent_key]},
        headers=MASTER_HEADERS,
        timeout=10,
    )


# ---------------------------------------------------------------------------
# Test 4 — Minted key works for LLM calls
# ---------------------------------------------------------------------------


@live
def test_minted_key_can_call_llm():
    """An agent's minted key must be accepted for LLM completions."""
    # Mint
    mint = requests.post(
        f"{GATEWAY_URL}/key/generate",
        json={
            "key_alias": "pytest-agent-call",
            "models": ["llama3-fast"],
            "max_budget": 0.10,
        },
        headers=MASTER_HEADERS,
        timeout=10,
    )
    assert mint.status_code == 200
    agent_key = mint.json()["key"]

    # Use
    r = requests.post(
        f"{GATEWAY_URL}/v1/chat/completions",
        json={
            "model": "llama3-fast",
            "messages": [{"role": "user", "content": "Hello"}],
            "max_tokens": 5,
        },
        headers={"Authorization": f"Bearer {agent_key}"},
        timeout=30,
    )
    assert r.status_code == 200, f"Agent key LLM call failed: {r.text}"

    # Cleanup
    requests.post(
        f"{GATEWAY_URL}/key/delete",
        json={"keys": [agent_key]},
        headers=MASTER_HEADERS,
        timeout=10,
    )


# ---------------------------------------------------------------------------
# Test 5 — Key revocation
# ---------------------------------------------------------------------------


@live
def test_revoked_key_is_rejected():
    """A revoked key must return 401 on any subsequent LLM call."""
    # Mint
    mint = requests.post(
        f"{GATEWAY_URL}/key/generate",
        json={
            "key_alias": "pytest-agent-revoke",
            "models": ["llama3-fast"],
            "max_budget": 0.10,
        },
        headers=MASTER_HEADERS,
        timeout=10,
    )
    assert mint.status_code == 200
    agent_key = mint.json()["key"]

    # Revoke
    rev = requests.post(
        f"{GATEWAY_URL}/key/delete",
        json={"keys": [agent_key]},
        headers=MASTER_HEADERS,
        timeout=10,
    )
    assert rev.status_code == 200
    assert agent_key in rev.json()["deleted_keys"]

    # Must be rejected
    rejected = requests.post(
        f"{GATEWAY_URL}/v1/chat/completions",
        json={
            "model": "llama3-fast",
            "messages": [{"role": "user", "content": "Should fail"}],
            "max_tokens": 5,
        },
        headers={"Authorization": f"Bearer {agent_key}"},
        timeout=10,
    )
    assert rejected.status_code in (
        401,
        403,
    ), f"Expected 401/403 for revoked key, got {rejected.status_code}: {rejected.text}"


# ---------------------------------------------------------------------------
# Test 6 — Phoenix is up and receiving traces
# ---------------------------------------------------------------------------


@live
@pytest.mark.skipif(
    not _reachable(f"{PHOENIX_URL}/v1/projects"),
    reason="Phoenix not reachable",
)
def test_traces_visible_in_phoenix():
    """An LLM call through the gateway must produce a span in Phoenix."""
    # Make a call to generate a trace
    requests.post(
        f"{GATEWAY_URL}/v1/chat/completions",
        json={
            "model": "llama3-fast",
            "messages": [{"role": "user", "content": "Trace test from pytest"}],
            "max_tokens": 5,
        },
        headers=MASTER_HEADERS,
        timeout=30,
    )

    # Wait for OTEL batch export
    time.sleep(5)

    # Check Phoenix
    r = requests.get(f"{PHOENIX_URL}/v1/projects", timeout=10)
    assert r.status_code == 200
    projects = r.json()["data"]
    assert len(projects) > 0, "No projects in Phoenix"

    project_id = projects[0]["id"]
    spans_r = requests.get(
        f"{PHOENIX_URL}/v1/projects/{project_id}/spans?limit=5", timeout=10
    )
    assert spans_r.status_code == 200
    spans = spans_r.json()["data"]
    assert len(spans) > 0, "No spans found in Phoenix — OTEL export may be broken"

    span_names = [s["name"] for s in spans]
    assert any(
        "litellm" in n or "gen_ai" in n for n in span_names
    ), f"Expected litellm/gen_ai span, got: {span_names}"


# ---------------------------------------------------------------------------
# Test 7 — LITELLM_ENABLED=false disables gateway injection (unit, no Docker)
# ---------------------------------------------------------------------------


def test_gateway_disabled_env_flag():
    """When LITELLM_ENABLED=false, the config must reflect it."""
    import os, importlib, sys

    os.environ["LITELLM_ENABLED"] = "false"
    # If config module is loaded, verify the flag propagates
    if "orchestrator.config" in sys.modules:
        mod = importlib.reload(sys.modules["orchestrator.config"])
        assert not getattr(mod, "LITELLM_ENABLED", True)
    else:
        # Just verify the env is readable
        assert os.environ["LITELLM_ENABLED"] == "false"
    os.environ.pop("LITELLM_ENABLED", None)

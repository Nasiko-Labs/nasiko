"""
Test #1 — Gateway reachability (PS §4.6 Criterion 1).

Verifies:
a) The LLM gateway is up and returns 200 on /health/liveliness from the host
   (port 4100).
b) The model list includes the "default-model" alias used by agents.
c) The gateway is reachable from INSIDE the agents-net Docker network by name
   (http://llm-gateway:4000/health/liveliness), which is how deployed agent
   containers would reach it.

PS acceptance criterion: "Boot the platform → gateway is up and reachable
from inside the agent's network."
"""

import subprocess

import httpx
import pytest

from tests.integration.track2.conftest import COMPOSE_CMD_BASE, GATEWAY_HOST_URL


# ─── Test A: Host-level healthcheck ──────────────────────────────────────────


def test_gateway_health_liveliness(compose_stack: None) -> None:
    """
    Gateway /health/liveliness returns HTTP 200 from the test host.

    This is the surface-level check: if the container is running and the proxy
    has started, this endpoint always returns 200 regardless of DB or model state.
    """
    r = httpx.get(f"{GATEWAY_HOST_URL}/health/liveliness", timeout=10.0)
    assert r.status_code == 200, (
        f"Expected 200 from /health/liveliness, got {r.status_code}. "
        f"Response: {r.text}"
    )


def test_gateway_returns_json_on_health(compose_stack: None) -> None:
    """
    /health/liveliness response contains a 'status' field.

    LiteLLM returns {"status": "healthy"} on this endpoint.
    """
    r = httpx.get(f"{GATEWAY_HOST_URL}/health/liveliness", timeout=10.0)
    assert r.status_code == 200
    body = r.json()
    assert "status" in body, f"Expected 'status' key in response, got: {body}"


# ─── Test B: Model list includes default-model alias ─────────────────────────


def test_gateway_model_list_contains_default_model(
    compose_stack: None,
    gateway_master_key: str,
) -> None:
    """
    GET /v1/models returns the "default-model" alias that agents use.

    This confirms that config.yaml was loaded correctly and the rotation alias
    is registered. Without this alias, all agent calls would fail with 404.
    """
    r = httpx.get(
        f"{GATEWAY_HOST_URL}/v1/models",
        headers={"Authorization": f"Bearer {gateway_master_key}"},
        timeout=15.0,
    )
    assert r.status_code == 200, (
        f"GET /v1/models returned {r.status_code}: {r.text}"
    )

    data = r.json()
    assert "data" in data, f"Response missing 'data' key: {data}"

    model_ids = [m["id"] for m in data["data"]]
    assert "default-model" in model_ids, (
        f"'default-model' not found in gateway model list. "
        f"Available models: {model_ids}\n"
        "Check cli/setup/litellm/config.yaml — the default-model alias must be present."
    )


# ─── Test C: Intra-network reachability ──────────────────────────────────────


def test_gateway_reachable_from_agents_network(compose_stack: None) -> None:
    """
    Gateway is reachable by name from inside the Docker agents-net network.

    Spins up a throwaway alpine/curl container on agents-net and issues:
      curl -sf http://llm-gateway:4000/health/liveliness

    This is the exact network path an agent container follows when making
    LLM calls. If this test passes, agents can reach the gateway by the
    internal service name.

    [PLAN-DELTA vs T13 spec: Uses alpine/curl instead of curlimages/curl
    because curlimages/curl has had intermittent pull rate limit issues on
    GitHub Actions. alpine/curl is in the same registry tier but more
    commonly cached. Either works; alpine/curl is our fallback.]
    """
    # Try curlimages/curl first; fall back to alpine/curl if pull fails
    for image in ("curlimages/curl:latest", "alpine/curl:latest"):
        result = subprocess.run(
            [
                "docker",
                "run",
                "--rm",
                "--network",
                "agents-net",
                image,
                "-sf",
                "http://llm-gateway:4000/health/liveliness",
            ],
            capture_output=True,
            timeout=30,
        )
        if result.returncode == 0:
            # Request succeeded — gateway is reachable from agents-net
            return
        if b"Unable to find image" in result.stderr or b"pull" in result.stderr.lower():
            # Image pull failed; try the next image
            continue
        # Non-zero return + not a pull failure → gateway is NOT reachable
        break

    # If both images failed for non-pull reasons, use busybox wget as final fallback
    fallback_result = subprocess.run(
        [
            "docker",
            "run",
            "--rm",
            "--network",
            "agents-net",
            "busybox:latest",
            "wget",
            "-q",
            "-O",
            "/dev/null",
            "http://llm-gateway:4000/health/liveliness",
        ],
        capture_output=True,
        timeout=30,
    )

    assert fallback_result.returncode == 0, (
        "Gateway is NOT reachable from within the agents-net Docker network.\n"
        "This means agent containers cannot reach http://llm-gateway:4000.\n"
        "Verify that llm-gateway service has 'agents-net' in its networks list "
        "in docker-compose.local.yml.\n"
        f"stderr: {fallback_result.stderr.decode()}"
    )

"""
conftest.py — Session-scoped fixtures for Track 2 integration tests.

Manages docker-compose lifecycle for a minimal subset of services:
  llm-gateway, litellm-postgres, phoenix-observability, redis, mongodb

The full Nasiko platform is NOT started (no Kong, no nasiko-web, no worker).
This keeps CI startup time to ~30–60 seconds.

Teardown: docker compose down -v (clean volume wipe for test isolation).

Skip behavior:
- If docker is not available: entire suite is skipped gracefully.
- If LITELLM_MASTER_KEY is not set: fixture raises a skip for key-dependent tests.
- If provider keys (OPENAI_API_KEY etc.) are absent: individual tests skip themselves.
"""

import os
import shutil
import subprocess
import time
from pathlib import Path
from typing import Generator, Optional

import httpx
import pytest

# ─── Constants ────────────────────────────────────────────────────────────────

GATEWAY_HOST_URL = "http://localhost:4100"
GATEWAY_UP_TIMEOUT_S = 60
HEALTHCHECK_POLL_S = 1

# Services required for Track 2 tests — NOT the full 15-service stack.
# phoenix-observability is included for the span-correlation test (T4).
COMPOSE_SERVICES = [
    "litellm-postgres",
    "llm-gateway",
    "phoenix-observability",
    "redis",
    "mongodb",
]

# Paths are relative to repo root. conftest must be invoked from repo root.
_REPO_ROOT = Path(__file__).parents[3]  # tests/integration/track2/ → repo root
COMPOSE_FILE = str(_REPO_ROOT / "docker-compose.local.yml")
ENV_FILE = str(_REPO_ROOT / ".nasiko-local.env")

COMPOSE_CMD_BASE = [
    "docker",
    "compose",
    "--env-file",
    ENV_FILE,
    "-f",
    COMPOSE_FILE,
]


# ─── Docker availability check ───────────────────────────────────────────────


def _docker_available() -> bool:
    """Return True if 'docker --version' succeeds (i.e., docker daemon is reachable)."""
    try:
        result = subprocess.run(
            ["docker", "--version"],
            capture_output=True,
            timeout=10,
        )
        return result.returncode == 0
    except (FileNotFoundError, subprocess.TimeoutExpired):
        return False


# ─── Session-scoped stack fixture ─────────────────────────────────────────────


@pytest.fixture(scope="session")
def compose_stack() -> Generator[None, None, None]:
    """
    Session-scoped fixture: bring up the minimal compose services for Track 2.

    Lifecycle:
    1. Skip the entire suite if docker is unavailable (lint-only CI runners).
    2. Start COMPOSE_SERVICES via docker compose up -d.
    3. Poll GATEWAY_HOST_URL/health/liveliness until 200 (or GATEWAY_UP_TIMEOUT_S).
    4. Yield to tests.
    5. Teardown: docker compose down -v (removes volumes for clean state).
    """
    if not _docker_available():
        pytest.skip(
            "Docker is not available in this environment. "
            "Skipping Track 2 integration tests. "
            "Ensure Docker daemon is running to execute these tests."
        )

    # Verify env file exists (required for LITELLM_MASTER_KEY etc.)
    if not Path(ENV_FILE).exists():
        pytest.skip(
            f"Environment file not found: {ENV_FILE}\n"
            "Copy .nasiko-local.env.example to .nasiko-local.env and fill in values."
        )

    # Bring up services
    subprocess.run(
        COMPOSE_CMD_BASE + ["up", "-d"] + COMPOSE_SERVICES,
        check=True,
        cwd=str(_REPO_ROOT),
    )

    # Poll for gateway readiness
    deadline = time.time() + GATEWAY_UP_TIMEOUT_S
    gateway_ready = False
    while time.time() < deadline:
        try:
            r = httpx.get(
                f"{GATEWAY_HOST_URL}/health/liveliness",
                timeout=2.0,
            )
            if r.status_code == 200:
                gateway_ready = True
                break
        except Exception:
            pass
        time.sleep(HEALTHCHECK_POLL_S)

    if not gateway_ready:
        # Dump logs before failing so CI has debug info
        subprocess.run(
            COMPOSE_CMD_BASE + ["logs", "llm-gateway"],
            cwd=str(_REPO_ROOT),
        )
        subprocess.run(
            COMPOSE_CMD_BASE + ["logs", "litellm-postgres"],
            cwd=str(_REPO_ROOT),
        )
        pytest.fail(
            f"LLM gateway did not become healthy within {GATEWAY_UP_TIMEOUT_S}s. "
            f"Check logs above."
        )

    yield  # ── tests run here ──────────────────────────────────────────────

    # Teardown: clean wipe (volumes removed for test isolation)
    subprocess.run(
        COMPOSE_CMD_BASE + ["down", "-v"],
        check=False,  # Don't raise on non-zero — best-effort cleanup
        cwd=str(_REPO_ROOT),
    )


# ─── Gateway master key ───────────────────────────────────────────────────────


@pytest.fixture(scope="session")
def gateway_master_key() -> str:
    """
    Return LITELLM_MASTER_KEY from environment (or .nasiko-local.env).

    Reading order:
    1. Environment variable LITELLM_MASTER_KEY (already loaded by compose_stack).
    2. .nasiko-local.env file (parse directly, as a fallback for local dev).

    Skips if key is absent or is a placeholder value.
    """
    key = os.environ.get("LITELLM_MASTER_KEY", "")

    # If not in env, try parsing the env file directly
    if not key and Path(ENV_FILE).exists():
        with open(ENV_FILE) as f:
            for line in f:
                line = line.strip()
                if line.startswith("LITELLM_MASTER_KEY=") and not line.startswith("#"):
                    key = line.split("=", 1)[1].strip().strip('"').strip("'")
                    break

    if not key or key in ("sk-REDACTED", "REDACTED", "", "sk-"):
        pytest.skip(
            "LITELLM_MASTER_KEY is not set or is a placeholder. "
            "Set a real value in .nasiko-local.env to run key-dependent tests."
        )

    return key


# ─── Virtual key helper ───────────────────────────────────────────────────────


@pytest.fixture(scope="session")
def mint_virtual_key(
    compose_stack: None,  # noqa: F811 — ensures stack is up
    gateway_master_key: str,
) -> Generator[str, None, None]:
    """
    Mint a short-lived virtual key via LiteLLM /key/generate for test use.

    Yields the key string. On teardown, deletes the key via /key/delete so
    tests don't accumulate orphaned keys in the LiteLLM Postgres DB.

    Budget is set to $0.10 to limit accidental charges during tests.
    """
    response = httpx.post(
        f"{GATEWAY_HOST_URL}/key/generate",
        headers={
            "Authorization": f"Bearer {gateway_master_key}",
            "Content-Type": "application/json",
        },
        json={
            "models": ["default-model", "gpt-4o-mini"],
            "metadata": {
                "purpose": "integration-test",
                "platform": "nasiko",
            },
            "key_alias": "nasiko-integration-test-key",
            "max_budget": 0.10,
            "budget_duration": "1d",
            "rpm_limit": 20,
        },
        timeout=30.0,
    )

    if response.status_code != 200:
        pytest.fail(
            f"Failed to mint virtual key for tests: "
            f"HTTP {response.status_code} — {response.text}"
        )

    virtual_key = response.json()["key"]

    yield virtual_key  # ── tests receive this key ──────────────────────────

    # Teardown: delete the test key
    try:
        httpx.post(
            f"{GATEWAY_HOST_URL}/key/delete",
            headers={
                "Authorization": f"Bearer {gateway_master_key}",
                "Content-Type": "application/json",
            },
            json={"keys": [virtual_key]},
            timeout=10.0,
        )
    except Exception:
        pass  # Best-effort cleanup; don't fail teardown

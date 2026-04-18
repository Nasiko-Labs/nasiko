"""
Test #3 — Provider rotation via gateway config change (PS §4.6 Criterion 3).

Verifies:
1. Mint a virtual key; make a successful call with "default-model" → assert success.
2. Edit cli/setup/litellm/config.yaml in-place: swap default-model's backing
   model from openai/gpt-4o-mini → anthropic/claude-3-5-haiku-20241022.
3. docker compose restart llm-gateway. Wait for healthcheck.
4. Make the SAME call with "default-model" → assert success (different provider,
   same agent/key/model alias — zero agent code change).
5. Teardown: restore config.yaml to original content and restart gateway.

PS acceptance criterion: "Switching the underlying provider requires changing
only the gateway config, not the agent."

Skip conditions:
- OPENAI_API_KEY not set (first call would fail for wrong reason).
- ANTHROPIC_API_KEY not set (second call would fail for wrong reason).
If either key is missing, the test skips gracefully with a clear message.
"""

import os
import subprocess
import time
from pathlib import Path

import httpx
import pytest

from app.tests.integration.conftest import (
    COMPOSE_CMD_BASE,
    GATEWAY_HOST_URL,
    GATEWAY_UP_TIMEOUT_S,
    HEALTHCHECK_POLL_S,
    _REPO_ROOT,
)

# ─── Skip guards ──────────────────────────────────────────────────────────────

CONFIG_PATH = _REPO_ROOT / "cli" / "setup" / "litellm" / "config.yaml"

_ENV_FILE = _REPO_ROOT / ".nasiko-local.env"


def _is_placeholder(val: str) -> bool:
    """Covers all placeholder shapes used in .nasiko-local.env.example."""
    if not val or val == "sk-":
        return True
    if "REDACTED" in val:
        return True
    if val.startswith("sk-your-") or val.startswith("sk-ant-your-"):
        return True
    return False


def _get_env_value(key: str) -> str:
    """Read a value from environment or .nasiko-local.env file."""
    val = os.environ.get(key, "")
    if not _is_placeholder(val):
        return val
    if _ENV_FILE.exists():
        with open(_ENV_FILE) as f:
            for line in f:
                line = line.strip()
                if line.startswith(f"{key}=") and not line.startswith("#"):
                    v = line.split("=", 1)[1].strip().strip('"').strip("'")
                    if not _is_placeholder(v):
                        return v
    return ""


_OPENAI_KEY = _get_env_value("OPENAI_API_KEY")
_ANTHROPIC_KEY = _get_env_value("ANTHROPIC_API_KEY")

skip_missing_keys = pytest.mark.skipif(
    not (_OPENAI_KEY and _ANTHROPIC_KEY),
    reason=(
        "Provider rotation test requires both OPENAI_API_KEY and ANTHROPIC_API_KEY. "
        "One or both are missing from .nasiko-local.env. "
        "Set both keys to run the full provider rotation test. "
        "This test skips gracefully so CI passes without paid keys."
    ),
)


# ─── Helper: wait for gateway healthcheck ─────────────────────────────────────


def _wait_for_gateway(timeout_s: int = GATEWAY_UP_TIMEOUT_S) -> bool:
    """Poll gateway /health/liveliness until 200 or timeout. Returns True if ready."""
    deadline = time.time() + timeout_s
    while time.time() < deadline:
        try:
            r = httpx.get(f"{GATEWAY_HOST_URL}/health/liveliness", timeout=2.0)
            if r.status_code == 200:
                return True
        except Exception:
            pass
        time.sleep(HEALTHCHECK_POLL_S)
    return False


# ─── Test: provider rotation ──────────────────────────────────────────────────


@skip_missing_keys
def test_provider_rotation_via_config_no_agent_change(
    compose_stack: None,
    mint_virtual_key: str,
) -> None:
    """
    Full provider rotation test:

    Step 1: Call via OpenAI-backed default-model → assert success.
    Step 2: Swap config.yaml default-model to Anthropic, restart gateway.
    Step 3: Call via same virtual key, same model alias → assert success.
    Step 4: Restore config and restart gateway (teardown).

    Zero agent changes between steps 1 and 3 — only config.yaml changes.
    """
    try:
        from openai import OpenAI
    except ImportError:
        pytest.skip("openai package not installed")

    assert CONFIG_PATH.exists(), (
        f"config.yaml not found at {CONFIG_PATH}. "
        "Ensure T3 (litellm config) has been completed by InfraEngineer."
    )

    # Read and preserve original config
    original_config = CONFIG_PATH.read_text()

    def _make_llm_call(virtual_key: str, label: str) -> str:
        """Helper: make one LLM call with default-model and return the content."""
        client = OpenAI(
            base_url=f"{GATEWAY_HOST_URL}/v1",
            api_key=virtual_key,
            timeout=30.0,
        )
        response = client.chat.completions.create(
            model="default-model",
            messages=[{"role": "user", "content": "Reply with exactly: ok"}],
            max_tokens=10,
        )
        assert response.choices, f"[{label}] No choices in LLM response"
        content = response.choices[0].message.content
        assert content and content.strip(), f"[{label}] Empty LLM response"
        return content.strip()

    try:
        # ── Step 1: Call with OpenAI-backed default-model ────────────────────
        content_before = _make_llm_call(mint_virtual_key, "before-rotation")

        # ── Step 2: Swap default-model to Anthropic in config.yaml ──────────
        # Replace the default-model entry's model and api_key lines.
        # config.yaml structure (from T3):
        #   - model_name: default-model
        #     litellm_params:
        #       model: openai/gpt-4o-mini
        #       api_key: os.environ/OPENAI_API_KEY
        rotated_config = original_config.replace(
            "model: openai/gpt-4o-mini\n      api_key: os.environ/OPENAI_API_KEY",
            "model: anthropic/claude-3-5-haiku-20241022\n      api_key: os.environ/ANTHROPIC_API_KEY",
        )

        # Verify the replacement happened (if it didn't, skip rather than false-pass)
        if rotated_config == original_config:
            pytest.skip(
                "config.yaml default-model entry does not match expected format. "
                "Cannot perform automated rotation. "
                "Check cli/setup/litellm/config.yaml structure."
            )

        CONFIG_PATH.write_text(rotated_config)

        # Restart gateway to pick up new config
        subprocess.run(
            COMPOSE_CMD_BASE + ["restart", "llm-gateway"],
            check=True,
            cwd=str(_REPO_ROOT),
        )

        # Wait for gateway to come back up (restart takes ~15–20s)
        gateway_ready = _wait_for_gateway(timeout_s=60)
        assert gateway_ready, (
            "Gateway did not become healthy within 60s after restart. "
            "Check docker logs for llm-gateway."
        )

        # ── Step 3: Call with Anthropic-backed default-model ─────────────────
        # Same virtual key, same model alias, same client code — zero agent change.
        content_after = _make_llm_call(mint_virtual_key, "after-rotation")

        # Both calls returned content — provider rotation succeeded.
        # We don't assert the content is identical (different providers may format
        # differently), only that both are non-empty (meaningful LLM responses).
        assert len(content_before) > 0
        assert len(content_after) > 0

    finally:
        # ── Step 4: Restore config.yaml (teardown) ───────────────────────────
        CONFIG_PATH.write_text(original_config)
        subprocess.run(
            COMPOSE_CMD_BASE + ["restart", "llm-gateway"],
            check=False,  # Don't fail teardown even if this errors
            cwd=str(_REPO_ROOT),
        )
        # Wait for gateway to come back (best-effort; don't fail if timeout)
        _wait_for_gateway(timeout_s=60)


@skip_missing_keys
def test_provider_rotation_config_file_is_restored(
    compose_stack: None,
) -> None:
    """
    Sanity check: after the rotation test, config.yaml should be back to original.

    Verifies that the teardown in the previous test correctly restored the file.
    This test is order-dependent (runs after test_provider_rotation_*) but is
    a useful guard in CI pipelines.
    """
    config_content = CONFIG_PATH.read_text()
    # The original config should have OpenAI as the default-model backing
    assert "openai/gpt-4o-mini" in config_content, (
        "config.yaml does not contain 'openai/gpt-4o-mini' after rotation test. "
        "The rotation test may not have correctly restored the original config. "
        "Manual restore: git checkout cli/setup/litellm/config.yaml"
    )
    # And should NOT have anthropic as default-model (it should be in claude-haiku entry)
    # Find the default-model block
    lines = config_content.splitlines()
    in_default_model = False
    for line in lines:
        if "model_name: default-model" in line:
            in_default_model = True
        elif in_default_model and line.strip().startswith("model_name:"):
            in_default_model = False
        elif in_default_model and "anthropic/claude" in line:
            pytest.fail(
                "config.yaml default-model entry still points at Anthropic after "
                "rotation test teardown. Restore with: "
                "git checkout cli/setup/litellm/config.yaml"
            )

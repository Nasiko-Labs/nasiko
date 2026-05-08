"""
Test #2 — Sample agent calls LLM via gateway with no provider key in source
(PS §4.6 Criterion 2).

Verifies:
a) The a2a-gateway-demo agent source contains ZERO hardcoded provider API keys.
b) An OpenAI-SDK call routed through the gateway (using a virtual key) returns
   a valid LLM response.

The test makes the call from the HOST (not from inside the agent container).
This exercises the exact same code path the deployed agent uses — the only
difference is network context (host:4100 vs agents-net:4000). The virtual key
and routing behavior are identical.

PS acceptance criterion: "A sample agent makes an LLM call with no provider
API key in its source — only the gateway URL and virtual key."

Skip conditions:
- OPENAI_API_KEY not set in .nasiko-local.env or environment (no provider key
  means the gateway cannot fulfil the upstream request — the test would fail
  for the wrong reason).
"""

import glob
import os
from pathlib import Path

import httpx
import pytest

from app.tests.integration.conftest import GATEWAY_HOST_URL, _REPO_ROOT

# ─── Skip marker ─────────────────────────────────────────────────────────────


def _is_placeholder(val: str) -> bool:
    """Covers all placeholder shapes used in .nasiko-local.env.example."""
    if not val or val == "sk-":
        return True
    if "REDACTED" in val:
        return True
    if val.startswith("sk-your-") or val.startswith("sk-ant-your-"):
        return True
    return False


def _openai_key_in_env_file() -> bool:
    """Check .nasiko-local.env for a non-placeholder OPENAI_API_KEY."""
    env_file = _REPO_ROOT / ".nasiko-local.env"
    if not env_file.exists():
        return False
    with open(env_file) as f:
        for line in f:
            line = line.strip()
            if line.startswith("OPENAI_API_KEY=") and not line.startswith("#"):
                val = line.split("=", 1)[1].strip().strip('"').strip("'")
                return not _is_placeholder(val)
    return False


_OPENAI_KEY_AVAILABLE = not _is_placeholder(os.environ.get("OPENAI_API_KEY", ""))
_PROVIDER_KEY_AVAILABLE = _OPENAI_KEY_AVAILABLE or _openai_key_in_env_file()

skip_no_provider_key = pytest.mark.skipif(
    not _PROVIDER_KEY_AVAILABLE,
    reason=(
        "OPENAI_API_KEY not set — skipping live LLM call test. "
        "Set OPENAI_API_KEY in .nasiko-local.env or as an environment variable "
        "to run this test. The test skips gracefully so CI passes without keys."
    ),
)


# ─── Test A: Source audit — zero provider keys ────────────────────────────────


def test_no_provider_keys_in_agent_source() -> None:
    """
    The a2a-gateway-demo source code must contain no hardcoded provider API keys.

    Checks all Python files under agents/a2a-gateway-demo/src/ for:
    - Any string starting with "sk-" followed by alphanumeric chars (key pattern)
    - ANTHROPIC_API_KEY defined as a literal (not read via os.getenv)
    - OPENROUTER_API_KEY defined as a literal

    "os.environ" references are fine (that's the correct pattern).
    The test does NOT flag legitimate env-var reads, only embedded literals.
    """
    agent_src = _REPO_ROOT / "agents" / "a2a-gateway-demo" / "src"
    assert agent_src.exists(), (
        f"Agent source directory not found: {agent_src}\n"
        "Ensure T10 (a2a-gateway-demo) has been completed by AgentMigrator."
    )

    py_files = list(agent_src.rglob("*.py"))
    assert len(py_files) > 0, f"No Python files found under {agent_src}"

    violations = []
    for py_file in py_files:
        content = py_file.read_text()
        lines = content.splitlines()
        for lineno, line in enumerate(lines, start=1):
            stripped = line.strip()
            # Skip comments
            if stripped.startswith("#"):
                continue
            # Detect hardcoded sk-... key pattern (not inside os.environ reference)
            import re

            if re.search(r'["\']sk-[a-zA-Z0-9_\-]{8,}', line):
                # Allow "sk-REDACTED" (documentation placeholder)
                if "sk-REDACTED" not in line:
                    violations.append(
                        f"{py_file.relative_to(_REPO_ROOT)}:{lineno}: "
                        f"possible hardcoded API key: {stripped[:80]}"
                    )
            # Detect ANTHROPIC_API_KEY not via os.getenv/os.environ
            if "ANTHROPIC_API_KEY" in line and "os.environ" not in line and "os.getenv" not in line:
                if "=" in line and "ANTHROPIC_API_KEY" in line.split("=")[0]:
                    violations.append(
                        f"{py_file.relative_to(_REPO_ROOT)}:{lineno}: "
                        f"possible hardcoded ANTHROPIC_API_KEY: {stripped[:80]}"
                    )
            # Detect OPENROUTER_API_KEY not via os.getenv/os.environ
            if "OPENROUTER_API_KEY" in line and "os.environ" not in line and "os.getenv" not in line:
                if "=" in line and "OPENROUTER_API_KEY" in line.split("=")[0]:
                    violations.append(
                        f"{py_file.relative_to(_REPO_ROOT)}:{lineno}: "
                        f"possible hardcoded OPENROUTER_API_KEY: {stripped[:80]}"
                    )

    assert not violations, (
        "Provider API keys found in agent source — this violates the Track 2 contract.\n"
        "Violations:\n" + "\n".join(violations)
    )


# ─── Test B: LLM call through gateway using virtual key ──────────────────────


@skip_no_provider_key
def test_llm_call_via_gateway_with_virtual_key(
    compose_stack: None,
    mint_virtual_key: str,
) -> None:
    """
    Make a real LLM call through the gateway using a virtual key.

    Uses the openai Python package's synchronous client. The call:
      client = OpenAI(base_url="http://localhost:4100", api_key=virtual_key)
      resp = client.chat.completions.create(model="gpt-4o-mini", messages=[...])

    This is functionally identical to what the a2a-gateway-demo agent does at
    runtime (except the agent uses "default-model" and runs inside agents-net).

    We use "gpt-4o-mini" directly here (not "default-model") so the test is
    independent of whatever provider the rotation alias is currently pointed at.
    """
    try:
        from openai import OpenAI
    except ImportError:
        pytest.skip(
            "openai package not installed. "
            "Install with: pip install openai>=1.57.0"
        )

    client = OpenAI(
        base_url=f"{GATEWAY_HOST_URL}/v1",
        api_key=mint_virtual_key,
        timeout=30.0,
    )

    response = client.chat.completions.create(
        model="gpt-4o-mini",
        messages=[{"role": "user", "content": "Reply with exactly the word: pong"}],
        max_tokens=10,
    )

    assert response.choices, "LLM response has no choices"
    content = response.choices[0].message.content
    assert content, (
        "LLM response content is empty. "
        f"Full response: {response.model_dump()}"
    )
    # Light sanity check: the model followed the instruction somewhat
    # (don't enforce exact match since models vary)
    assert len(content.strip()) > 0, "Response content is blank"


@skip_no_provider_key
def test_llm_call_with_default_model_alias(
    compose_stack: None,
    mint_virtual_key: str,
) -> None:
    """
    Make an LLM call using the "default-model" alias.

    This alias is what agents are configured to call. It routes to whatever
    provider is currently configured in cli/setup/litellm/config.yaml.
    This test proves the rotation alias works end-to-end.
    """
    try:
        from openai import OpenAI
    except ImportError:
        pytest.skip("openai package not installed")

    client = OpenAI(
        base_url=f"{GATEWAY_HOST_URL}/v1",
        api_key=mint_virtual_key,
        timeout=30.0,
    )

    response = client.chat.completions.create(
        model="default-model",
        messages=[{"role": "user", "content": "Say hello in one word"}],
        max_tokens=10,
    )

    assert response.choices, "No choices in LLM response via default-model alias"
    content = response.choices[0].message.content
    assert content and len(content.strip()) > 0, (
        "Empty response via default-model alias. "
        f"Full response: {response.model_dump()}"
    )

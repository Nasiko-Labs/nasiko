"""Test scaffolding for Track 2 integration tests.

The orchestrator drags in Phoenix / OpenTelemetry / `app.*` packages that
aren't needed to exercise the env-injection path. Rather than install the
full production stack for a unit-level test, we stub the two heavy imports
so ``redis_stream_listener`` can be imported in isolation.
"""

from __future__ import annotations

import sys
import types

import pytest


def _install_observability_stubs() -> None:
    if "app.utils.observability.injector" in sys.modules:
        return

    app_pkg = types.ModuleType("app")
    app_pkg.__path__ = []  # mark as package
    utils_pkg = types.ModuleType("app.utils")
    utils_pkg.__path__ = []
    obs_pkg = types.ModuleType("app.utils.observability")
    obs_pkg.__path__ = []

    injector_mod = types.ModuleType("app.utils.observability.injector")

    class _StubTracingInjector:
        def __init__(self, *a, **kw):
            pass

        def inject_into_agent(self, *a, **kw):
            return None

    injector_mod.TracingInjector = _StubTracingInjector

    config_mod = types.ModuleType("app.utils.observability.config")

    class _StubObservabilityConfig:
        pass

    config_mod.ObservabilityConfig = _StubObservabilityConfig

    sys.modules.update({
        "app": app_pkg,
        "app.utils": utils_pkg,
        "app.utils.observability": obs_pkg,
        "app.utils.observability.injector": injector_mod,
        "app.utils.observability.config": config_mod,
    })


_install_observability_stubs()


@pytest.fixture(autouse=True)
def _reset_env(monkeypatch):
    """Clear LITELLM_* env so each test starts from a known state."""
    for key in (
        "LITELLM_ENABLED",
        "LITELLM_BASE_URL",
        "LITELLM_VIRTUAL_KEY",
        "LITELLM_DEFAULT_MODEL",
    ):
        monkeypatch.delenv(key, raising=False)
    yield

"""Root conftest.py — mock unavailable third-party modules before test collection.

arize-phoenix cannot be pip-installed on this system (numpy build failure),
but nasiko.app.utils.observability.mcp_tracing does
``from phoenix.otel import register`` at the top level.

This conftest inserts a mock ``phoenix`` package into ``sys.modules`` so that
the import succeeds during testing.  The mock ``register()`` returns a
MagicMock TracerProvider, which is sufficient for all test scenarios (the real
TracerProvider is only needed when exporting to a live Phoenix instance).
"""

import sys
from types import ModuleType
from unittest.mock import MagicMock

# ---------------------------------------------------------------------------
# Mock the phoenix.otel module hierarchy BEFORE any test file imports
# mcp_tracing.py.  pytest loads conftest.py before collecting tests, so this
# runs early enough.
# ---------------------------------------------------------------------------

if "phoenix" not in sys.modules:
    _phoenix = ModuleType("phoenix")
    _phoenix_otel = ModuleType("phoenix.otel")

    # register() returns a mock TracerProvider with add_span_processor()
    _mock_register = MagicMock(name="phoenix.otel.register")
    _mock_provider = MagicMock(name="MockTracerProvider")
    _mock_register.return_value = _mock_provider
    _phoenix_otel.register = _mock_register

    _phoenix.otel = _phoenix_otel

    sys.modules["phoenix"] = _phoenix
    sys.modules["phoenix.otel"] = _phoenix_otel

# ---------------------------------------------------------------------------
# Mock the tracing_utils, config, and injector modules that the observability
# __init__.py tries to import.  These modules exist in the main Nasiko repo
# but are NOT present in the stack-up repo.
# ---------------------------------------------------------------------------

_obs_base = "nasiko.app.utils.observability"

for _submod_name in ("tracing_utils", "config", "injector"):
    _fqn = f"{_obs_base}.{_submod_name}"
    if _fqn not in sys.modules:
        _mod = ModuleType(_fqn)
        if _submod_name == "tracing_utils":
            _mod.bootstrap_tracing = MagicMock(name="bootstrap_tracing")
        elif _submod_name == "config":
            _mod.ObservabilityConfig = MagicMock(name="ObservabilityConfig")
        elif _submod_name == "injector":
            _mod.TracingInjector = MagicMock(name="TracingInjector")
        sys.modules[_fqn] = _mod

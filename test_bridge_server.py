"""
Observability utilities for the Nasiko MCP Bridge.

This package provides tracing instrumentation for the MCP bridge server.
Agent-side tracing (tracing_utils.py, config.py, injector.py) lives in the
reference Nasiko repo and is NOT included here — only MCP bridge tracing.
"""

# MCP bridge tracing — the only observability module in this repo
try:
    from .mcp_tracing import (
        bootstrap_mcp_tracing,
        instrument_mcp_bridge,
        create_tool_call_span,
        record_tool_result,
        record_tool_error,
    )

    __all__ = [
        "bootstrap_mcp_tracing",
        "instrument_mcp_bridge",
        "create_tool_call_span",
        "record_tool_result",
        "record_tool_error",
    ]
except ImportError:
    # OpenTelemetry not installed — tracing will be silently disabled.
    # The bridge server has its own try/except guard in server.py.
    __all__ = []

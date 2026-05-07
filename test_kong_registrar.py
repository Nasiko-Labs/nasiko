"""
mcp_tracing.py — OpenTelemetry tracing utilities for MCP (Model Context Protocol) servers.

This module is the MCP counterpart of tracing_utils.py.  While tracing_utils.py
handles *agent-side* tracing (LangChain, CrewAI, etc.), this module handles
*MCP-bridge-side* tracing so we can see every tool call that flows through the
FastAPI bridge in the Phoenix UI.

Main capabilities:
  1. bootstrap_mcp_tracing  – create a TracerProvider  & return a Tracer
  2. instrument_mcp_bridge  – auto-instrument the FastAPI app (incl. W3C
     trace-context propagation so agent → bridge spans are linked)
  3. create_tool_call_span  – open a span for a single MCP tool invocation
  4. record_tool_result     – attach the tool result to the span
  5. record_tool_error      – mark the span as failed and record the exception

Dependency note:
  This module requires `opentelemetry-instrumentation-fastapi`.  Make sure it
  is listed in the bridge's requirements.txt / pyproject.toml:
      opentelemetry-instrumentation-fastapi>=0.48b0
"""

import os
import json
import logging
from typing import Optional
from contextlib import contextmanager

# ---------------------------------------------------------------------------
# OpenTelemetry core SDK – same packages that tracing_utils.py already uses
# ---------------------------------------------------------------------------
from opentelemetry import trace
from opentelemetry.sdk.trace.export import SimpleSpanProcessor
from opentelemetry.exporter.otlp.proto.http.trace_exporter import OTLPSpanExporter
from opentelemetry.trace import StatusCode

# Phoenix helper — imported lazily inside bootstrap_mcp_tracing() to avoid
# hard dependency.  When phoenix is not installed, the module still loads
# and all functions work (tracing is just silently disabled).
_register = None  # set by bootstrap_mcp_tracing if phoenix is available

# ---------------------------------------------------------------------------
# Logger – uses the same "observability" logger namespace as tracing_utils.py
# ---------------------------------------------------------------------------
logger = logging.getLogger("observability")


# ============================================================================
# 1.  bootstrap_mcp_tracing
# ============================================================================

def bootstrap_mcp_tracing(
    project_name: str,
    endpoint: Optional[str] = None,
):
    """
    Initialise OpenTelemetry tracing for an MCP server and return a Tracer.

    This is the MCP equivalent of ``bootstrap_tracing()`` in tracing_utils.py.
    Agents call ``bootstrap_tracing()``; the MCP bridge calls this function
    instead.

    What it does, step by step:
      1. Reads the TRACING_ENABLED env var — if it's "false", tracing is
         skipped and the function returns ``None``.
      2. Resolves the Phoenix collector endpoint from (in priority order):
         a) the ``endpoint`` parameter,
         b) the PHOENIX_COLLECTOR_ENDPOINT env var,
         c) the fallback "http://localhost:6006/v1/traces".
      3. Creates a TracerProvider via ``phoenix.otel.register()`` — this is the
         same helper the agent-side code uses.
      4. Attaches a ``SimpleSpanProcessor`` + ``OTLPSpanExporter`` so spans are
         shipped to Phoenix immediately (no batching).
      5. Returns a *Tracer* (not the provider) so callers can create spans.

    Args:
        project_name: Identifies this service in the Phoenix UI.
                      Example: "mcp-bridge" or "mcp-github-tools".
        endpoint:     Override for the OTLP HTTP endpoint.  Leave as None to
                      read from the environment.

    Returns:
        A ``opentelemetry.trace.Tracer`` instance, or ``None`` if tracing is
        disabled.
    """

    # --- Step 1: check the kill-switch ----------------------------------------
    if os.getenv("TRACING_ENABLED", "true").lower() == "false":
        logger.info(f"Tracing is disabled (TRACING_ENABLED=false) for {project_name}")
        return None

    # --- Step 2: resolve the collector endpoint --------------------------------
    if endpoint is None:
        endpoint = os.getenv(
            "PHOENIX_COLLECTOR_ENDPOINT",
            "http://localhost:6006/v1/traces",
        )

    logger.info(
        f"Bootstrapping MCP tracing for '{project_name}' -> {endpoint}"
    )

    try:
        # --- Step 3: create the TracerProvider via Phoenix ----------------------
        # ``register()`` creates a TracerProvider *and* sets it as the global
        # provider.  ``auto_instrument=False`` because we don't need LangChain /
        # CrewAI instrumentors here — we only care about FastAPI + manual spans.
        from phoenix.otel import register
        provider = register(
            project_name=project_name,
            endpoint=endpoint,
            auto_instrument=False,
        )

        # --- Step 4: add the span exporter ------------------------------------
        # SimpleSpanProcessor exports each span synchronously as soon as it
        # ends.  This is simpler than BatchSpanProcessor and fine for the
        # bridge's traffic volume.
        provider.add_span_processor(
            SimpleSpanProcessor(OTLPSpanExporter(endpoint=endpoint))
        )

        # --- Step 5: obtain a Tracer from the provider -------------------------
        # A Tracer is a lightweight object that creates spans.  We name it after
        # this module so Phoenix can show where spans originated.
        tracer = trace.get_tracer(
            instrumenting_module_name="nasiko.mcp_tracing",
            tracer_provider=provider,
        )

        logger.info(f"✅ MCP tracing initialised for '{project_name}'")
        return tracer

    except Exception as exc:
        logger.error(f"❌ Failed to initialise MCP tracing: {exc}")
        return None


# ============================================================================
# 2.  instrument_mcp_bridge
# ============================================================================

def instrument_mcp_bridge(app):
    """
    Auto-instrument a FastAPI application with OpenTelemetry.

    Why this matters:
      When an agent makes an HTTP call to the bridge, it sends a ``traceparent``
      header (W3C Trace Context).  The FastAPI instrumentor reads that header
      and automatically creates a *child span* linked to the agent's trace.
      This means you see a single, end-to-end trace in Phoenix:

          Agent span  →  Bridge HTTP span  →  Tool-call span

    What it does:
      - Wraps every incoming request in an OpenTelemetry span.
      - Extracts W3C ``traceparent`` / ``tracestate`` headers so the bridge's
        spans show up as children of the calling agent's trace.

    Args:
        app:  The FastAPI application instance (the ``app`` object from
              ``server.py``).

    Dependency note:
        Requires ``opentelemetry-instrumentation-fastapi``.  Install it with:
            pip install opentelemetry-instrumentation-fastapi>=0.48b0
    """
    try:
        from opentelemetry.instrumentation.fastapi import FastAPIInstrumentor

        # ``.instrument_app()`` monkey-patches the ASGI app so that every
        # request automatically gets a span.
        FastAPIInstrumentor.instrument_app(app)
        logger.info("✅ FastAPI auto-instrumentation applied to MCP bridge")

    except ImportError:
        # If the package isn't installed, log a clear error but don't crash
        # the bridge — it should still work, just without trace propagation.
        logger.error(
            "❌ Could not instrument FastAPI — the package "
            "'opentelemetry-instrumentation-fastapi' is not installed.  "
            "Run: pip install opentelemetry-instrumentation-fastapi>=0.48b0"
        )
    except Exception as exc:
        logger.error(f"❌ Failed to instrument FastAPI: {exc}")


# ============================================================================
# 3.  create_tool_call_span
# ============================================================================


class _NullSpan:
    """
    A harmless stand-in for a real Span when tracing is disabled.

    All method calls (set_attribute, set_status, record_exception, etc.) are
    silently ignored.  This lets callers use the *same* code path regardless
    of whether tracing is on or off — no ``if tracer:`` checks needed.
    """

    def set_attribute(self, *args, **kwargs):
        pass

    def set_status(self, *args, **kwargs):
        pass

    def record_exception(self, *args, **kwargs):
        pass


@contextmanager
def create_tool_call_span(tracer, tool_name, arguments, server_name, artifact_id):
    """
    Context manager that creates an OpenTelemetry span for a single MCP tool call.

    **Null-safe**: If ``tracer`` is ``None`` (tracing disabled), this yields a
    no-op ``_NullSpan`` instead of crashing.  You can always write::

        with create_tool_call_span(tracer, ...) as span:
            result = do_work()
            record_tool_result(span, result)

    …without checking whether tracing is active.

    Attributes set on the span (these follow the emerging MCP semantic
    conventions so all MCP servers look consistent in Phoenix):

        mcp.tool.name       – e.g. "read_file"
        mcp.tool.arguments  – JSON-encoded dict of arguments
        mcp.server.name     – human-readable server name
        mcp.server.id       – the Nasiko artifact ID
        mcp.transport       – always "stdio" (the bridge talks to MCP servers
                              over stdin/stdout)

    Args:
        tracer:       The Tracer returned by ``bootstrap_mcp_tracing()``,
                      or ``None`` if tracing is disabled.
        tool_name:    Name of the MCP tool being called.
        arguments:    Dict of arguments passed to the tool.
        server_name:  Human-readable name of the MCP server.
        artifact_id:  The Nasiko artifact ID that identifies this server.

    Yields:
        The active ``opentelemetry.trace.Span``, or a ``_NullSpan`` if tracing
        is disabled.
    """

    # --- Null-safe path: if tracing is off, yield a no-op span ----------------
    if tracer is None:
        yield _NullSpan()
        return

    # --- Normal path: create a real OTel span ---------------------------------
    # ``record_exception=True`` means that if an unhandled exception escapes
    # the ``with`` block, OTel will automatically record it on the span.
    with tracer.start_as_current_span(
        name=f"mcp.tool/{tool_name}",
        record_exception=True,
        set_status_on_exception=True,
    ) as span:
        # Set MCP-specific attributes -------------------------------------------
        span.set_attribute("mcp.tool.name", tool_name)
        span.set_attribute("mcp.tool.arguments", json.dumps(arguments))
        span.set_attribute("mcp.server.name", server_name)
        span.set_attribute("mcp.server.id", artifact_id)
        span.set_attribute("mcp.transport", "stdio")

        # Hand the span to the caller so they can attach result / error
        yield span


# ============================================================================
# 4.  record_tool_result
# ============================================================================

def record_tool_result(span, result):
    """
    Attach a successful tool result to the span.

    **Null-safe**: if ``span`` is a ``_NullSpan`` (tracing disabled), this
    is a no-op.

    Args:
        span:   The span yielded by ``create_tool_call_span()``.
        result: The raw result object returned by the MCP tool (will be
                JSON-serialised before being stored as a span attribute).
    """
    if span is None:
        return
    try:
        span.set_attribute("mcp.tool.result", json.dumps(result))
        span.set_status(StatusCode.OK)
    except Exception as exc:
        # Don't let a serialisation failure crash the request — just log it.
        logger.warning(f"Failed to record tool result on span: {exc}")
        span.set_status(StatusCode.OK)


# ============================================================================
# 5.  record_tool_error
# ============================================================================

def record_tool_error(span, error):
    """
    Mark the span as failed and attach exception details.

    **Null-safe**: if ``span`` is a ``_NullSpan`` (tracing disabled), this
    is a no-op.

    Args:
        span:  The span yielded by ``create_tool_call_span()``.
        error: The exception / error object.  ``record_exception()`` will
               capture its type, message, and traceback.
    """
    if span is None:
        return
    span.set_status(StatusCode.ERROR, description=str(error))
    span.record_exception(error)

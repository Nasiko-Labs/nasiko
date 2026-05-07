"""
mcp_tracing_local.py — Lightweight OTel tracing that sends spans directly to
Phoenix's OTLP HTTP endpoint. No phoenix pip package required.

Drop-in replacement for mcp_tracing.py's public API:
  - bootstrap_mcp_tracing()
  - instrument_mcp_bridge()
  - create_tool_call_span()
  - record_tool_result()
  - record_tool_error()

Uses standard OpenTelemetry SDK + OTLP HTTP exporter instead of
phoenix.otel.register(), which requires arize-phoenix (numpy build failures).

Phoenix's OTLP HTTP collector runs on port 4318 (NOT 6006 which is the web UI).
"""

import os
import json
import logging
from typing import Optional
from contextlib import contextmanager

logger = logging.getLogger("observability")


# ── Null-safe fallback ─────────────────────────────────────────────
class _NullSpan:
    """No-op span when tracing is disabled or OTel packages missing."""
    def set_attribute(self, *a, **kw):
        pass

    def set_status(self, *a, **kw):
        pass

    def record_exception(self, *a, **kw):
        pass


def bootstrap_mcp_tracing(
    project_name: str,
    endpoint: Optional[str] = None,
):
    """Create a TracerProvider with OTLP HTTP exporter → Phoenix.

    Returns a tracer object, or None if tracing is disabled or
    OpenTelemetry packages are not installed.

    Parameters
    ----------
    project_name:
        Name shown in Phoenix's project list (e.g. "mcp-bridge").
    endpoint:
        OTLP HTTP endpoint. Defaults to PHOENIX_COLLECTOR_ENDPOINT env var,
        then falls back to http://localhost:4318/v1/traces.
    """
    if os.getenv("TRACING_ENABLED", "true").lower() == "false":
        logger.info(f"Tracing disabled for {project_name}")
        return None

    # Default to Phoenix's OTLP HTTP port (4318), NOT the UI port (6006)
    if endpoint is None:
        endpoint = os.getenv(
            "PHOENIX_COLLECTOR_ENDPOINT",
            "http://localhost:4318/v1/traces",
        )

    try:
        from opentelemetry import trace
        from opentelemetry.sdk.trace import TracerProvider
        from opentelemetry.sdk.resources import Resource
        from opentelemetry.sdk.trace.export import SimpleSpanProcessor
        from opentelemetry.exporter.otlp.proto.http.trace_exporter import (
            OTLPSpanExporter,
        )

        resource = Resource.create({
            "service.name": project_name,
            "project.name": project_name,  # Phoenix uses this for grouping
        })

        provider = TracerProvider(resource=resource)
        provider.add_span_processor(
            SimpleSpanProcessor(OTLPSpanExporter(endpoint=endpoint))
        )
        trace.set_tracer_provider(provider)

        tracer = trace.get_tracer("nasiko.mcp_tracing")
        logger.info(f"✅ MCP tracing active for '{project_name}' → {endpoint}")
        return tracer

    except ImportError as e:
        logger.error(f"❌ OTel packages not installed: {e}")
        logger.error(
            "Run: pip install opentelemetry-api opentelemetry-sdk "
            "opentelemetry-exporter-otlp-proto-http"
        )
        return None
    except Exception as e:
        logger.error(f"❌ Failed to init tracing: {e}")
        return None


def instrument_mcp_bridge(app) -> None:
    """Auto-instrument FastAPI with W3C trace context propagation."""
    try:
        from opentelemetry.instrumentation.fastapi import FastAPIInstrumentor
        FastAPIInstrumentor.instrument_app(app)
        logger.info("✅ FastAPI auto-instrumentation applied")
    except ImportError:
        logger.warning("⚠️ opentelemetry-instrumentation-fastapi not installed")
    except Exception as e:
        logger.error(f"❌ FastAPI instrumentation failed: {e}")


@contextmanager
def create_tool_call_span(tracer, tool_name, arguments, server_name="", artifact_id=""):
    """Create an OTel span for an MCP tool call.

    Null-safe: if tracer is None, yields a _NullSpan (no-op).
    """
    if tracer is None:
        yield _NullSpan()
        return

    with tracer.start_as_current_span(
        name=f"nasiko.mcp.tool_call.{tool_name}",
        record_exception=True,
        set_status_on_exception=True,
    ) as span:
        span.set_attribute("mcp.tool.name", tool_name)
        span.set_attribute("mcp.tool.arguments", json.dumps(arguments))
        span.set_attribute("mcp.server.name", server_name)
        span.set_attribute("mcp.server.id", artifact_id)
        span.set_attribute("mcp.transport", "stdio")
        yield span


def record_tool_result(span, result) -> None:
    """Attach successful result to span. Null-safe."""
    if span is None or isinstance(span, _NullSpan):
        return
    try:
        from opentelemetry.trace import StatusCode
        span.set_attribute("mcp.tool.result", json.dumps(result))
        span.set_status(StatusCode.OK)
    except Exception as e:
        logger.warning(f"Failed to record result: {e}")


def record_tool_error(span, error) -> None:
    """Mark span as failed. Null-safe."""
    if span is None or isinstance(span, _NullSpan):
        return
    try:
        from opentelemetry.trace import StatusCode
        span.set_status(StatusCode.ERROR, description=str(error))
        span.record_exception(error)
    except Exception:
        pass

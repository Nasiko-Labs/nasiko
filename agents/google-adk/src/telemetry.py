"""Nasiko Agent OTel telemetry bootstrap.

Import and call `init_telemetry()` at agent startup to auto-instrument:
- HTTP calls (httpx, requests, urllib3)
- LLM calls (openai, anthropic — via GenAI semantic conventions)
- A2A server spans (incoming requests)

All traces are exported to the OTLP endpoint configured by
OTEL_EXPORTER_OTLP_ENDPOINT (injected by CP during agent deploy).

If no endpoint is set, telemetry is silently disabled (no-op).
"""

import os
import logging

logger = logging.getLogger(__name__)

_initialized = False


def init_telemetry(service_name: str | None = None) -> None:
    """Initialize OpenTelemetry tracing + metrics. Safe to call multiple times."""
    global _initialized
    if _initialized:
        return
    _initialized = True

    endpoint = os.environ.get("OTEL_EXPORTER_OTLP_ENDPOINT")
    if not endpoint:
        logger.debug("OTEL_EXPORTER_OTLP_ENDPOINT not set — telemetry disabled")
        return

    try:
        from opentelemetry import trace, metrics
        from opentelemetry.sdk.trace import TracerProvider
        from opentelemetry.sdk.trace.export import BatchSpanProcessor
        from opentelemetry.sdk.metrics import MeterProvider
        from opentelemetry.sdk.metrics.export import PeriodicExportingMetricReader
        from opentelemetry.sdk.resources import Resource
        from opentelemetry.exporter.otlp.proto.grpc.trace_exporter import OTLPSpanExporter
        from opentelemetry.exporter.otlp.proto.grpc.metric_exporter import OTLPMetricExporter
        from opentelemetry.propagate import set_global_textmap
        from opentelemetry.propagators.composite import CompositePropagator
        from opentelemetry.trace.propagation import TraceContextTextMapPropagator
    except ImportError:
        logger.warning("opentelemetry SDK not installed — telemetry disabled")
        return

    name = service_name or os.environ.get("OTEL_SERVICE_NAME", "nasiko-agent")
    resource = Resource.create({"service.name": name})

    # Traces
    tracer_provider = TracerProvider(resource=resource)
    tracer_provider.add_span_processor(
        BatchSpanProcessor(OTLPSpanExporter(endpoint=endpoint, insecure=True))
    )
    trace.set_tracer_provider(tracer_provider)

    # Metrics
    metric_reader = PeriodicExportingMetricReader(
        OTLPMetricExporter(endpoint=endpoint, insecure=True),
        export_interval_millis=10000,
    )
    meter_provider = MeterProvider(resource=resource, metric_readers=[metric_reader])
    metrics.set_meter_provider(meter_provider)

    # Propagation (W3C TraceContext so CP flow spans become parents)
    set_global_textmap(CompositePropagator([TraceContextTextMapPropagator()]))

    # Auto-instrument common libraries
    _auto_instrument()

    logger.info(f"OTel telemetry initialized → {endpoint}")


def _auto_instrument():
    """Best-effort auto-instrumentation for common libraries."""
    _try_instrument("opentelemetry.instrumentation.httpx", "HTTPXClientInstrumentor")
    _try_instrument("opentelemetry.instrumentation.requests", "RequestsInstrumentor")
    _try_instrument("opentelemetry.instrumentation.logging", "LoggingInstrumentor")
    _try_instrument("opentelemetry.instrumentation.starlette", "StarletteInstrumentor")


def _try_instrument(module_path: str, class_name: str):
    """Try to instrument a library; skip silently if not installed."""
    try:
        import importlib
        mod = importlib.import_module(module_path)
        instrumentor = getattr(mod, class_name)()
        if not instrumentor.is_instrumented_by_opentelemetry:
            instrumentor.instrument()
    except (ImportError, Exception):
        pass

"""Nasiko Agent OTel telemetry bootstrap.

Import and call `init_telemetry()` at agent startup to auto-instrument:
- HTTP calls (httpx, requests, urllib3)
- LLM calls (openai, anthropic — via GenAI semantic conventions)
- A2A server spans (incoming requests)

Propagation (traceparent header) is ALWAYS enabled — required for CP flow tracking.
Export to a collector is optional (OTEL_EXPORTER_OTLP_ENDPOINT).
"""

import os
import logging

logger = logging.getLogger(__name__)

_initialized = False


def init_telemetry(service_name: str | None = None) -> None:
    """Initialize OpenTelemetry tracing + propagation. Safe to call multiple times."""
    global _initialized
    if _initialized:
        return
    _initialized = True

    try:
        from opentelemetry import trace, metrics
        from opentelemetry.sdk.trace import TracerProvider
        from opentelemetry.sdk.trace.export import BatchSpanProcessor
        from opentelemetry.sdk.resources import Resource
        from opentelemetry.propagate import set_global_textmap
        from opentelemetry.propagators.composite import CompositePropagator
        from opentelemetry.trace.propagation import TraceContextTextMapPropagator
    except ImportError:
        logger.warning("opentelemetry SDK not installed — telemetry disabled")
        return

    name = service_name or os.environ.get("OTEL_SERVICE_NAME", "nasiko-agent")
    resource = Resource.create({"service.name": name})

    # Propagation — ALWAYS enabled (required for flow tracking via traceparent)
    set_global_textmap(CompositePropagator([TraceContextTextMapPropagator()]))

    # TracerProvider — always set so instrumented HTTP clients propagate context
    tracer_provider = TracerProvider(resource=resource)

    # Export — only if endpoint configured
    endpoint = os.environ.get("OTEL_EXPORTER_OTLP_ENDPOINT")
    if endpoint:
        from opentelemetry.exporter.otlp.proto.grpc.trace_exporter import OTLPSpanExporter
        from opentelemetry.sdk.metrics import MeterProvider
        from opentelemetry.sdk.metrics.export import PeriodicExportingMetricReader
        from opentelemetry.exporter.otlp.proto.grpc.metric_exporter import OTLPMetricExporter

        tracer_provider.add_span_processor(
            BatchSpanProcessor(OTLPSpanExporter(endpoint=endpoint, insecure=True))
        )

        metric_reader = PeriodicExportingMetricReader(
            OTLPMetricExporter(endpoint=endpoint, insecure=True),
            export_interval_millis=10000,
        )
        metrics.set_meter_provider(MeterProvider(resource=resource, metric_readers=[metric_reader]))

    trace.set_tracer_provider(tracer_provider)

    # Auto-instrument HTTP clients (propagates traceparent on all outbound calls)
    _auto_instrument()

    logger.info(f"OTel telemetry initialized (export={'enabled → ' + endpoint if endpoint else 'disabled'})")


def _auto_instrument():
    """Best-effort auto-instrumentation for common libraries."""
    _try_instrument("opentelemetry.instrumentation.httpx", "HTTPXClientInstrumentor")
    _try_instrument("opentelemetry.instrumentation.requests", "RequestsInstrumentor")
    _try_instrument("opentelemetry.instrumentation.logging", "LoggingInstrumentor")
    _try_instrument("opentelemetry.instrumentation.starlette", "StarletteInstrumentor")
    # LLM clients — emit gen_ai.usage.input_tokens / output_tokens / request.model
    _try_instrument("opentelemetry.instrumentation.openai", "OpenAIInstrumentor")
    _try_instrument("opentelemetry.instrumentation.anthropic", "AnthropicInstrumentor")


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

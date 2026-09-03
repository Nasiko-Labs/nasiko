"""Nasiko Agent OTel telemetry bootstrap.

Import and call `init_telemetry()` at agent startup to auto-instrument:
- HTTP calls (httpx, requests, urllib3)
- LLM calls (openai, anthropic — via GenAI semantic conventions)
- A2A server spans (incoming requests)

Propagation (traceparent header) is ALWAYS enabled — required for CP flow tracking.
Export to a collector is optional (OTEL_EXPORTER_OTLP_ENDPOINT).
"""

import contextvars
import os
import logging

logger = logging.getLogger(__name__)

_initialized = False

# ContextVar populated by TraceparentMiddleware for each incoming HTTP request.
# The extracted OTel context (derived from the W3C traceparent header) is stored
# here so that execute(), which runs in a background asyncio Task, can use it
# as the parent when creating the agent.request span.
request_otel_context: contextvars.ContextVar = contextvars.ContextVar(
    "nasiko_request_otel_context",
    default=None,
)


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
        from opentelemetry.trace.propagation.tracecontext import TraceContextTextMapPropagator
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

    # Auto-instrument HTTP and LLM clients (propagates traceparent on all outbound calls).
    _auto_instrument()

    logger.info(f"OTel telemetry initialized (export={'enabled → ' + endpoint if endpoint else 'disabled'})")


def _auto_instrument():
    """Best-effort auto-instrumentation for common libraries."""
    _try_instrument("opentelemetry.instrumentation.httpx", "HTTPXClientInstrumentor")
    _try_instrument("opentelemetry.instrumentation.requests", "RequestsInstrumentor")
    _try_instrument("opentelemetry.instrumentation.logging", "LoggingInstrumentor")
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


class TraceparentMiddleware:
    """Raw ASGI middleware that extracts the W3C traceparent header from each incoming
    HTTP request and stores the resulting OTel context in `request_otel_context`.

    This lets execute() — which runs in a background asyncio Task — reliably inherit
    the correct trace context for each request, preventing OTel context leakage.

    Using a raw ASGI middleware (rather than BaseHTTPMiddleware) avoids response
    buffering, which matters for the SSE streaming responses used by A2A.
    """

    def __init__(self, app):
        self.app = app

    async def __call__(self, scope, receive, send):
        if scope["type"] != "http":
            await self.app(scope, receive, send)
            return

        # Extract traceparent from ASGI scope headers (bytes).
        headers: dict[str, str] = {}
        for name_b, value_b in scope.get("headers", []):
            try:
                name = name_b.decode("latin-1")
                value = value_b.decode("latin-1")
                headers[name.lower()] = value
            except Exception:
                pass

        try:
            from opentelemetry.propagate import extract as otel_extract
            ctx = otel_extract(headers)
        except Exception:
            from opentelemetry import context as otel_context
            ctx = otel_context.Context()

        token = request_otel_context.set(ctx)
        try:
            await self.app(scope, receive, send)
        finally:
            request_otel_context.reset(token)

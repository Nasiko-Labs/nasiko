"""OTel bootstrap — copied from the common agent telemetry pattern."""

import contextvars
import logging
import os

logger = logging.getLogger(__name__)

_initialized = False

request_otel_context: contextvars.ContextVar = contextvars.ContextVar(
    "nasiko_request_otel_context",
    default=None,
)


def init_telemetry(service_name: str | None = None) -> None:
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

    # If opentelemetry-instrument already set up a TracerProvider, skip —
    # calling set_tracer_provider again raises "Overriding not allowed".
    from opentelemetry.sdk.trace import TracerProvider as SdkTracerProvider
    if isinstance(trace.get_tracer_provider(), SdkTracerProvider):
        logger.info("OTel TracerProvider already configured (opentelemetry-instrument); skipping init")
        set_global_textmap(CompositePropagator([TraceContextTextMapPropagator()]))
        return

    name = service_name or os.environ.get("OTEL_SERVICE_NAME", "nasiko-agent")
    resource = Resource.create({"service.name": name})
    set_global_textmap(CompositePropagator([TraceContextTextMapPropagator()]))
    tracer_provider = TracerProvider(resource=resource)

    endpoint = os.environ.get("OTEL_EXPORTER_OTLP_ENDPOINT")
    if endpoint:
        from opentelemetry.exporter.otlp.proto.grpc.trace_exporter import OTLPSpanExporter
        tracer_provider.add_span_processor(
            BatchSpanProcessor(OTLPSpanExporter(endpoint=endpoint, insecure=True))
        )

    trace.set_tracer_provider(tracer_provider)
    _auto_instrument()
    logger.info(f"OTel telemetry initialized (export={'enabled → ' + endpoint if endpoint else 'disabled'})")


def _auto_instrument():
    for mod, cls in [
        ("opentelemetry.instrumentation.httpx", "HTTPXClientInstrumentor"),
        ("opentelemetry.instrumentation.openai_v2", "OpenAIInstrumentor"),
    ]:
        try:
            import importlib
            m = importlib.import_module(mod)
            inst = getattr(m, cls)()
            if not inst.is_instrumented_by_opentelemetry:
                inst.instrument()
        except (ImportError, Exception):
            pass


class TraceparentMiddleware:
    def __init__(self, app):
        self.app = app

    async def __call__(self, scope, receive, send):
        if scope["type"] != "http":
            await self.app(scope, receive, send)
            return
        headers: dict[str, str] = {}
        for name_b, value_b in scope.get("headers", []):
            try:
                headers[name_b.decode("latin-1").lower()] = value_b.decode("latin-1")
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

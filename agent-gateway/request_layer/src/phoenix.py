"""Phoenix span helpers, built on Nasiko's shared tracing bootstrap."""
import logging
from contextlib import contextmanager
from typing import Iterator

from opentelemetry import trace
from opentelemetry.trace import Span, Status, StatusCode

logger = logging.getLogger(__name__)

_tracer: trace.Tracer | None = None


def bootstrap(project_name: str, endpoint: str) -> None:
    """Initialize tracing using the shared Nasiko bootstrapper.

    Imported lazily so unit tests that don't need OTel don't pay the
    instrumentation cost.
    """

    global _tracer
    try:
        from app.utils.observability.tracing_utils import bootstrap_tracing
    except ImportError:
        # Outside the Nasiko docker network (e.g. unit tests) this import
        # may not resolve. Fall back to a stand-alone tracer that emits to
        # the same OTLP endpoint.
        logger.info(
            "app.utils.observability not available; falling back to direct OTLP setup"
        )
        _tracer = _fallback_tracer(project_name, endpoint)
        return

    bootstrap_tracing(
        project_name=project_name,
        endpoint=endpoint,
        instrumentors=None,
        framework="fastapi",
    )
    _tracer = trace.get_tracer(project_name)


def _fallback_tracer(project_name: str, endpoint: str) -> trace.Tracer:
    """Set up a minimal OTLP tracer when the shared utility is unavailable."""

    from opentelemetry.exporter.otlp.proto.grpc.trace_exporter import (
        OTLPSpanExporter,
    )
    from opentelemetry.sdk.resources import Resource
    from opentelemetry.sdk.trace import TracerProvider
    from opentelemetry.sdk.trace.export import BatchSpanProcessor

    resource = Resource.create({"service.name": project_name})
    provider = TracerProvider(resource=resource)
    provider.add_span_processor(
        BatchSpanProcessor(OTLPSpanExporter(endpoint=endpoint, insecure=True))
    )
    trace.set_tracer_provider(provider)
    return trace.get_tracer(project_name)


def get_tracer() -> trace.Tracer:
    """Return the configured tracer; initializes a no-op if unset."""

    if _tracer is None:
        return trace.get_tracer("request_layer")
    return _tracer


@contextmanager
def cache_hit_span(
    *,
    layer: str,
    agent: str,
    similarity: float | None = None,
    matched_query: str | None = None,
    age_seconds: float | None = None,
    savings_usd: float = 0.0,
    savings_ms: float = 0.0,
    router_skipped: bool = False,
) -> Iterator[Span]:
    """Open a ``request_layer.cache.hit`` span pre-populated with attributes."""

    tracer = get_tracer()
    with tracer.start_as_current_span("request_layer.cache.hit") as span:
        span.set_attribute("cache.layer", layer)
        span.set_attribute("agent.name", agent)
        if similarity is not None:
            span.set_attribute("cache.similarity", similarity)
        if matched_query is not None:
            span.set_attribute("cache.matched_query", matched_query[:200])
        if age_seconds is not None:
            span.set_attribute("cache.age_seconds", age_seconds)
        span.set_attribute("cache.savings_usd", savings_usd)
        span.set_attribute("cache.savings_ms", savings_ms)
        span.set_attribute("cache.router_skipped", router_skipped)
        try:
            yield span
        except Exception as exc:  # noqa: BLE001
            span.set_status(Status(StatusCode.ERROR, str(exc)))
            raise


@contextmanager
def coalesce_follower_span(agent: str, query_hash: str) -> Iterator[Span]:
    tracer = get_tracer()
    with tracer.start_as_current_span("request_layer.coalesce.follower") as span:
        span.set_attribute("agent.name", agent)
        span.set_attribute("query.hash", query_hash)
        yield span


@contextmanager
def queue_span(name: str, agent: str, lane: str) -> Iterator[Span]:
    tracer = get_tracer()
    with tracer.start_as_current_span(name) as span:
        span.set_attribute("agent.name", agent)
        span.set_attribute("queue.lane", lane)
        yield span

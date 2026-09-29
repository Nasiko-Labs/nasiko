"""Minimal original tracing for the stdlib A2A worker; no prompt capture."""
from contextlib import contextmanager
import os

tracer=None
propagate=None
if os.getenv('OTEL_EXPORTER_OTLP_ENDPOINT'):
    from opentelemetry import trace, propagate
    from opentelemetry.exporter.otlp.proto.grpc.trace_exporter import OTLPSpanExporter
    from opentelemetry.sdk.resources import Resource
    from opentelemetry.sdk.trace import TracerProvider
    from opentelemetry.sdk.trace.export import BatchSpanProcessor
    provider=TracerProvider(resource=Resource.create({}))
    provider.add_span_processor(BatchSpanProcessor(OTLPSpanExporter(),schedule_delay_millis=500))
    trace.set_tracer_provider(provider)
    tracer=trace.get_tracer('agentkv.worker')


@contextmanager
def span(name, headers=None, **attributes):
    if tracer is None:
        yield None
        return
    context=propagate.extract({k.lower():v for k,v in headers.items()}) if headers is not None else None
    with tracer.start_as_current_span(name,context=context,attributes=attributes) as current:
        yield current


def outgoing():
    headers={}
    if propagate: propagate.inject(headers)
    return headers

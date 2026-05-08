"""
Gateway tracing utilities for LLM calls routed through LiteLLM.

Adds child spans to the current trace so gateway calls are visible in Phoenix
without modifying the existing trace structure.
"""

import logging
from contextlib import contextmanager
from typing import Optional

from opentelemetry import trace

logger = logging.getLogger("observability.gateway")

GATEWAY_URL = "http://litellm:4000/v1"


@contextmanager
def gateway_span(
    model: str, operation: str = "llm.completion", attributes: Optional[dict] = None
):
    """
    Context manager that wraps an LLM gateway call in an OpenTelemetry child span.

    Usage:
        with gateway_span(model="gpt-4o", attributes={"input.tokens": 100}):
            response = openai_client.chat.completions.create(...)

    The span appears as a child of the calling agent's active trace in Phoenix.
    """
    tracer = trace.get_tracer("nasiko.gateway")
    with tracer.start_as_current_span(operation) as span:
        span.set_attribute("llm.gateway.url", GATEWAY_URL)
        span.set_attribute("llm.model_name", model)
        span.set_attribute("llm.provider", "litellm-gateway")
        if attributes:
            for key, value in attributes.items():
                span.set_attribute(key, value)
        try:
            yield span
        except Exception as exc:
            span.record_exception(exc)
            span.set_status(trace.StatusCode.ERROR, str(exc))
            raise

"""
Phoenix / OpenTelemetry logger for Aegis firewall verdicts.

Subscribes to the ``firewall_verdict`` event bus channel and forwards
each verdict as a structured span/log to Arize Phoenix or any
OpenTelemetry-compatible collector.

If Phoenix / OTel SDK is not installed, logging falls back to stderr
so the app never crashes due to missing observability deps.

Usage::

    from nasiko.phoenix_logger import register
    register()   # call once at startup
"""

from __future__ import annotations

import sys
import logging
from typing import Any

from events.event_bus import bus
from firewall.models import FirewallVerdict

logger = logging.getLogger("aegis.phoenix")

# Try importing OpenTelemetry; graceful fallback if unavailable
_otel_available = False
_tracer = None

try:
    from opentelemetry import trace
    from opentelemetry.sdk.trace import TracerProvider
    from opentelemetry.sdk.trace.export import SimpleSpanProcessor

    # Try Phoenix exporter first, then console fallback
    try:
        from opentelemetry.exporter.otlp.proto.http.trace_exporter import OTLPSpanExporter

        provider = TracerProvider()
        exporter = OTLPSpanExporter(
            endpoint="http://localhost:6006/v1/traces",
        )
        provider.add_span_processor(SimpleSpanProcessor(exporter))
        trace.set_tracer_provider(provider)
        _tracer = trace.get_tracer("aegis-firewall")
        _otel_available = True
        logger.info("Phoenix/OTel tracing enabled → http://localhost:6006")
    except ImportError:
        logger.info("OTLP exporter not found; Phoenix tracing disabled")
except ImportError:
    logger.info("OpenTelemetry SDK not installed; tracing disabled")


async def _on_verdict(verdict: FirewallVerdict) -> None:
    """Handle a firewall_verdict event."""
    _log_to_stderr(verdict)

    if _otel_available and _tracer is not None:
        _log_to_phoenix(verdict)


def _log_to_stderr(verdict: FirewallVerdict) -> None:
    """Always log to stderr as a baseline."""
    d = verdict.decision.value
    tool = verdict.call.tool
    agent = verdict.call.agent
    risk = verdict.risk.score
    reason = verdict.risk.reason

    icon = {"ALLOW": "✓", "WARN": "⚠", "BLOCK": "✗", "PENDING": "?"}.get(d, "·")
    msg = f"[Aegis] {icon} {d:7} agent={agent} tool={tool} risk={risk:.2f} reason={reason}"

    if verdict.violation:
        msg += f" violation={verdict.violation.rule}"

    level = {
        "ALLOW": logging.DEBUG,
        "WARN": logging.WARNING,
        "BLOCK": logging.ERROR,
        "PENDING": logging.INFO,
    }.get(d, logging.INFO)

    logger.log(level, msg)


def _log_to_phoenix(verdict: FirewallVerdict) -> None:
    """Create an OTel span for the verdict."""
    if _tracer is None:
        return

    with _tracer.start_as_current_span("aegis.firewall.verdict") as span:
        span.set_attribute("aegis.agent", verdict.call.agent)
        span.set_attribute("aegis.tool", verdict.call.tool)
        span.set_attribute("aegis.call_id", verdict.call.call_id)
        span.set_attribute("aegis.risk_score", verdict.risk.score)
        span.set_attribute("aegis.risk_reason", verdict.risk.reason)
        span.set_attribute("aegis.decision", verdict.decision.value)

        if verdict.violation:
            span.set_attribute("aegis.violation_rule", verdict.violation.rule)
            span.set_attribute("aegis.violation_detail", verdict.violation.detail)

        # Mark blocked calls as error spans
        if verdict.decision.value == "BLOCK":
            span.set_status(trace.StatusCode.ERROR, "Tool call blocked by Aegis")

        # Set args as event (not attribute — can be large)
        span.add_event("tool_call_args", attributes={
            k: str(v) for k, v in verdict.call.args.items()
        })


def register() -> None:
    """
    Register the Phoenix logger with the Aegis event bus.
    Call once at application startup.
    """
    bus.subscribe("firewall_verdict", _on_verdict)
    logger.info("Aegis Phoenix logger registered")

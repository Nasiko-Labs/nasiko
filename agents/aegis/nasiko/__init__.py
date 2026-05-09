"""
Nasiko integration layer for Aegis.

Provides:
- AegisMiddleware: ASGI middleware for request-level agent attribution
- AegisExecutorMixin: intercepts tool calls inside agent executors
- route(): convenience wrapper for ad-hoc firewall checks
- AgentCard parser: extracts skill metadata for policy auto-population
- Phoenix logger: forwards firewall verdicts to Phoenix/OpenTelemetry
"""

from .router_hook import AegisMiddleware, AegisExecutorMixin, route
from .agentcard_parser import parse_agentcard
from .phoenix_logger import register

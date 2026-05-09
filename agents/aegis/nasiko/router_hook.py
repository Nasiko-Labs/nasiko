"""
Nasiko Router Hook — Aegis integration for Nasiko agents.

Three integration levels:

1. AegisMiddleware  — ASGI middleware; attach to any FastAPI app for
   request-level agent attribution.  Blocking is NOT done here
   (tool name isn't known yet); use the executor mixin for that.

2. AegisExecutorMixin — Drop into any agent executor class to route
   every _call_tool() invocation through the Aegis firewall.

3. route()  — One-shot convenience wrapper for ad-hoc tool calls.
"""

from __future__ import annotations

import asyncio
import uuid
from typing import Any, Callable, Awaitable

from firewall.models import ToolCall, Decision
from firewall.firewall import firewall
from traces.storage import trace_store


# ---------------------------------------------------------------------------
# 1.  ASGI Middleware
# ---------------------------------------------------------------------------

class AegisMiddleware:
    """
    Wraps a Nasiko agent's ASGI app so every inbound request carries the
    agent name for downstream attribution.

    Usage::

        from nasiko.router_hook import AegisMiddleware
        app = AegisMiddleware(app, agent_name="github-agent")
    """

    def __init__(self, app, agent_name: str = "unknown"):
        self.app = app
        self.agent_name = agent_name

    async def __call__(self, scope, receive, send):
        if scope["type"] in ("http", "websocket"):
            # Inject agent name into ASGI scope so downstream handlers
            # can read  scope["state"]["aegis_agent"]
            scope.setdefault("state", {})
            scope["state"]["aegis_agent"] = self.agent_name
        await self.app(scope, receive, send)


# ---------------------------------------------------------------------------
# 2.  Executor Mixin
# ---------------------------------------------------------------------------

class AegisExecutorMixin:
    """
    Mix into any agent executor that exposes a ``_call_tool(name, args)``
    method.  The mixin intercepts the call and routes it through the
    Aegis firewall before the real method runs.

    Usage::

        class SecureExecutor(AegisExecutorMixin, OpenAIAgentExecutor):
            _aegis_agent_name = "github-agent"
    """

    _aegis_agent_name: str = "unknown"

    async def _call_tool(self, tool_name: str, args: dict[str, Any], **kwargs) -> Any:
        return await route(
            tool_name,
            args,
            lambda: super()._call_tool(tool_name, args, **kwargs),  # type: ignore[misc]
            agent=self._aegis_agent_name,
        )


# ---------------------------------------------------------------------------
# 3.  Convenience helper
# ---------------------------------------------------------------------------

async def route(
    tool_name: str,
    args: dict[str, Any],
    fn: Callable[..., Any],
    agent: str = "unknown",
) -> Any:
    """
    One-shot firewall check.

    Usage::

        result = await route("my_tool", {"arg": "val"}, my_async_fn, agent="my-agent")
    """
    call = ToolCall(
        tool=tool_name,
        args=args,
        agent=agent,
        call_id=str(uuid.uuid4())[:8],
    )
    verdict = await firewall.evaluate(call)
    trace_store.record(verdict)

    if verdict.decision in (Decision.ALLOW, Decision.WARN):
        result = fn()
        return await result if asyncio.iscoroutine(result) or asyncio.isfuture(result) else result
    else:
        reason = verdict.violation.detail if verdict.violation else verdict.risk.reason
        raise PermissionError(
            f"[{verdict.decision.value}] {tool_name} blocked — {reason}"
        )

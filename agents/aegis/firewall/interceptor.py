import uuid
import asyncio
from typing import Any, Callable
from .models import Decision, ToolCall
from .firewall import firewall
from traces.storage import trace_store


async def intercept(tool_name: str, args: dict[str, Any], fn: Callable, agent: str = "demo_agent") -> Any:
    call = ToolCall(tool=tool_name, args=args, agent=agent, call_id=str(uuid.uuid4())[:8])
    verdict = await firewall.evaluate(call)
    trace_store.record(verdict)

    if verdict.decision in (Decision.ALLOW, Decision.WARN):
        result = fn()
        return await result if asyncio.iscoroutine(result) else result
    else:
        raise PermissionError(f"[{verdict.decision}] {tool_name} blocked — {verdict.violation}")

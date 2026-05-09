import asyncio
from .models import Decision, FirewallVerdict, ToolCall
from .risk_engine import score
from .policy_engine import PolicyEngine
from events.event_bus import bus
from cache.cache_manager import cache_manager


class Firewall:
    def __init__(self):
        self._policy = PolicyEngine()
        self._approval_events: dict[str, asyncio.Event] = {}
        self._approval_results: dict[str, bool] = {}

    async def evaluate(self, call: ToolCall) -> FirewallVerdict:
        risk = score(call)
        violation = self._policy.check(call, risk)

        if violation:
            verdict = FirewallVerdict(call, risk, Decision.BLOCK, violation)
        elif self._policy.needs_approval(call):
            verdict = FirewallVerdict(call, risk, Decision.PENDING)
            await bus.publish("approval_request", verdict)
            approved = await self._wait_for_approval(call.call_id)
            verdict.decision = Decision.ALLOW if approved else Decision.BLOCK
        elif risk.score >= 0.5:
            verdict = FirewallVerdict(call, risk, Decision.WARN)
        else:
            verdict = FirewallVerdict(call, risk, Decision.ALLOW)

        await bus.publish("firewall_verdict", verdict)
        return verdict

    async def _wait_for_approval(self, call_id: str, timeout: float = 30.0) -> bool:
        ev = asyncio.Event()
        self._approval_events[call_id] = ev
        try:
            await asyncio.wait_for(ev.wait(), timeout=timeout)
            return self._approval_results.get(call_id, False)
        except asyncio.TimeoutError:
            return False
        finally:
            self._approval_events.pop(call_id, None)
            self._approval_results.pop(call_id, None)

    def resolve_approval(self, call_id: str, approved: bool) -> None:
        self._approval_results[call_id] = approved
        if ev := self._approval_events.get(call_id):
            ev.set()
    
    def handle_user_input(self, query: str)-> None: 
        asyncio.create_task(bus.publish("user_query", query))
    
    async def evaluate_with_cache(self, call: ToolCall) -> FirewallVerdict:
        return await cache_manager.execute(
            agent_name=call.agent,
            payload={"tool": call.tool, "args": call.args},
            execution_fn=lambda: self._perform_full_evaluation(call)
        )
        
firewall = Firewall()

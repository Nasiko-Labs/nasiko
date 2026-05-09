import json
from pathlib import Path
from firewall.models import FirewallVerdict, TraceEvent


class TraceStore:
    def __init__(self, session_dir: str = "sessions"):
        self._dir = Path(session_dir)
        self._dir.mkdir(exist_ok=True)
        self._events: list[TraceEvent] = []

    def record(self, verdict: FirewallVerdict) -> None:
        ev = TraceEvent(
            call_id=verdict.call.call_id,
            tool=verdict.call.tool,
            agent=verdict.call.agent,
            risk_score=verdict.risk.score,
            decision=verdict.decision.value,
            reason=verdict.risk.reason,
            timestamp=verdict.call.timestamp,
        )
        self._events.append(ev)
        self._persist(ev)

    def _persist(self, ev: TraceEvent) -> None:
        log_file = self._dir / "trace.jsonl"
        with open(log_file, "a") as f:
            f.write(json.dumps({
                "call_id": ev.call_id,
                "tool": ev.tool,
                "agent": ev.agent,
                "risk_score": ev.risk_score,
                "decision": ev.decision,
                "reason": ev.reason,
                "timestamp": ev.timestamp.isoformat(),
            }) + "\n")

    def all(self) -> list[TraceEvent]:
        return list(self._events)


trace_store = TraceStore()

import json
from pathlib import Path
from firewall.models import TraceEvent
from datetime import datetime


def load_session(path: str = "sessions/trace.jsonl") -> list[TraceEvent]:
    p = Path(path)
    if not p.exists():
        return []
    events = []
    with open(p) as f:
        for line in f:
            d = json.loads(line)
            events.append(TraceEvent(
                call_id=d["call_id"],
                tool=d["tool"],
                agent=d["agent"],
                risk_score=d["risk_score"],
                decision=d["decision"],
                reason=d["reason"],
                timestamp=datetime.fromisoformat(d["timestamp"]),
            ))
    return events

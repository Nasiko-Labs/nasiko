from dataclasses import dataclass, field
from datetime import datetime
from enum import Enum
from typing import Any


class Decision(str, Enum):
    ALLOW = "ALLOW"
    WARN = "WARN"
    BLOCK = "BLOCK"
    PENDING = "PENDING"


@dataclass
class ToolCall:
    tool: str
    args: dict[str, Any]
    agent: str = "demo_agent"
    call_id: str = ""
    timestamp: datetime = field(default_factory=datetime.utcnow)


@dataclass
class RiskResult:
    score: float          # 0.0 – 1.0
    reason: str


@dataclass
class PolicyViolation:
    rule: str
    detail: str


@dataclass
class FirewallVerdict:
    call: ToolCall
    risk: RiskResult
    decision: Decision
    violation: PolicyViolation | None = None


@dataclass
class TraceEvent:
    call_id: str
    tool: str
    agent: str
    risk_score: float
    decision: str
    reason: str
    timestamp: datetime = field(default_factory=datetime.utcnow)

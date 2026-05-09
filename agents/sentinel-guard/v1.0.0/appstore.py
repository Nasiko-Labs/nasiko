import time
from collections import deque
from dataclasses import dataclass, field
from typing import Any


@dataclass
class CacheEntry:
    query: str
    vector: list[float]
    response: Any
    agent: str
    hits: int = 0
    created_at: float = field(default_factory=time.time)
    last_hit: float = field(default_factory=time.time)


# ── In-memory stores ────────────────────────────────────────────────
cache_store: dict[str, list[CacheEntry]] = {}
cache_hits: dict[str, int] = {}
cache_misses: dict[str, int] = {}

rate_windows: dict[str, deque] = {}
rate_limits: dict[str, int] = {}
DEFAULT_RATE_LIMIT_RPM = 60
queue_depths: dict[str, int] = {}


# ── Decision log ────────────────────────────────────────────────────
@dataclass
class Decision:
    timestamp: float
    agent: str
    query: str
    outcome: str
    similarity: float | None = None
    queue_position: int | None = None
    estimated_wait_ms: int | None = None
    latency_ms: float | None = None


decision_log: deque[Decision] = deque(maxlen=50)


def record_decision(d: Decision):
    decision_log.append(d)

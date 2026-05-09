"""
Shared state store for Sentinel Guard.
In-memory data structures backed by Redis for durability.
"""

import time
from collections import deque
from dataclasses import dataclass, field
from typing import Any, Optional


# ── Cache Entry ────────────────────────────────────────────────────────────────


@dataclass
class CacheEntry:
    """A single cached agent response."""

    query: str
    response: Any
    agent: str
    cache_key: str
    similarity: float = 1.0
    hits: int = 0
    created_at: float = field(default_factory=time.time)
    last_hit: float = field(default_factory=time.time)
    ttl_seconds: int = 1800


# ── Decision Log ───────────────────────────────────────────────────────────────


@dataclass
class Decision:
    """Record of a single routing/caching decision."""

    timestamp: float
    agent: str
    query: str
    outcome: str  # "cache_hit_exact", "cache_hit_semantic", "cache_miss", "rate_limited", "queued", "forwarded"
    similarity: Optional[float] = None
    queue_position: Optional[int] = None
    estimated_wait_ms: Optional[int] = None
    latency_ms: Optional[float] = None
    source: str = ""  # "redis" or "semantic" or ""


# ── Global State ───────────────────────────────────────────────────────────────

# Decision log — ring buffer of last 500 decisions
decision_log: deque[Decision] = deque(maxlen=500)

# Per-agent counters
cache_hits: dict[str, int] = {}
cache_misses: dict[str, int] = {}
cache_semantic_hits: dict[str, int] = {}
requests_forwarded: dict[str, int] = {}
requests_queued: dict[str, int] = {}
requests_rejected: dict[str, int] = {}
total_latency_ms: dict[str, float] = {}
total_requests: dict[str, int] = {}

# Rate limit state
rate_limits: dict[str, int] = {}  # agent -> RPM limit


def record_decision(d: Decision) -> None:
    """Record a routing decision into the global log."""
    decision_log.append(d)


def increment_counter(store: dict[str, int], agent: str, amount: int = 1) -> None:
    """Increment a per-agent counter."""
    store[agent] = store.get(agent, 0) + amount


def increment_float(store: dict[str, float], agent: str, amount: float) -> None:
    """Increment a per-agent float counter."""
    store[agent] = store.get(agent, 0.0) + amount


def get_all_known_agents() -> set[str]:
    """Return the union of all agents seen across all counter stores."""
    return (
        set(cache_hits.keys())
        | set(cache_misses.keys())
        | set(requests_forwarded.keys())
        | set(requests_queued.keys())
        | set(rate_limits.keys())
        | set(total_requests.keys())
    )

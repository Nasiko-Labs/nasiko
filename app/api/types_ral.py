"""
RAL Pydantic response types for the backend API.
These follow the same typing conventions used across app/api/types.py.
"""

from __future__ import annotations

from typing import Any, Dict, List, Optional
from pydantic import BaseModel


# ── Per-agent stats ────────────────────────────────────────────────────────

class RalAgentStat(BaseModel):
    agent_id: str
    requests: int = 0
    errors: int = 0
    avg_latency_ms: float = 0.0
    # Rate-limiter snapshot
    tokens_available: Optional[float] = None
    burst_capacity: Optional[int] = None
    active_requests: int = 0
    max_concurrent: Optional[int] = None
    throttle_count: int = 0
    utilisation_pct: float = 0.0


# ── Cache stats ────────────────────────────────────────────────────────────

class RalCacheStats(BaseModel):
    hits: int = 0
    misses: int = 0
    hit_ratio: float = 0.0
    avg_latency_ms: float = 0.0


# ── Queue stats ────────────────────────────────────────────────────────────

class RalQueueStats(BaseModel):
    queue_size: int = 0
    max_queue_size: int = 0
    enqueued: int = 0
    completed: int = 0
    failed: int = 0
    retried: int = 0
    timed_out: int = 0
    dropped: int = 0


# ── Full metrics snapshot ──────────────────────────────────────────────────

class RalMetricsResponse(BaseModel):
    active_requests: int = 0
    requests_per_sec: float = 0.0
    avg_latency_ms: float = 0.0
    cache_hits: int = 0
    cache_misses: int = 0
    cache_hit_ratio: float = 0.0
    total_requests: int = 0
    total_errors: int = 0
    total_retries: int = 0
    total_throttled: int = 0
    dedup_in_flight: int = 0
    per_agent: Dict[str, Any] = {}
    rate_limiter: Dict[str, Any] = {}
    queue: RalQueueStats = RalQueueStats()
    cache: RalCacheStats = RalCacheStats()
    status_code: int = 200
    message: str = "ok"


# ── Request log entry ──────────────────────────────────────────────────────

class RalRequestLog(BaseModel):
    request_id: str
    query: str
    agent_id: str
    latency_ms: float
    from_cache: bool = False
    throttled: bool = False
    status: str  # "ok" | "error"
    timestamp: float


class RalLogsResponse(BaseModel):
    logs: List[RalRequestLog]
    total: int
    status_code: int = 200
    message: str = "ok"


# ── Health response ────────────────────────────────────────────────────────

class RalComponentHealth(BaseModel):
    name: str
    status: str  # "healthy" | "degraded" | "unavailable"
    detail: Optional[str] = None


class RalHealthResponse(BaseModel):
    overall: str  # "healthy" | "degraded" | "unhealthy"
    components: List[RalComponentHealth]
    active_requests: int = 0
    queue_size: int = 0
    cache_hit_ratio: float = 0.0
    status_code: int = 200
    message: str = "ok"


# ── Cache control ──────────────────────────────────────────────────────────

class RalCacheFlushResponse(BaseModel):
    deleted: int
    status_code: int = 200
    message: str = "cache flushed"

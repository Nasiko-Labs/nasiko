from __future__ import annotations

from enum import Enum
from typing import Any

from pydantic import BaseModel, Field, HttpUrl


class AgentTarget(BaseModel):
    agent_id: str
    public_path: str
    upstream_url: str
    target_revision: str
    source: str
    namespace: str
    updated_at: float


class AgentLimits(BaseModel):
    cache_ttl_seconds: int = Field(ge=0)
    max_concurrency: int = Field(ge=1)
    sustained_rps: float = Field(gt=0)
    burst_capacity: int = Field(ge=1)
    max_queue_depth: int = Field(ge=0)
    max_queue_wait_ms: int = Field(ge=0)
    cache_enabled: bool = True


class CacheState(str, Enum):
    hit = "HIT"
    miss = "MISS"
    bypass = "BYPASS"


class LimitState(str, Enum):
    normal = "normal"
    degraded = "degraded"
    circuit_open = "circuit-open"


class CacheDecision(BaseModel):
    cacheable: bool
    reason: str
    cache_key: str | None = None
    fingerprint: dict[str, Any] | None = None


class CachedResponse(BaseModel):
    status_code: int
    media_type: str | None = "application/json"
    body: bytes
    headers: dict[str, str] = Field(default_factory=dict)


class AcquireResult(BaseModel):
    acquired: bool
    queued: bool = False
    queue_wait_ms: int = 0
    retry_after_seconds: int = 1
    reason: str = "ok"
    degraded: bool = False


class CircuitDecision(BaseModel):
    allowed: bool
    state: str
    retry_after_seconds: int = 0


class AgentStats(BaseModel):
    agent_id: str
    active_requests: int
    queued_requests: int
    cache_hits: int
    cache_misses: int
    cache_bypasses: int
    singleflight_waiters: int
    upstream_requests: int
    upstream_errors: int
    queue_timeouts: int
    circuit_state: str
    p50_latency_ms: float
    p95_latency_ms: float
    p95_queue_wait_ms: float
    limits: AgentLimits


class GlobalStats(BaseModel):
    status: str
    redis_available: bool
    active_requests: int
    cache_hits: int
    cache_misses: int
    cache_bypasses: int
    upstream_requests: int
    upstream_errors: int
    queue_timeouts: int
    agents: list[AgentStats]

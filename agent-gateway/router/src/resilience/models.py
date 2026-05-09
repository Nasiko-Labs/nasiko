from __future__ import annotations

from dataclasses import dataclass, field
from typing import Literal

from pydantic import BaseModel, Field


class CacheConfig(BaseModel):
    ttl_seconds: float = Field(default=3600.0, ge=0)
    namespace: str = "nasiko-router-response-v1"
    enabled: bool = True
    redis_url: str | None = None
    semantic_enabled: bool = False
    semantic_threshold: float = Field(default=0.92, ge=0.0, le=1.0)


class CacheLookup(BaseModel):
    agent_id: str
    query: str
    auth_scope: str
    route: str | None = None
    has_files: bool = False


@dataclass(slots=True)
class CacheEntry:
    key: str
    agent_id: str
    response: str
    expires_at: float


class CacheStats(BaseModel):
    hits: int = 0
    misses: int = 0
    stores: int = 0
    evictions: int = 0
    errors: int = 0

    @property
    def hit_ratio(self) -> float:
        total = self.hits + self.misses
        if total == 0:
            return 0.0
        return self.hits / total


class LimitConfig(BaseModel):
    base_rps: float = Field(default=5.0, gt=0)
    min_rps: float = Field(default=0.25, gt=0)
    burst: int = Field(default=5, ge=1)
    max_queue_depth: int = Field(default=50, ge=0)
    max_queue_wait_seconds: float = Field(default=10.0, ge=0)
    target_latency_seconds: float = Field(default=2.0, gt=0)
    latency_weight: float = Field(default=0.5, ge=0)
    error_weight: float = Field(default=0.3, ge=0)
    queue_weight: float = Field(default=0.2, ge=0)


class LimitDecision(BaseModel):
    action: Literal["allow", "queue", "reject"]
    agent_id: str
    effective_rps: float
    retry_after_seconds: float = 0.0
    reason: str = ""


@dataclass(slots=True)
class AgentLimitState:
    tokens: float
    last_refill: float
    recent_latencies: list[float] = field(default_factory=list)
    recent_errors: list[int] = field(default_factory=list)


class ResilienceError(Exception):
    def __init__(self, message: str, status_code: int, retry_after_seconds: float = 0):
        super().__init__(message)
        self.status_code = status_code
        self.retry_after_seconds = retry_after_seconds


class RuntimeSnapshot(BaseModel):
    cache_hits: int
    cache_misses: int
    cache_hit_ratio: float
    cache_stores: int
    rate_limit_rejections: int
    agent_errors: int
    queue_depths: dict[str, int]
    current_limits: dict[str, float]
    agent_latency_count: dict[str, int]
    agent_latency_average_seconds: dict[str, float]
    queue_wait_count: dict[str, int]
    queue_wait_average_seconds: dict[str, float]

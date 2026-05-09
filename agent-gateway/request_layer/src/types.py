"""Pydantic models shared across stages."""
from datetime import datetime
from typing import Any

from pydantic import BaseModel, ConfigDict, Field


class CacheEntry(BaseModel):
    """A response stored in the L1 / L2 caches."""

    status_code: int
    headers: dict[str, str]
    body: str
    cost_usd: float = 0.0
    latency_ms: float = 0.0
    cached_at: datetime
    matched_query: str | None = None


class RoutingDecision(BaseModel):
    """A cached router decision used by L3.

    The router service in Nasiko runs a LangChain pipeline that consumes the
    user query and the set of registered AgentCards to pick an agent.
    Request layer caches the embedding-of-the-query → ``agent_url`` mapping so the
    LangChain call can be skipped on intent matches.
    """

    agent_name: str
    agent_url: str
    confidence: float
    cached_at: datetime
    last_validated_at: datetime
    matched_query: str | None = None


class Policy(BaseModel):
    """Per-agent cache and rate-limit policy."""

    cache_ttl_seconds: int
    semantic_threshold: float
    rps_limit: int
    cost_cap_usd_per_min: float
    notes: str | None = None


class AgentManifest(BaseModel):
    """A normalized view of any agent platform's capability manifest.

    The :class:`~request_layer.src.agentcard.NasikoAdapter` populates this from
    AgentCard.json files served by ``/api/v1/registry``.
    """

    name: str
    endpoint_url: str
    capabilities: set[str] = Field(default_factory=set)
    tags: set[str] = Field(default_factory=set)
    examples: list[str] = Field(default_factory=list)
    model: str | None = None
    raw: dict[str, Any] = Field(default_factory=dict)


class CacheEvent(BaseModel):
    """Event emitted when a cache decision is made (used for SSE stream)."""

    model_config = ConfigDict(extra="allow")

    timestamp: datetime
    agent: str
    layer: str  # "L1", "L2", "L3", "miss"
    similarity: float | None = None
    matched_query: str | None = None
    savings_usd: float = 0.0
    savings_ms: float = 0.0
    router_skipped: bool = False


class RecommendationItem(BaseModel):
    """A self-tuning recommendation surfaced on the admin API."""

    id: str
    agent: str
    field: str
    current_value: float | int
    suggested_value: float | int
    reason: str
    generated_at: datetime

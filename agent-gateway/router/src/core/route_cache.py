"""
Routing decision cache backed by Redis.

Cache key: SHA-256 of (normalized query + sorted agent names).
Cache value: the selected agent name string.

This avoids repeating the LLM call + embedding search for identical queries
against the same set of agents.
"""

import hashlib
import logging
from typing import List, Optional

import redis

from router.src.config import settings

logger = logging.getLogger(__name__)

_CACHE_NS = "router:route"
_STATS_KEY = "router:route:stats"


class RouteCache:
    def __init__(self):
        self._client: Optional[redis.Redis] = None
        if settings.ROUTING_CACHE_ENABLED and settings.REDIS_URL:
            try:
                self._client = redis.from_url(settings.REDIS_URL, decode_responses=True)
                self._client.ping()
                logger.info("Router route cache connected to Redis")
            except Exception as exc:
                logger.warning("Route cache unavailable, routing will not be cached: %s", exc)
                self._client = None

    @property
    def enabled(self) -> bool:
        return self._client is not None and settings.ROUTING_CACHE_TTL > 0

    def _make_key(self, query: str, agent_names: List[str]) -> str:
        normalized = query.strip().lower()
        agents_part = "|".join(sorted(agent_names))
        digest = hashlib.sha256(f"{normalized}::{agents_part}".encode()).hexdigest()
        return f"{_CACHE_NS}:{digest}"

    def get(self, query: str, agent_names: List[str]) -> Optional[str]:
        if not self.enabled:
            return None
        try:
            key = self._make_key(query, agent_names)
            value = self._client.get(key)
            if value:
                self._client.hincrby(_STATS_KEY, "hits", 1)
                logger.info("Route cache HIT for query=%.60r -> agent=%s", query, value)
            else:
                self._client.hincrby(_STATS_KEY, "misses", 1)
            return value
        except Exception as exc:
            logger.warning("Route cache get failed: %s", exc)
            return None

    def set(self, query: str, agent_names: List[str], agent_name: str) -> None:
        if not self.enabled:
            return
        try:
            key = self._make_key(query, agent_names)
            self._client.setex(key, settings.ROUTING_CACHE_TTL, agent_name)
            self._client.hincrby(_STATS_KEY, "stored", 1)
            logger.info(
                "Route cache SET for query=%.60r -> agent=%s (ttl=%ds)",
                query, agent_name, settings.ROUTING_CACHE_TTL,
            )
        except Exception as exc:
            logger.warning("Route cache set failed: %s", exc)

    def stats(self) -> dict:
        if not self.enabled:
            return {"enabled": False}
        try:
            raw = self._client.hgetall(_STATS_KEY)
            hits = int(raw.get("hits", 0))
            misses = int(raw.get("misses", 0))
            stored = int(raw.get("stored", 0))
            total = hits + misses
            cached_keys = len(self._client.keys(f"{_CACHE_NS}:*"))
            return {
                "enabled": True,
                "hits": hits,
                "misses": misses,
                "stored": stored,
                "total_lookups": total,
                "hit_rate": round(hits / total, 3) if total else 0.0,
                "cached_decisions": cached_keys,
                "ttl_seconds": settings.ROUTING_CACHE_TTL,
            }
        except Exception as exc:
            logger.warning("Route cache stats failed: %s", exc)
            return {"enabled": True, "error": str(exc)}

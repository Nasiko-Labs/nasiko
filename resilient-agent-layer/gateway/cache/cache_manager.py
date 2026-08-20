import json
import time
import asyncio
from typing import Any, Optional
from collections import OrderedDict
import redis.asyncio as aioredis

from gateway.cache.key_builder import build_cache_key
from gateway.config import REDIS_URL, DEFAULT_CACHE_TTL


# ─── In-Memory LRU Fallback ───────────────────────────────────────────────────

class LRUCache:
    """Simple in-memory LRU cache used as fallback when Redis is unavailable."""

    def __init__(self, max_size: int = 1000):
        self._cache: OrderedDict[str, tuple[Any, float]] = OrderedDict()
        self._max_size = max_size

    def get(self, key: str) -> Optional[Any]:
        if key not in self._cache:
            return None
        value, expires_at = self._cache[key]
        if time.monotonic() > expires_at:
            del self._cache[key]
            return None
        self._cache.move_to_end(key)
        return value

    def set(self, key: str, value: Any, ttl: int = DEFAULT_CACHE_TTL):
        if key in self._cache:
            self._cache.move_to_end(key)
        self._cache[key] = (value, time.monotonic() + ttl)
        if len(self._cache) > self._max_size:
            self._cache.popitem(last=False)

    def delete_prefix(self, prefix: str):
        keys = [k for k in self._cache if k.startswith(prefix)]
        for k in keys:
            del self._cache[k]

    def flush(self):
        self._cache.clear()

    def size(self) -> int:
        return len(self._cache)

    def all_keys(self) -> list[str]:
        return list(self._cache.keys())


# ─── Main Cache Manager ───────────────────────────────────────────────────────

class CacheManager:
    """
    Redis-backed response cache with in-memory LRU fallback.
    Tracks hit/miss counts per agent for monitoring.
    """

    def __init__(self):
        self._redis: Optional[aioredis.Redis] = None
        self._lru = LRUCache()
        self._use_redis = True

        # Stats counters
        self._hits: dict[str, int] = {}
        self._misses: dict[str, int] = {}
        self._total_latency_saved_ms: dict[str, float] = {}

    async def connect(self):
        try:
            self._redis = await aioredis.from_url(REDIS_URL, decode_responses=True)
            await self._redis.ping()
            self._use_redis = True
        except Exception as e:
            print(f"[Cache] Redis unavailable, falling back to LRU: {e}")
            self._use_redis = False

    async def get(self, agent_id: str, payload: Any) -> Optional[dict]:
        key = build_cache_key(agent_id, payload)
        value = None

        if self._use_redis and self._redis:
            try:
                raw = await self._redis.get(key)
                if raw:
                    value = json.loads(raw)
            except Exception:
                value = self._lru.get(key)
        else:
            value = self._lru.get(key)

        if value is not None:
            self._hits[agent_id] = self._hits.get(agent_id, 0) + 1
        else:
            self._misses[agent_id] = self._misses.get(agent_id, 0) + 1

        return value

    async def set(self, agent_id: str, payload: Any, response: dict, ttl: int = DEFAULT_CACHE_TTL):
        key = build_cache_key(agent_id, payload)
        serialized = json.dumps(response)

        if self._use_redis and self._redis:
            try:
                await self._redis.setex(key, ttl, serialized)
            except Exception:
                pass
        self._lru.set(key, response, ttl)

    async def flush_all(self):
        if self._use_redis and self._redis:
            try:
                keys = await self._redis.keys("cache:*")
                if keys:
                    await self._redis.delete(*keys)
            except Exception:
                pass
        self._lru.flush()
        self._hits.clear()
        self._misses.clear()

    async def flush_agent(self, agent_id: str):
        prefix = f"cache:{agent_id}:"
        if self._use_redis and self._redis:
            try:
                keys = await self._redis.keys(f"{prefix}*")
                if keys:
                    await self._redis.delete(*keys)
            except Exception:
                pass
        self._lru.delete_prefix(prefix)
        self._hits.pop(agent_id, None)
        self._misses.pop(agent_id, None)

    def get_stats(self, agent_id: Optional[str] = None) -> dict:
        if agent_id:
            hits = self._hits.get(agent_id, 0)
            misses = self._misses.get(agent_id, 0)
            total = hits + misses
            return {
                "agent_id": agent_id,
                "hits": hits,
                "misses": misses,
                "total": total,
                "hit_rate": round(hits / total * 100, 2) if total else 0.0,
            }

        # Global
        total_hits = sum(self._hits.values())
        total_misses = sum(self._misses.values())
        total = total_hits + total_misses
        return {
            "hits": total_hits,
            "misses": total_misses,
            "total": total,
            "hit_rate": round(total_hits / total * 100, 2) if total else 0.0,
            "backend": "redis" if self._use_redis else "lru",
            "lru_size": self._lru.size(),
            "per_agent": {
                aid: self.get_stats(aid)
                for aid in set(list(self._hits.keys()) + list(self._misses.keys()))
            },
        }


# Singleton instance
cache_manager = CacheManager()

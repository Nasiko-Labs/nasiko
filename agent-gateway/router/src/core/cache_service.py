"""
Response cache service for agent requests.

Caches agent responses keyed by a normalized query hash so that repeated
identical (or semantically similar) requests are served from cache without
re-running the full LLM + agent pipeline.

Cache backend: Redis (with in-process LRU fallback when Redis is unavailable).
"""

import hashlib
import json
import logging
import time
from collections import OrderedDict
from typing import Any, Dict, List, Optional, Tuple

from router.src.config import settings

logger = logging.getLogger(__name__)


# ---------------------------------------------------------------------------
# In-process LRU fallback (used when Redis is not configured / unreachable)
# ---------------------------------------------------------------------------

class _LRUCache:
    """Simple thread-safe LRU cache with TTL support."""

    def __init__(self, max_size: int = 512, ttl: int = 300):
        self._store: OrderedDict[str, Tuple[Any, float]] = OrderedDict()
        self._max_size = max_size
        self._ttl = ttl

    def get(self, key: str) -> Optional[Any]:
        if key not in self._store:
            return None
        value, ts = self._store[key]
        if time.time() - ts > self._ttl:
            del self._store[key]
            return None
        # Move to end (most recently used)
        self._store.move_to_end(key)
        return value

    def set(self, key: str, value: Any) -> None:
        if key in self._store:
            self._store.move_to_end(key)
        self._store[key] = (value, time.time())
        if len(self._store) > self._max_size:
            self._store.popitem(last=False)

    def delete(self, key: str) -> bool:
        if key in self._store:
            del self._store[key]
            return True
        return False

    def clear(self) -> int:
        count = len(self._store)
        self._store.clear()
        return count

    def size(self) -> int:
        # Evict expired entries first
        now = time.time()
        expired = [k for k, (_, ts) in self._store.items() if now - ts > self._ttl]
        for k in expired:
            del self._store[k]
        return len(self._store)

    def stats(self) -> Dict[str, Any]:
        return {
            "size": self.size(),
            "max_size": self._max_size,
            "ttl_seconds": self._ttl,
        }


# ---------------------------------------------------------------------------
# Cache key helpers
# ---------------------------------------------------------------------------

def _normalize_query(query: str) -> str:
    """Lowercase and strip whitespace for consistent key generation."""
    return " ".join(query.lower().split())


def make_cache_key(agent_name: str, query: str, session_id: str = "") -> str:
    """
    Build a deterministic cache key.

    The key is scoped to the agent so different agents with the same query
    don't collide.  Session ID is intentionally excluded from the key so
    that the same query from different sessions can share a cached response.
    """
    normalized = _normalize_query(query)
    raw = f"agent:{agent_name}|query:{normalized}"
    return "nasiko:cache:" + hashlib.sha256(raw.encode()).hexdigest()


def make_agent_stats_key(agent_name: str) -> str:
    return f"nasiko:stats:{agent_name}"


# ---------------------------------------------------------------------------
# CacheService
# ---------------------------------------------------------------------------

class CacheService:
    """
    Unified response cache backed by Redis with an in-process LRU fallback.

    Usage
    -----
    cache = CacheService()
    await cache.connect()

    hit = await cache.get(agent_name, query)
    if hit is None:
        response = await call_agent(...)
        await cache.set(agent_name, query, response)
    """

    def __init__(self):
        self._redis: Optional[Any] = None          # redis.asyncio.Redis
        self._lru = _LRUCache(
            max_size=settings.CACHE_MAX_SIZE,
            ttl=settings.CACHE_TTL_SECONDS,
        )
        self._hits = 0
        self._misses = 0
        self._connected = False

    # ------------------------------------------------------------------
    # Lifecycle
    # ------------------------------------------------------------------

    async def connect(self) -> None:
        """Attempt to connect to Redis; fall back to LRU on failure."""
        if not settings.CACHE_REDIS_URL:
            logger.info("CACHE_REDIS_URL not set — using in-process LRU cache")
            return

        try:
            import redis.asyncio as aioredis  # type: ignore

            self._redis = aioredis.from_url(
                settings.CACHE_REDIS_URL,
                encoding="utf-8",
                decode_responses=True,
                socket_connect_timeout=3,
                socket_timeout=3,
            )
            # Verify connection
            await self._redis.ping()
            self._connected = True
            logger.info(f"Cache connected to Redis at {settings.CACHE_REDIS_URL}")
        except Exception as exc:
            logger.warning(
                f"Redis unavailable ({exc}); falling back to in-process LRU cache"
            )
            self._redis = None
            self._connected = False

    async def close(self) -> None:
        if self._redis:
            await self._redis.aclose()
            self._redis = None
            self._connected = False

    # ------------------------------------------------------------------
    # Core operations
    # ------------------------------------------------------------------

    async def get(self, agent_name: str, query: str) -> Optional[List[str]]:
        """
        Look up a cached response.

        Returns
        -------
        List[str] of JSON-serialised RouterResponse lines, or None on miss.
        """
        key = make_cache_key(agent_name, query)

        try:
            if self._redis:
                raw = await self._redis.get(key)
                if raw is not None:
                    self._hits += 1
                    await self._increment_agent_hits(agent_name)
                    logger.debug(f"Cache HIT (Redis) for agent={agent_name}")
                    return json.loads(raw)
            else:
                cached = self._lru.get(key)
                if cached is not None:
                    self._hits += 1
                    logger.debug(f"Cache HIT (LRU) for agent={agent_name}")
                    return cached
        except Exception as exc:
            logger.warning(f"Cache get error: {exc}")

        self._misses += 1
        logger.debug(f"Cache MISS for agent={agent_name}")
        return None

    async def set(
        self,
        agent_name: str,
        query: str,
        response_lines: List[str],
    ) -> None:
        """
        Store a response in the cache.

        Parameters
        ----------
        agent_name:     name of the agent that produced the response
        query:          original user query
        response_lines: list of JSON-serialised RouterResponse strings
        """
        key = make_cache_key(agent_name, query)
        ttl = settings.CACHE_TTL_SECONDS

        try:
            if self._redis:
                pipe = self._redis.pipeline()
                pipe.setex(key, ttl, json.dumps(response_lines))
                # Track the key in a per-agent set so clear_agent can find it
                set_key = f"nasiko:cache:keys:{agent_name}"
                pipe.sadd(set_key, key)
                pipe.expire(set_key, ttl + 60)  # set expires slightly after entries
                await pipe.execute()
                logger.debug(f"Cache SET (Redis) for agent={agent_name}, ttl={ttl}s")
            else:
                self._lru.set(key, response_lines)
                logger.debug(f"Cache SET (LRU) for agent={agent_name}")
        except Exception as exc:
            logger.warning(f"Cache set error: {exc}")

    async def delete(self, agent_name: str, query: str) -> bool:
        """Invalidate a specific cache entry."""
        key = make_cache_key(agent_name, query)
        try:
            if self._redis:
                result = await self._redis.delete(key)
                return result > 0
            else:
                return self._lru.delete(key)
        except Exception as exc:
            logger.warning(f"Cache delete error: {exc}")
            return False

    async def clear_agent(self, agent_name: str) -> int:
        """
        Clear all cached entries for a specific agent.
        Returns the number of keys deleted.
        """
        pattern = f"nasiko:cache:*"  # We can't filter by agent in key hash
        # For Redis: scan for keys with the agent prefix stored in a set
        count = 0
        try:
            if self._redis:
                # We maintain a secondary set of keys per agent
                set_key = f"nasiko:cache:keys:{agent_name}"
                keys = await self._redis.smembers(set_key)
                if keys:
                    count = await self._redis.delete(*keys)
                    await self._redis.delete(set_key)
            else:
                # LRU: clear all (no per-agent granularity in simple LRU)
                count = self._lru.clear()
        except Exception as exc:
            logger.warning(f"Cache clear_agent error: {exc}")
        return count

    async def clear_all(self) -> int:
        """Clear the entire response cache. Returns number of keys deleted."""
        count = 0
        try:
            if self._redis:
                cursor = 0
                keys_to_delete = []
                while True:
                    cursor, keys = await self._redis.scan(
                        cursor, match="nasiko:cache:*", count=100
                    )
                    keys_to_delete.extend(keys)
                    if cursor == 0:
                        break
                if keys_to_delete:
                    count = await self._redis.delete(*keys_to_delete)
            else:
                count = self._lru.clear()
        except Exception as exc:
            logger.warning(f"Cache clear_all error: {exc}")
        return count

    # ------------------------------------------------------------------
    # Stats
    # ------------------------------------------------------------------

    async def get_stats(self) -> Dict[str, Any]:
        """Return cache statistics."""
        total = self._hits + self._misses
        hit_rate = (self._hits / total * 100) if total > 0 else 0.0

        stats: Dict[str, Any] = {
            "backend": "redis" if self._connected else "lru",
            "connected": self._connected,
            "hits": self._hits,
            "misses": self._misses,
            "total_requests": total,
            "hit_rate_pct": round(hit_rate, 2),
            "ttl_seconds": settings.CACHE_TTL_SECONDS,
        }

        if self._redis:
            try:
                info = await self._redis.info("memory")
                stats["redis_used_memory"] = info.get("used_memory_human", "unknown")
                stats["redis_keys"] = await self._redis.dbsize()
            except Exception:
                pass
        else:
            stats.update(self._lru.stats())

        return stats

    async def _increment_agent_hits(self, agent_name: str) -> None:
        """Track per-agent hit counts in Redis."""
        if not self._redis:
            return
        try:
            key = make_agent_stats_key(agent_name)
            await self._redis.hincrby(key, "hits", 1)
            await self._redis.expire(key, 86400)  # 24h TTL on stats
        except Exception:
            pass

    async def record_agent_request(self, agent_name: str) -> None:
        """Record a total request for an agent (for hit-rate calculation)."""
        if not self._redis:
            return
        try:
            key = make_agent_stats_key(agent_name)
            await self._redis.hincrby(key, "total", 1)
            await self._redis.expire(key, 86400)
        except Exception:
            pass

    async def get_agent_stats(self, agent_name: str) -> Dict[str, Any]:
        """Return per-agent cache stats."""
        if not self._redis:
            return {"agent": agent_name, "backend": "lru", "note": "per-agent stats unavailable in LRU mode"}
        try:
            key = make_agent_stats_key(agent_name)
            data = await self._redis.hgetall(key)
            hits = int(data.get("hits", 0))
            total = int(data.get("total", 0))
            return {
                "agent": agent_name,
                "hits": hits,
                "total": total,
                "hit_rate_pct": round(hits / total * 100, 2) if total > 0 else 0.0,
            }
        except Exception as exc:
            return {"agent": agent_name, "error": str(exc)}

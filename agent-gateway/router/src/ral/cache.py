"""
RAL Response Cache
==================
Async Redis-backed response cache with SHA-256 keying.

Key design choices
------------------
* Cache key excludes `session_id` → global semantic deduplication.
  Identical queries from different users share one cache entry, maximising
  hit-rate and minimising redundant LLM calls.
* Serialisation: responses are stored as UTF-8 JSON strings.
* TTL is configurable via RAL_CACHE_TTL (default 300 s).
* Hit / miss counters are maintained as Redis INCR keys so the metrics
  subsystem can read them cheaply without needing a local accumulator.
"""

from __future__ import annotations

import hashlib
import json
import logging
import time
from typing import Optional, Any, Dict

import redis.asyncio as aioredis

from .config import ral_settings

logger = logging.getLogger(__name__)

# Redis key templates
_KEY_CACHE   = "{pfx}:cache:{key}"
_KEY_HITS    = "{pfx}:stats:cache_hits"
_KEY_MISSES  = "{pfx}:stats:cache_misses"
_KEY_LAT_SUM = "{pfx}:stats:cache_lat_ms_sum"
_KEY_LAT_CNT = "{pfx}:stats:cache_lat_ms_count"


def _build_cache_key(
    query: str,
    route: Optional[str],
    model: str,
    provider: str,
) -> str:
    """
    Derive a deterministic, content-addressed cache key.

    Parameters are normalised (lowercased, stripped) before hashing so that
    trivial whitespace differences do not produce different keys.
    """
    canonical = json.dumps(
        {
            "q": query.strip().lower(),
            "route": (route or "").strip().lower(),
            "model": model.strip().lower(),
            "provider": provider.strip().lower(),
        },
        sort_keys=True,
        separators=(",", ":"),
    )
    return hashlib.sha256(canonical.encode()).hexdigest()


class ResponseCache:
    """
    Async LRU-style cache backed by Redis.

    Usage
    -----
    ```python
    cache = ResponseCache()
    await cache.connect()

    key = cache.make_key(query, route, model, provider)
    hit = await cache.get(key)
    if hit is None:
        result = await compute_expensive_response()
        await cache.set(key, result)
    ```
    """

    def __init__(self) -> None:
        self._redis: Optional[aioredis.Redis] = None
        self._pfx = ral_settings.RAL_REDIS_PREFIX
        self._ttl = ral_settings.RAL_CACHE_TTL

    # ------------------------------------------------------------------
    # Lifecycle
    # ------------------------------------------------------------------

    async def connect(self) -> None:
        """Create the async Redis connection pool (call once at startup)."""
        if self._redis is None:
            self._redis = aioredis.from_url(
                ral_settings.redis_dsn,
                encoding="utf-8",
                decode_responses=True,
                max_connections=20,
            )
            logger.info("RAL cache connected to Redis at %s", ral_settings.redis_dsn)

    async def close(self) -> None:
        if self._redis:
            await self._redis.aclose()
            self._redis = None

    # ------------------------------------------------------------------
    # Public API
    # ------------------------------------------------------------------

    def make_key(
        self,
        query: str,
        route: Optional[str],
        model: str,
        provider: str,
    ) -> str:
        """Return the hex cache key for the given request parameters."""
        return _build_cache_key(query, route, model, provider)

    async def get(self, key: str) -> Optional[str]:
        """
        Retrieve a cached response.

        Returns the raw JSON string on a hit, or *None* on a miss.
        Also updates Redis hit/miss counters and latency stats.
        """
        if self._redis is None:
            return None

        t0 = time.monotonic()
        try:
            value: Optional[str] = await self._redis.get(
                _KEY_CACHE.format(pfx=self._pfx, key=key)
            )
        except Exception as exc:
            logger.warning("RAL cache GET error: %s", exc)
            return None

        elapsed_ms = (time.monotonic() - t0) * 1000.0

        try:
            if value is not None:
                await self._redis.incr(_KEY_HITS.format(pfx=self._pfx))
                await self._redis.incrbyfloat(
                    _KEY_LAT_SUM.format(pfx=self._pfx), elapsed_ms
                )
                await self._redis.incr(_KEY_LAT_CNT.format(pfx=self._pfx))
                logger.debug("RAL cache HIT  key=%s latency=%.1fms", key[:16], elapsed_ms)
            else:
                await self._redis.incr(_KEY_MISSES.format(pfx=self._pfx))
                logger.debug("RAL cache MISS key=%s", key[:16])
        except Exception:
            pass  # Never let counter updates break the hot path

        return value

    async def set(self, key: str, value: str, ttl: Optional[int] = None) -> None:
        """Store a response in the cache with an optional TTL override."""
        if self._redis is None:
            return
        try:
            await self._redis.setex(
                _KEY_CACHE.format(pfx=self._pfx, key=key),
                ttl if ttl is not None else self._ttl,
                value,
            )
            logger.debug("RAL cache SET  key=%s ttl=%ds", key[:16], ttl or self._ttl)
        except Exception as exc:
            logger.warning("RAL cache SET error: %s", exc)

    async def invalidate(self, key: str) -> bool:
        """Delete a single cache entry. Returns True if it existed."""
        if self._redis is None:
            return False
        try:
            deleted = await self._redis.delete(
                _KEY_CACHE.format(pfx=self._pfx, key=key)
            )
            return bool(deleted)
        except Exception as exc:
            logger.warning("RAL cache invalidate error: %s", exc)
            return False

    async def flush_all(self) -> int:
        """Remove all RAL cache entries. Returns the count deleted."""
        if self._redis is None:
            return 0
        pattern = _KEY_CACHE.format(pfx=self._pfx, key="*")
        try:
            keys = await self._redis.keys(pattern)
            if keys:
                deleted = await self._redis.delete(*keys)
                logger.info("RAL cache flushed %d entries", deleted)
                return deleted
        except Exception as exc:
            logger.warning("RAL cache flush error: %s", exc)
        return 0

    async def get_stats(self) -> Dict[str, Any]:
        """Return a snapshot of cache hit/miss/latency counters."""
        if self._redis is None:
            return {"hits": 0, "misses": 0, "avg_latency_ms": 0.0, "hit_ratio": 0.0}
        try:
            hits   = int(await self._redis.get(_KEY_HITS.format(pfx=self._pfx))   or 0)
            misses = int(await self._redis.get(_KEY_MISSES.format(pfx=self._pfx)) or 0)
            lat_sum = float(await self._redis.get(_KEY_LAT_SUM.format(pfx=self._pfx)) or 0)
            lat_cnt = int(await self._redis.get(_KEY_LAT_CNT.format(pfx=self._pfx)) or 0)
            total = hits + misses
            return {
                "hits": hits,
                "misses": misses,
                "hit_ratio": round(hits / total, 4) if total else 0.0,
                "avg_latency_ms": round(lat_sum / lat_cnt, 2) if lat_cnt else 0.0,
            }
        except Exception as exc:
            logger.warning("RAL cache stats error: %s", exc)
            return {"hits": 0, "misses": 0, "avg_latency_ms": 0.0, "hit_ratio": 0.0}

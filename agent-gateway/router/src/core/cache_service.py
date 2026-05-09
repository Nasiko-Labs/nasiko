"""Redis-backed response cache for the router."""

import hashlib
import logging
from typing import Optional

from router.src.config import settings

logger = logging.getLogger(__name__)

_redis_client = None


def _get_redis():
    global _redis_client
    if _redis_client is None:
        import redis.asyncio as aioredis

        _redis_client = aioredis.from_url(settings.REDIS_URL, decode_responses=True)
    return _redis_client


def _cache_key(query: str) -> str:
    normalized = query.strip().lower()
    return f"cache:response:{hashlib.sha256(normalized.encode()).hexdigest()}"


class CacheService:
    async def get(self, query: str, agent_name: str = "router") -> Optional[str]:
        try:
            r = _get_redis()
            key = _cache_key(query)
            value = await r.get(key)
            stat = "hits" if value else "misses"
            await r.hincrby("cache:stats", stat, 1)
            await r.hincrby("cache:stats", "total_requests", 1)
            return value
        except Exception as e:
            logger.warning(f"Cache get failed: {e}")
            return None

    async def set(self, query: str, agent_name: str, value: str) -> None:
        try:
            r = _get_redis()
            key = _cache_key(query)
            await r.setex(key, settings.CACHE_TTL_SECONDS, value)
            await r.sadd(f"cache:agent:{agent_name}:keys", key)
            await r.hincrby("cache:stats", "total_sets", 1)
        except Exception as e:
            logger.warning(f"Cache set failed: {e}")

    async def flush_agent(self, agent_name: str) -> int:
        try:
            r = _get_redis()
            keys = await r.smembers(f"cache:agent:{agent_name}:keys")
            if keys:
                await r.delete(*keys)
                await r.delete(f"cache:agent:{agent_name}:keys")
            return len(keys)
        except Exception as e:
            logger.warning(f"Cache flush_agent failed: {e}")
            return 0

    async def flush_all(self) -> int:
        try:
            r = _get_redis()
            keys = await r.keys("cache:response:*")
            if keys:
                await r.delete(*keys)
            return len(keys)
        except Exception as e:
            logger.warning(f"Cache flush_all failed: {e}")
            return 0

    async def stats(self) -> dict:
        try:
            r = _get_redis()
            raw = await r.hgetall("cache:stats")
            hits = int(raw.get("hits", 0))
            misses = int(raw.get("misses", 0))
            total = int(raw.get("total_requests", 0))
            live_keys = len(await r.keys("cache:response:*"))
            return {
                "hits": hits,
                "misses": misses,
                "total_requests": total,
                "hit_rate_pct": round(hits / total * 100, 1) if total else 0.0,
                "total_sets": int(raw.get("total_sets", 0)),
                "live_keys": live_keys,
                "ttl_seconds": settings.CACHE_TTL_SECONDS,
            }
        except Exception as e:
            logger.warning(f"Cache stats failed: {e}")
            return {}

    async def agent_stats(self, agent_name: str) -> dict:
        try:
            r = _get_redis()
            count = await r.scard(f"cache:agent:{agent_name}:keys")
            return {"agent_name": agent_name, "live_keys": count}
        except Exception as e:
            logger.warning(f"Cache agent_stats failed: {e}")
            return {"agent_name": agent_name, "live_keys": 0}

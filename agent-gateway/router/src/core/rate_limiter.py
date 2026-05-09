"""Sliding-window rate limiter with async queue using Redis sorted sets."""

import asyncio
import logging
import time
from typing import Optional, Tuple

from router.src.config import settings

logger = logging.getLogger(__name__)

_redis_client = None


def _get_redis():
    global _redis_client
    if _redis_client is None:
        import redis.asyncio as aioredis

        _redis_client = aioredis.from_url(settings.REDIS_URL, decode_responses=True)
    return _redis_client


class RateLimiter:
    async def _get_config(self, agent_name: str) -> Tuple[int, int]:
        try:
            r = _get_redis()
            raw = await r.hgetall(f"ratelimit:config:{agent_name}")
            limit = int(raw.get("limit", settings.RATE_LIMIT_REQUESTS))
            window = int(raw.get("window_seconds", settings.RATE_LIMIT_WINDOW_SECONDS))
            return limit, window
        except Exception:
            return settings.RATE_LIMIT_REQUESTS, settings.RATE_LIMIT_WINDOW_SECONDS

    async def _check_and_record(self, agent_name: str) -> bool:
        try:
            r = _get_redis()
            limit, window = await self._get_config(agent_name)
            key = f"ratelimit:window:{agent_name}"
            now = time.time()
            cutoff = now - window
            async with r.pipeline() as pipe:
                await pipe.zremrangebyscore(key, "-inf", cutoff)
                await pipe.zcard(key)
                await pipe.zadd(key, {str(now): now})
                await pipe.expire(key, window + 1)
                results = await pipe.execute()
            count_before = results[1]
            if count_before >= limit:
                await r.zrem(key, str(now))
                return False
            await r.hincrby(f"ratelimit:stats:{agent_name}", "allowed", 1)
            return True
        except Exception as e:
            logger.warning(f"Rate limiter error: {e}")
            return True  # fail-open

    async def acquire(self, agent_name: str) -> Tuple[bool, str]:
        if await self._check_and_record(agent_name):
            return True, "allowed"
        timeout = settings.RATE_LIMIT_QUEUE_TIMEOUT_SECONDS
        deadline = time.time() + timeout
        queued = False
        while time.time() < deadline:
            await asyncio.sleep(0.5)
            if await self._check_and_record(agent_name):
                disposition = "queued" if queued else "allowed"
                return True, disposition
            queued = True
        try:
            r = _get_redis()
            await r.hincrby(f"ratelimit:stats:{agent_name}", "rejected", 1)
        except Exception:
            pass
        return False, "rejected"

    async def set_config(self, agent_name: str, limit: int, window_seconds: int) -> None:
        try:
            r = _get_redis()
            await r.hset(
                f"ratelimit:config:{agent_name}",
                mapping={"limit": limit, "window_seconds": window_seconds},
            )
        except Exception as e:
            logger.warning(f"Rate limiter set_config failed: {e}")

    async def stats(self, agent_name: Optional[str] = None) -> dict:
        try:
            r = _get_redis()
            if agent_name:
                raw = await r.hgetall(f"ratelimit:stats:{agent_name}")
                limit, window = await self._get_config(agent_name)
                count = await r.zcard(f"ratelimit:window:{agent_name}")
                return {
                    agent_name: {
                        "allowed": int(raw.get("allowed", 0)),
                        "rejected": int(raw.get("rejected", 0)),
                        "current_window_count": count,
                        "limit": limit,
                        "window_seconds": window,
                    }
                }
            keys = await r.keys("ratelimit:stats:*")
            result = {}
            for k in keys:
                name = k.split("ratelimit:stats:")[1]
                raw = await r.hgetall(k)
                limit, window = await self._get_config(name)
                count = await r.zcard(f"ratelimit:window:{name}")
                result[name] = {
                    "allowed": int(raw.get("allowed", 0)),
                    "rejected": int(raw.get("rejected", 0)),
                    "current_window_count": count,
                    "limit": limit,
                    "window_seconds": window,
                }
            return result
        except Exception as e:
            logger.warning(f"Rate limiter stats failed: {e}")
            return {}

    async def reset_stats(self, agent_name: str) -> None:
        try:
            r = _get_redis()
            await r.delete(f"ratelimit:stats:{agent_name}")
        except Exception as e:
            logger.warning(f"Rate limiter reset_stats failed: {e}")

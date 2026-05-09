"""
Per-agent health tracking stored in Redis.

Health score = success_rate * 0.5 - latency_penalty * 0.3 - queue_penalty * 0.2

Scores range from ~-0.5 (very unhealthy) to 1.0 (perfect).
The HEALTH_SCORE_THRESHOLD setting controls when smart fallback kicks in.
"""

import logging
import time
from typing import Any, Dict

import redis.asyncio as aioredis

logger = logging.getLogger(__name__)

_HEALTH_KEY_PREFIX = "agent:health:"
_HEALTH_TTL = 86400  # 24h — auto-expire stale agents


class AgentHealthTracker:
    """Tracks per-agent request success rate, latency, and derives a health score."""

    def __init__(self, redis: aioredis.Redis, max_queue_size: int = 50):
        self.redis = redis
        self.max_queue_size = max_queue_size

    def _key(self, agent_name: str) -> str:
        return f"{_HEALTH_KEY_PREFIX}{agent_name}"

    # -------------------------------------------------------------------------
    # Record outcome
    # -------------------------------------------------------------------------

    async def record(self, agent_name: str, success: bool, latency_ms: float) -> None:
        """Called after every agent request to update health stats."""
        key = self._key(agent_name)
        try:
            pipe = self.redis.pipeline()
            pipe.hincrby(key, "total_requests", 1)
            pipe.hincrbyfloat(key, "total_latency_ms", latency_ms)
            if success:
                pipe.hincrby(key, "success_count", 1)
            else:
                pipe.hincrby(key, "failure_count", 1)
            pipe.hset(key, "last_updated", time.time())
            pipe.expire(key, _HEALTH_TTL)
            await pipe.execute()
        except Exception as e:
            logger.warning(f"Health record error for '{agent_name}': {e}")

    # -------------------------------------------------------------------------
    # Score computation
    # -------------------------------------------------------------------------

    @staticmethod
    def _compute_score(
        total: int,
        success: int,
        total_latency_ms: float,
        queue_depth: int,
        max_queue: int,
    ) -> float:
        if total == 0:
            return 1.0
        success_rate = success / total
        avg_latency = total_latency_ms / total
        latency_penalty = min(avg_latency / 10_000.0, 1.0)  # 10 s → full penalty
        queue_penalty = min(queue_depth / max(max_queue, 1), 1.0)
        score = success_rate * 0.5 - latency_penalty * 0.3 - queue_penalty * 0.2
        return round(score, 4)

    async def get_score(self, agent_name: str, queue_depth: int = 0) -> float:
        key = self._key(agent_name)
        try:
            raw = await self.redis.hgetall(key)
            if not raw:
                return 1.0  # no data yet → assume healthy
            total = int(raw.get("total_requests", 0))
            success = int(raw.get("success_count", 0))
            total_latency = float(raw.get("total_latency_ms", 0.0))
            return self._compute_score(
                total, success, total_latency, queue_depth, self.max_queue_size
            )
        except Exception as e:
            logger.warning(f"Health get_score error for '{agent_name}': {e}")
            return 1.0

    # -------------------------------------------------------------------------
    # Stats
    # -------------------------------------------------------------------------

    async def get_all(self) -> Dict[str, Any]:
        """Return health summary for all tracked agents."""
        result: Dict[str, Any] = {}
        try:
            cursor = 0
            while True:
                cursor, keys = await self.redis.scan(
                    cursor, match=f"{_HEALTH_KEY_PREFIX}*", count=100
                )
                for key in keys:
                    agent_name = key[len(_HEALTH_KEY_PREFIX):]
                    raw = await self.redis.hgetall(key)
                    total = int(raw.get("total_requests", 0))
                    success = int(raw.get("success_count", 0))
                    failure = int(raw.get("failure_count", 0))
                    total_latency = float(raw.get("total_latency_ms", 0.0))
                    avg_latency = round(total_latency / total, 1) if total > 0 else 0.0
                    success_rate = round(success / total, 4) if total > 0 else 1.0

                    # Live queue depth from rate limiter key space
                    try:
                        queue_depth = await self.redis.zcard(f"queue:agent:{agent_name}")
                    except Exception:
                        queue_depth = 0

                    score = self._compute_score(
                        total, success, total_latency, queue_depth, self.max_queue_size
                    )
                    status = (
                        "healthy" if score >= 0.6
                        else "degraded" if score >= 0.3
                        else "unhealthy"
                    )
                    result[agent_name] = {
                        "score": score,
                        "status": status,
                        "total_requests": total,
                        "success_count": success,
                        "failure_count": failure,
                        "success_rate": success_rate,
                        "avg_latency_ms": avg_latency,
                        "queue_depth": queue_depth,
                    }
                if cursor == 0:
                    break
        except Exception as e:
            logger.warning(f"Health get_all error: {e}")
        return result

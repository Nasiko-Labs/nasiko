"""
Adaptive per-agent rate limiter for Sentinel Guard.
Sliding window counter with Redis backing and in-memory fallback.
"""

import logging
import time
from collections import deque
from typing import Optional

import redis

from app.config import config
from app.store import (
    Decision, rate_limits, requests_rejected,
    increment_counter, record_decision,
)

logger = logging.getLogger("sentinel.rate_limiter")


class RateLimiter:
    """Sliding window rate limiter with Redis-backed counters."""

    def __init__(self) -> None:
        try:
            self._redis = redis.Redis(
                host=config.REDIS_HOST, port=config.REDIS_PORT,
                db=config.REDIS_DB, decode_responses=True,
                socket_connect_timeout=5,
            )
            self._redis.ping()
            self._redis_ok = True
            logger.info("Rate limiter connected to Redis")
        except Exception as exc:
            logger.warning(f"Redis unavailable for rate limiter – using in-memory: {exc}")
            self._redis_ok = False
            self._redis = None
        self._windows: dict[str, deque[float]] = {}

    def get_limit(self, agent: str) -> int:
        return rate_limits.get(agent, config.RATE_LIMIT_DEFAULT_RPM)

    def set_limit(self, agent: str, rpm: int) -> None:
        rate_limits[agent] = max(1, rpm)
        logger.info(f"Rate limit set: agent={agent} rpm={rate_limits[agent]}")

    def check(self, agent: str) -> dict:
        limit = self.get_limit(agent)
        window = config.RATE_LIMIT_WINDOW_SECONDS
        now = time.time()
        if self._redis_ok and self._redis:
            return self._redis_check(agent, limit, window, now)
        return self._memory_check(agent, limit, window, now)

    def record_request(self, agent: str) -> None:
        now = time.time()
        if self._redis_ok and self._redis:
            self._redis_record(agent, now)
        else:
            self._memory_record(agent, now)

    def get_stats(self, agent: Optional[str] = None) -> dict:
        now = time.time()
        window = config.RATE_LIMIT_WINDOW_SECONDS

        def _agent_stats(ag: str) -> dict:
            limit = self.get_limit(ag)
            count = self._get_current_count(ag, window, now)
            return {
                "agent": ag, "active_requests_in_window": count,
                "limit_rpm": limit, "remaining": max(0, limit - count),
                "utilization_pct": round(count / limit * 100, 1) if limit > 0 else 0,
            }

        if agent:
            return _agent_stats(agent)
        all_agents = set(list(rate_limits.keys()) | set(self._windows.keys()))
        return {ag: _agent_stats(ag) for ag in all_agents}

    # ── Redis ──────────────────────────────────────────────────────────────

    def _redis_key(self, agent: str) -> str:
        return f"sentinel:rate:{agent}"

    def _redis_check(self, agent, limit, window, now):
        try:
            key = self._redis_key(agent)
            self._redis.zremrangebyscore(key, "-inf", now - window)
            count = self._redis.zcard(key)
            if count >= limit:
                oldest = self._redis.zrange(key, 0, 0, withscores=True)
                retry_ms = int((oldest[0][1] + window - now) * 1000) if oldest else 0
                retry_ms = max(0, retry_ms)
                increment_counter(requests_rejected, agent)
                record_decision(Decision(
                    timestamp=now, agent=agent, query="",
                    outcome="rate_limited", estimated_wait_ms=retry_ms,
                ))
                return {"allowed": False, "remaining": 0, "limit": limit,
                        "retry_after_ms": retry_ms, "current_count": int(count)}
            return {"allowed": True, "remaining": limit - int(count),
                    "limit": limit, "retry_after_ms": None, "current_count": int(count)}
        except Exception as exc:
            logger.warning(f"Redis rate check error: {exc}")
            return {"allowed": True, "remaining": limit, "limit": limit,
                    "retry_after_ms": None, "current_count": 0}

    def _redis_record(self, agent, now):
        try:
            key = self._redis_key(agent)
            self._redis.zadd(key, {f"{now}:{id(object())}": now})
            self._redis.expire(key, config.RATE_LIMIT_WINDOW_SECONDS + 10)
        except Exception as exc:
            logger.debug(f"Redis rate record error: {exc}")

    # ── In-memory ──────────────────────────────────────────────────────────

    def _memory_check(self, agent, limit, window, now):
        if agent not in self._windows:
            self._windows[agent] = deque()
        dq = self._windows[agent]
        while dq and dq[0] <= now - window:
            dq.popleft()
        count = len(dq)
        if count >= limit:
            retry_ms = int((dq[0] + window - now) * 1000) if dq else 0
            retry_ms = max(0, retry_ms)
            increment_counter(requests_rejected, agent)
            record_decision(Decision(
                timestamp=now, agent=agent, query="",
                outcome="rate_limited", estimated_wait_ms=retry_ms,
            ))
            return {"allowed": False, "remaining": 0, "limit": limit,
                    "retry_after_ms": retry_ms, "current_count": count}
        return {"allowed": True, "remaining": limit - count,
                "limit": limit, "retry_after_ms": None, "current_count": count}

    def _memory_record(self, agent, now):
        if agent not in self._windows:
            self._windows[agent] = deque()
        self._windows[agent].append(now)

    def _get_current_count(self, agent, window, now):
        if self._redis_ok and self._redis:
            try:
                key = self._redis_key(agent)
                self._redis.zremrangebyscore(key, "-inf", now - window)
                return int(self._redis.zcard(key))
            except Exception:
                pass
        dq = self._windows.get(agent, deque())
        return sum(1 for ts in dq if ts > now - window)

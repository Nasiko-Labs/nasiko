"""
RAL Metrics Collector
=====================
Collects, aggregates, and persists all RAL observability data to Redis so
that the backend API can serve them to the dashboard without coupling to the
router process memory.

Metrics stored
--------------
ral:metrics:active_requests       INCR/DECR gauge
ral:metrics:rps_window            SORTED SET  (member=uuid, score=timestamp)
ral:metrics:avg_latency_ms        Exponential moving average (HSET)
ral:metrics:per_agent:{name}      HASH  {requests, errors, latency_sum, latency_cnt}
ral:metrics:totals                HASH  {requests, errors, retries, throttled, cache_hits, cache_misses}
ral:logs                          LIST  (LPUSH, capped via LTRIM)

All writes are fire-and-forget via asyncio tasks to avoid adding latency
to the hot path.
"""

from __future__ import annotations

import asyncio
import json
import logging
import time
import uuid
from typing import Any, Dict, Optional

import redis.asyncio as aioredis

from .config import ral_settings

logger = logging.getLogger(__name__)

_PFX = ral_settings.RAL_REDIS_PREFIX
_WIN = ral_settings.RAL_RPS_WINDOW
_LOG_MAX = ral_settings.RAL_LOG_MAX_ENTRIES
_RET = ral_settings.RAL_METRICS_RETENTION

# Key templates
K_ACTIVE   = f"{_PFX}:metrics:active_requests"
K_RPS      = f"{_PFX}:metrics:rps_window"
K_LATENCY  = f"{_PFX}:metrics:avg_latency_ms"
K_AGENT    = f"{_PFX}:metrics:per_agent:{{agent}}"
K_TOTALS   = f"{_PFX}:metrics:totals"
K_LOGS     = f"{_PFX}:logs"


class RalMetrics:
    """
    Singleton-style metrics collector.

    One instance per `RouterOrchestrator`. Writes to Redis asynchronously;
    never blocks the hot path.
    """

    def __init__(self) -> None:
        self._redis: Optional[aioredis.Redis] = None

    # ------------------------------------------------------------------
    # Lifecycle
    # ------------------------------------------------------------------

    async def connect(self) -> None:
        if self._redis is None:
            self._redis = aioredis.from_url(
                ral_settings.redis_dsn,
                encoding="utf-8",
                decode_responses=True,
                max_connections=10,
            )

    async def close(self) -> None:
        if self._redis:
            await self._redis.aclose()
            self._redis = None

    # ------------------------------------------------------------------
    # Event hooks — call these from the orchestrator
    # ------------------------------------------------------------------

    def on_request_start(self, agent_id: str) -> None:
        """Called when a request enters active processing."""
        self._fire(self._inc_active(1))
        self._fire(self._record_rps_tick())
        self._fire(self._inc_agent(agent_id, "requests", 1))
        self._fire(self._inc_total("requests", 1))

    def on_request_end(
        self,
        agent_id: str,
        latency_ms: float,
        *,
        from_cache: bool = False,
        error: bool = False,
        retried: bool = False,
        throttled: bool = False,
    ) -> None:
        """Called when a request completes (success or failure)."""
        self._fire(self._inc_active(-1))
        self._fire(self._record_latency(latency_ms, agent_id))
        if error:
            self._fire(self._inc_total("errors", 1))
            self._fire(self._inc_agent(agent_id, "errors", 1))
        if retried:
            self._fire(self._inc_total("retries", 1))
        if throttled:
            self._fire(self._inc_total("throttled", 1))

    def on_cache_hit(self) -> None:
        self._fire(self._inc_total("cache_hits", 1))

    def on_cache_miss(self) -> None:
        self._fire(self._inc_total("cache_misses", 1))

    def log_request(self, entry: Dict[str, Any]) -> None:
        """Append a structured log entry for the recent-requests table."""
        self._fire(self._append_log(entry))

    # ------------------------------------------------------------------
    # Snapshot (read path — used by ral_service.py)
    # ------------------------------------------------------------------

    async def get_snapshot(self) -> Dict[str, Any]:
        if self._redis is None:
            return self._empty_snapshot()
        try:
            pipe = self._redis.pipeline(transaction=False)
            pipe.get(K_ACTIVE)
            pipe.hgetall(K_TOTALS)
            pipe.get(K_LATENCY)
            pipe.zcard(K_RPS)
            # Clean the rps window as part of the read
            now = time.time()
            pipe.zremrangebyscore(K_RPS, "-inf", now - _WIN)
            pipe.zcard(K_RPS)  # count after cleanup = requests in window
            results = await pipe.execute()

            active       = int(results[0] or 0)
            totals       = results[1] or {}
            avg_lat      = float(results[2] or 0)
            rps_after    = int(results[5] or 0)
            rps          = round(rps_after / _WIN, 2)

            # Per-agent stats
            agent_keys = await self._redis.keys(f"{_PFX}:metrics:per_agent:*")
            per_agent = {}
            if agent_keys:
                for k in agent_keys:
                    agent_name = k.split(":")[-1]
                    data = await self._redis.hgetall(k)
                    lat_sum = float(data.get("latency_sum", 0))
                    lat_cnt = int(data.get("latency_cnt", 1) or 1)
                    per_agent[agent_name] = {
                        "requests":   int(data.get("requests", 0)),
                        "errors":     int(data.get("errors", 0)),
                        "avg_latency_ms": round(lat_sum / lat_cnt, 2),
                    }

            hits   = int(totals.get("cache_hits", 0))
            misses = int(totals.get("cache_misses", 0))
            total_reqs = hits + misses
            hit_ratio  = round(hits / total_reqs, 4) if total_reqs else 0.0

            return {
                "active_requests":   active,
                "requests_per_sec":  rps,
                "avg_latency_ms":    avg_lat,
                "cache_hits":        hits,
                "cache_misses":      misses,
                "cache_hit_ratio":   hit_ratio,
                "total_requests":    int(totals.get("requests", 0)),
                "total_errors":      int(totals.get("errors", 0)),
                "total_retries":     int(totals.get("retries", 0)),
                "total_throttled":   int(totals.get("throttled", 0)),
                "per_agent":         per_agent,
            }
        except Exception as exc:
            logger.warning("RAL metrics snapshot error: %s", exc)
            return self._empty_snapshot()

    async def get_recent_logs(self, limit: int = 50) -> list:
        if self._redis is None:
            return []
        try:
            raw = await self._redis.lrange(K_LOGS, 0, limit - 1)
            return [json.loads(r) for r in raw]
        except Exception as exc:
            logger.warning("RAL logs read error: %s", exc)
            return []

    # ------------------------------------------------------------------
    # Internal async writers
    # ------------------------------------------------------------------

    def _fire(self, coro) -> None:
        """Schedule a coroutine as a fire-and-forget background task."""
        try:
            loop = asyncio.get_event_loop()
            if loop.is_running():
                loop.create_task(coro)
        except Exception:
            pass  # Never let metric writes crash the hot path

    async def _inc_active(self, delta: int) -> None:
        if self._redis:
            try:
                await self._redis.incrby(K_ACTIVE, delta)
            except Exception:
                pass

    async def _record_rps_tick(self) -> None:
        if self._redis:
            try:
                now = time.time()
                member = f"{now:.6f}-{uuid.uuid4().hex[:8]}"
                await self._redis.zadd(K_RPS, {member: now})
                # Expire old entries
                await self._redis.zremrangebyscore(K_RPS, "-inf", now - _WIN)
                await self._redis.expire(K_RPS, _WIN * 2)
            except Exception:
                pass

    async def _record_latency(self, latency_ms: float, agent_id: str) -> None:
        if self._redis:
            try:
                # Simple exponential moving average stored as a float string
                prev = float(await self._redis.get(K_LATENCY) or latency_ms)
                ema = prev * 0.9 + latency_ms * 0.1
                await self._redis.set(K_LATENCY, f"{ema:.2f}", ex=_RET)
                # Per-agent latency accumulation
                k = K_AGENT.format(agent=agent_id)
                await self._redis.hincrbyfloat(k, "latency_sum", latency_ms)
                await self._redis.hincrby(k, "latency_cnt", 1)
                await self._redis.expire(k, _RET)
            except Exception:
                pass

    async def _inc_agent(self, agent_id: str, field: str, delta: int) -> None:
        if self._redis:
            try:
                k = K_AGENT.format(agent=agent_id)
                await self._redis.hincrby(k, field, delta)
                await self._redis.expire(k, _RET)
            except Exception:
                pass

    async def _inc_total(self, field: str, delta: int) -> None:
        if self._redis:
            try:
                await self._redis.hincrby(K_TOTALS, field, delta)
                await self._redis.expire(K_TOTALS, _RET)
            except Exception:
                pass

    async def _append_log(self, entry: Dict[str, Any]) -> None:
        if self._redis:
            try:
                await self._redis.lpush(K_LOGS, json.dumps(entry))
                await self._redis.ltrim(K_LOGS, 0, _LOG_MAX - 1)
                await self._redis.expire(K_LOGS, _RET)
            except Exception:
                pass

    @staticmethod
    def _empty_snapshot() -> Dict[str, Any]:
        return {
            "active_requests": 0,
            "requests_per_sec": 0.0,
            "avg_latency_ms": 0.0,
            "cache_hits": 0,
            "cache_misses": 0,
            "cache_hit_ratio": 0.0,
            "total_requests": 0,
            "total_errors": 0,
            "total_retries": 0,
            "total_throttled": 0,
            "per_agent": {},
        }

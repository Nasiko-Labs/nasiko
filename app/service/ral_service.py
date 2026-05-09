"""
RAL Service
===========
Reads RAL metrics stored in Redis by the router's RAL middleware and exposes
them through the main backend API.  Follows the existing Service pattern
(thin service layer over a data source, injected into handlers).

All Redis keys are in the `ral:` namespace, written by the router process.
This service is read-only — it never writes to those keys directly.
"""

from __future__ import annotations

import json
import logging
import time
from typing import Any, Dict, List, Optional

import redis.asyncio as aioredis

from app.pkg.config.config import settings

logger = logging.getLogger(__name__)

_PFX = "ral"

# Key templates (must match the ones in router/src/ral/metrics.py)
K_ACTIVE   = f"{_PFX}:metrics:active_requests"
K_RPS      = f"{_PFX}:metrics:rps_window"
K_LATENCY  = f"{_PFX}:metrics:avg_latency_ms"
K_AGENT    = f"{_PFX}:metrics:per_agent:{{agent}}"
K_TOTALS   = f"{_PFX}:metrics:totals"
K_LOGS     = f"{_PFX}:logs"
K_HIT      = f"{_PFX}:stats:cache_hits"
K_MISS     = f"{_PFX}:stats:cache_misses"
K_LAT_SUM  = f"{_PFX}:stats:cache_lat_ms_sum"
K_LAT_CNT  = f"{_PFX}:stats:cache_lat_ms_count"

_RPS_WINDOW = 10  # seconds


class RalService:
    """
    Read-only service that surfaces RAL metrics stored by the router process.
    """

    def __init__(self, logger_=None) -> None:
        self.logger = logger_ or logger
        self._redis: Optional[aioredis.Redis] = None

    # ------------------------------------------------------------------
    # Lifecycle
    # ------------------------------------------------------------------

    async def connect(self) -> None:
        if self._redis is None:
            dsn = getattr(settings, "REDIS_URL", None) or (
                f"redis://{settings.REDIS_HOST}:{settings.REDIS_PORT}/{settings.REDIS_DB}"
            )
            self._redis = aioredis.from_url(
                dsn,
                encoding="utf-8",
                decode_responses=True,
                max_connections=10,
            )
            self.logger.info("RalService connected to Redis")

    async def close(self) -> None:
        if self._redis:
            await self._redis.aclose()
            self._redis = None

    # ------------------------------------------------------------------
    # Public API
    # ------------------------------------------------------------------

    async def get_metrics_snapshot(self) -> Dict[str, Any]:
        """Return the full metrics snapshot from Redis."""
        if not self._redis:
            await self.connect()

        try:
            pipe = self._redis.pipeline(transaction=False)
            pipe.get(K_ACTIVE)
            pipe.hgetall(K_TOTALS)
            pipe.get(K_LATENCY)
            pipe.get(K_HIT)
            pipe.get(K_MISS)
            pipe.get(K_LAT_SUM)
            pipe.get(K_LAT_CNT)
            # Clean + count rps window
            now = time.time()
            pipe.zremrangebyscore(K_RPS, "-inf", now - _RPS_WINDOW)
            pipe.zcard(K_RPS)
            results = await pipe.execute()

            active     = int(results[0] or 0)
            totals     = results[1] or {}
            avg_lat    = float(results[2] or 0)
            hits       = int(results[3] or 0)
            misses     = int(results[4] or 0)
            lat_sum    = float(results[5] or 0)
            lat_cnt    = int(results[6] or 0)
            rps_count  = int(results[8] or 0)
            rps        = round(rps_count / _RPS_WINDOW, 2)

            cache_total = hits + misses
            hit_ratio   = round(hits / cache_total, 4) if cache_total else 0.0
            cache_lat   = round(lat_sum / lat_cnt, 2)  if lat_cnt    else 0.0

            # Per-agent breakdown
            agent_keys = await self._redis.keys(f"{_PFX}:metrics:per_agent:*")
            per_agent: Dict[str, Any] = {}
            for k in (agent_keys or []):
                name = k.split(":")[-1]
                data = await self._redis.hgetall(k)
                lsum = float(data.get("latency_sum", 0))
                lcnt = int(data.get("latency_cnt", 1) or 1)
                per_agent[name] = {
                    "requests": int(data.get("requests", 0)),
                    "errors":   int(data.get("errors", 0)),
                    "avg_latency_ms": round(lsum / lcnt, 2),
                }

            return {
                "active_requests":  active,
                "requests_per_sec": rps,
                "avg_latency_ms":   avg_lat,
                "cache": {
                    "hits": hits,
                    "misses": misses,
                    "hit_ratio": hit_ratio,
                    "avg_latency_ms": cache_lat,
                },
                "queue": await self._get_queue_stats(),
                "total_requests":  int(totals.get("requests",  0)),
                "total_errors":    int(totals.get("errors",    0)),
                "total_retries":   int(totals.get("retries",   0)),
                "total_throttled": int(totals.get("throttled", 0)),
                "per_agent": per_agent,
            }
        except Exception as exc:
            self.logger.warning("RalService.get_metrics_snapshot error: %s", exc)
            return self._empty_snapshot()

    async def get_request_logs(self, limit: int = 50) -> List[Dict[str, Any]]:
        """Return the most recent request log entries."""
        if not self._redis:
            await self.connect()
        try:
            raw = await self._redis.lrange(K_LOGS, 0, limit - 1)
            return [json.loads(r) for r in raw]
        except Exception as exc:
            self.logger.warning("RalService.get_request_logs error: %s", exc)
            return []

    async def get_agent_stats(self) -> List[Dict[str, Any]]:
        """Return per-agent stats list."""
        snapshot = await self.get_metrics_snapshot()
        per_agent = snapshot.get("per_agent", {})
        return [{"agent_id": k, **v} for k, v in per_agent.items()]

    async def flush_cache(self) -> int:
        """Delete all ral:cache:* keys. Returns count deleted."""
        if not self._redis:
            await self.connect()
        try:
            keys = await self._redis.keys(f"{_PFX}:cache:*")
            if keys:
                deleted = await self._redis.delete(*keys)
                self.logger.info("RalService: flushed %d cache keys", deleted)
                return int(deleted)
        except Exception as exc:
            self.logger.warning("RalService.flush_cache error: %s", exc)
        return 0

    async def get_health(self) -> Dict[str, Any]:
        """Return a health summary of the RAL subsystem."""
        if not self._redis:
            await self.connect()

        components = []
        overall = "healthy"

        # Redis connectivity
        try:
            await self._redis.ping()
            components.append({"name": "redis", "status": "healthy"})
        except Exception as exc:
            components.append({"name": "redis", "status": "unavailable", "detail": str(exc)})
            overall = "unhealthy"

        # Metric freshness
        try:
            snapshot = await self.get_metrics_snapshot()
            active = snapshot["active_requests"]
            q_size = snapshot.get("queue", {}).get("queue_size", 0)
            hit_ratio = snapshot.get("cache", {}).get("hit_ratio", 0.0)
            components.append({"name": "metrics", "status": "healthy"})
        except Exception as exc:
            active = q_size = 0
            hit_ratio = 0.0
            components.append({"name": "metrics", "status": "degraded", "detail": str(exc)})
            overall = "degraded" if overall == "healthy" else overall

        return {
            "overall": overall,
            "components": components,
            "active_requests": active,
            "queue_size": q_size,
            "cache_hit_ratio": hit_ratio,
        }

    # ------------------------------------------------------------------
    # Internal helpers
    # ------------------------------------------------------------------

    async def _get_queue_stats(self) -> Dict[str, Any]:
        """Queue stats are written by the router; we read them from Redis."""
        try:
            # The router writes queue stats into ral:metrics:queue hash
            data = await self._redis.hgetall(f"{_PFX}:metrics:queue") or {}
            return {
                "queue_size":     int(data.get("queue_size",    0)),
                "max_queue_size": int(data.get("max_queue_size", 0)),
                "enqueued":       int(data.get("enqueued",      0)),
                "completed":      int(data.get("completed",     0)),
                "failed":         int(data.get("failed",        0)),
                "retried":        int(data.get("retried",       0)),
                "timed_out":      int(data.get("timed_out",     0)),
                "dropped":        int(data.get("dropped",       0)),
            }
        except Exception:
            return {}

    @staticmethod
    def _empty_snapshot() -> Dict[str, Any]:
        return {
            "active_requests": 0, "requests_per_sec": 0.0,
            "avg_latency_ms": 0.0,
            "cache": {"hits": 0, "misses": 0, "hit_ratio": 0.0, "avg_latency_ms": 0.0},
            "queue": {}, "total_requests": 0, "total_errors": 0,
            "total_retries": 0, "total_throttled": 0, "per_agent": {},
        }

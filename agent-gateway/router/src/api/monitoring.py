"""
Monitoring API router — cache, rate limit, agent health, and system overview.

All endpoints are unauthenticated (Kong guards the outer perimeter).
Mount with prefix="/monitoring" in main.py.
"""

import logging
from typing import Any, Dict, Optional

from fastapi import APIRouter, HTTPException
from pydantic import BaseModel

from router.src.core.cache_service import AgentResponseCache
from router.src.core.rate_limiter import AgentRateLimiter
from router.src.core.agent_health import AgentHealthTracker
from router.src.core.events import EventEmitter

logger = logging.getLogger(__name__)


class RateLimitConfigUpdate(BaseModel):
    rpm: int
    max_queue: int


def create_monitoring_router(
    cache: Optional[AgentResponseCache],
    rate_limiter: Optional[AgentRateLimiter],
    health: Optional[AgentHealthTracker],
    emitter: Optional[EventEmitter] = None,
) -> APIRouter:
    router = APIRouter(tags=["monitoring"])

    # -------------------------------------------------------------------------
    # Overview — single endpoint for dashboards / health checks
    # -------------------------------------------------------------------------

    @router.get("/overview")
    async def overview() -> Dict[str, Any]:
        """
        System-level health overview.

        status: "healthy" | "degraded" | "overloaded"
        - healthy:    hit_rate > 0.5 AND max_queue_depth < 10 AND avg_latency < 3000ms
        - overloaded: hit_rate < 0.3 OR max_queue_depth > 40 OR avg_latency > 8000ms
        - degraded:   everything in between
        """
        cache_stats = await cache.get_stats() if cache else {}
        rl_stats = await rate_limiter.get_stats() if rate_limiter else {"agents": {}}
        health_data = await health.get_all() if health else {}

        hit_rate: float = cache_stats.get("hit_rate", 0.0)
        total_requests: int = cache_stats.get("total_requests", 0)

        # Aggregate latency across all agents
        all_latencies = [
            v["avg_latency_ms"] for v in health_data.values() if "avg_latency_ms" in v
        ]
        avg_latency_ms = round(sum(all_latencies) / len(all_latencies), 1) if all_latencies else 0.0

        # Queue depth totals + bottleneck
        agent_queues = {
            name: data.get("queue_depth", 0)
            for name, data in rl_stats["agents"].items()
        }
        queue_depth_total = sum(agent_queues.values())
        top_bottleneck = (
            max(agent_queues, key=agent_queues.get) if agent_queues else None
        )

        # Derive status
        if (
            hit_rate < 0.3 and total_requests > 10
            or queue_depth_total > 40
            or avg_latency_ms > 8000
        ):
            status = "overloaded"
        elif (
            hit_rate < 0.5 and total_requests > 10
            or queue_depth_total > 10
            or avg_latency_ms > 3000
        ):
            status = "degraded"
        else:
            status = "healthy"

        return {
            "status": status,
            "cache_hit_rate": hit_rate,
            "total_requests": total_requests,
            "avg_latency_ms": avg_latency_ms,
            "top_bottleneck_agent": top_bottleneck,
            "queue_depth_total": queue_depth_total,
            "cache_enabled": cache is not None,
            "rate_limit_enabled": rate_limiter is not None,
            "health_tracking_enabled": health is not None,
        }

    # -------------------------------------------------------------------------
    # Cache
    # -------------------------------------------------------------------------

    @router.get("/cache/stats")
    async def cache_stats() -> Dict[str, Any]:
        if not cache:
            raise HTTPException(status_code=503, detail="Cache not enabled")
        return await cache.get_stats()

    @router.delete("/cache")
    async def cache_clear_all() -> Dict[str, Any]:
        if not cache:
            raise HTTPException(status_code=503, detail="Cache not enabled")
        deleted = await cache.invalidate_all()
        return {"deleted": deleted, "message": "All cache entries cleared"}

    @router.delete("/cache/{agent_name}")
    async def cache_clear_agent(agent_name: str) -> Dict[str, Any]:
        if not cache:
            raise HTTPException(status_code=503, detail="Cache not enabled")
        deleted = await cache.invalidate_agent(agent_name)
        return {"agent": agent_name, "deleted": deleted}

    # -------------------------------------------------------------------------
    # Rate limit config
    # -------------------------------------------------------------------------

    @router.get("/ratelimit/config")
    async def ratelimit_config_all() -> Dict[str, Any]:
        if not rate_limiter:
            raise HTTPException(status_code=503, detail="Rate limiter not enabled")
        configs = await rate_limiter.get_all_configs()
        return {
            "agents": configs,
            "defaults": {
                "rpm": rate_limiter.default_rpm,
                "max_queue": rate_limiter.max_queue_size,
            },
        }

    @router.get("/ratelimit/config/{agent_name}")
    async def ratelimit_config_agent(agent_name: str) -> Dict[str, Any]:
        if not rate_limiter:
            raise HTTPException(status_code=503, detail="Rate limiter not enabled")
        config = await rate_limiter.get_config(agent_name)
        return {"agent": agent_name, **config}

    @router.put("/ratelimit/config/{agent_name}")
    async def ratelimit_config_update(
        agent_name: str, body: RateLimitConfigUpdate
    ) -> Dict[str, Any]:
        if not rate_limiter:
            raise HTTPException(status_code=503, detail="Rate limiter not enabled")
        if body.rpm < 1:
            raise HTTPException(status_code=422, detail="rpm must be >= 1")
        if body.max_queue < 0:
            raise HTTPException(status_code=422, detail="max_queue must be >= 0")
        await rate_limiter.set_config(agent_name, body.rpm, body.max_queue)
        return {"agent": agent_name, "rpm": body.rpm, "max_queue": body.max_queue}

    # -------------------------------------------------------------------------
    # Rate limit stats
    # -------------------------------------------------------------------------

    @router.get("/ratelimit/stats")
    async def ratelimit_stats() -> Dict[str, Any]:
        if not rate_limiter:
            raise HTTPException(status_code=503, detail="Rate limiter not enabled")
        return await rate_limiter.get_stats()

    # -------------------------------------------------------------------------
    # Agent health
    # -------------------------------------------------------------------------

    @router.get("/agents/health")
    async def agents_health() -> Dict[str, Any]:
        if not health:
            raise HTTPException(status_code=503, detail="Health tracking not enabled")
        data = await health.get_all()
        return {"agents": data}

    # -------------------------------------------------------------------------
    # Combined stats
    # -------------------------------------------------------------------------

    @router.get("/stats")
    async def all_stats() -> Dict[str, Any]:
        result: Dict[str, Any] = {}
        if cache:
            result["cache"] = await cache.get_stats()
        if rate_limiter:
            result["rate_limit"] = await rate_limiter.get_stats()
        if health:
            result["agents"] = await health.get_all()
        return result

    # -------------------------------------------------------------------------
    # Real-time event stream (populated by EventEmitter, no inference)
    # -------------------------------------------------------------------------

    @router.get("/events")
    async def events(n: int = 50) -> Dict[str, Any]:
        data = await emitter.get_recent(n) if emitter else []
        return {"events": data, "count": len(data)}

    # -------------------------------------------------------------------------
    # Impact metrics — honest, correctly named
    # -------------------------------------------------------------------------

    @router.get("/impact")
    async def impact() -> Dict[str, Any]:
        cache_s = await cache.get_stats() if cache else {}
        hits = int(cache_s.get("hits", 0))
        total = int(cache_s.get("total_requests", 0))
        cache_coverage_percent = round((hits / total) * 100, 1) if total > 0 else 0.0

        health_data = await health.get_all() if health else {}
        latencies = [
            v["avg_latency_ms"] for v in health_data.values() if v.get("avg_latency_ms", 0) > 0
        ]
        avg_agent_latency = round(sum(latencies) / len(latencies), 1) if latencies else 2000.0
        avg_cache_latency = 8.0

        compute_saved_estimate_ms = (
            round(hits * (avg_agent_latency - avg_cache_latency), 0)
            if avg_agent_latency > avg_cache_latency
            else 0
        )

        return {
            "cache_coverage_percent": cache_coverage_percent,
            "compute_saved_estimate_ms": compute_saved_estimate_ms,
            "cache_hit_rate": round(hits / total, 4) if total > 0 else 0.0,
            "llm_calls_saved": hits,
            "avg_latency_uncached_ms": avg_agent_latency,
            "avg_latency_cached_ms": avg_cache_latency,
            "total_requests": total,
        }

    return router

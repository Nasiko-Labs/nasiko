import asyncio
import json
import time
from typing import AsyncIterator, Optional

from fastapi import APIRouter, HTTPException, Path, Body, Query
from fastapi.responses import StreamingResponse

from gateway.cache.cache_manager import cache_manager
from gateway.rate_limiter.token_bucket import rate_limiter
from gateway.queue.request_queue import queue_manager
from gateway.routing.proxy import get_proxy_stats, AGENT_FLEET

router = APIRouter(prefix="/admin", tags=["Admin"])


# ─── Global Stats ─────────────────────────────────────────────────────────────

@router.get("/stats")
async def global_stats():
    """Return aggregated global stats: cache, rate limiter, queues, latency."""
    return {
        "timestamp": time.time(),
        "cache": cache_manager.get_stats(),
        "rate_limiter": rate_limiter.get_stats(),
        "queues": queue_manager.get_all_stats(),
        "proxy": get_proxy_stats(),
    }


@router.get("/stats/{agent_id}")
async def agent_stats(agent_id: str = Path(...)):
    """Return stats for a specific agent."""
    return {
        "timestamp": time.time(),
        "agent_id": agent_id,
        "cache": cache_manager.get_stats(agent_id),
        "rate_limiter": rate_limiter.get_stats(agent_id),
        "queue": queue_manager.get_agent_stats(agent_id),
    }


# ─── Cache Management ─────────────────────────────────────────────────────────

@router.get("/cache")
async def view_cache():
    """View cache stats."""
    return cache_manager.get_stats()


@router.delete("/cache")
async def flush_all_cache():
    """Flush entire cache."""
    await cache_manager.flush_all()
    return {"status": "ok", "message": "All cache entries flushed"}


@router.delete("/cache/{agent_id}")
async def flush_agent_cache(agent_id: str = Path(...)):
    """Flush cache for a specific agent."""
    await cache_manager.flush_agent(agent_id)
    return {"status": "ok", "message": f"Cache flushed for agent '{agent_id}'"}


# ─── Rate Limit Config ────────────────────────────────────────────────────────

@router.get("/rate-limits")
async def view_rate_limits():
    """View current rate limit configs for all agents."""
    return {
        "configs": rate_limiter.all_configs(),
        "stats": rate_limiter.get_stats(),
    }


@router.put("/rate-limits/{agent_id}")
async def update_rate_limit(
    agent_id: str = Path(...),
    rps: Optional[float] = Body(None, embed=True),
    burst: Optional[int] = Body(None, embed=True),
):
    """Dynamically update rate limit for a specific agent."""
    if rps is None and burst is None:
        raise HTTPException(status_code=400, detail="Provide at least one of: rps, burst")
    rate_limiter.update_config(agent_id, rps=rps, burst=burst)
    return {
        "status": "ok",
        "agent_id": agent_id,
        "updated_config": rate_limiter.get_config(agent_id).__dict__,
    }


# ─── Queue Inspection ─────────────────────────────────────────────────────────

@router.get("/queues")
async def view_queues():
    """View all agent queue depths and stats."""
    return queue_manager.get_all_stats()


@router.get("/queues/{agent_id}")
async def view_agent_queue(agent_id: str = Path(...)):
    """View queue stats for a specific agent."""
    stats = queue_manager.get_agent_stats(agent_id)
    if not stats:
        raise HTTPException(status_code=404, detail=f"No queue found for agent '{agent_id}'")
    return stats


# ─── Agent Fleet ──────────────────────────────────────────────────────────────

@router.get("/agents")
async def list_agents():
    """List all registered agents and their configuration."""
    return {"agents": AGENT_FLEET}


# ─── Live SSE Stream ──────────────────────────────────────────────────────────

async def _stats_event_generator() -> AsyncIterator[str]:
    """Yield stats every second as Server-Sent Events."""
    while True:
        data = {
            "timestamp": time.time(),
            "cache": cache_manager.get_stats(),
            "rate_limiter": rate_limiter.get_stats(),
            "queues": queue_manager.get_all_stats(),
            "proxy": get_proxy_stats(),
        }
        yield f"data: {json.dumps(data)}\n\n"
        await asyncio.sleep(1)


@router.get("/stream", response_class=StreamingResponse)
async def stats_stream():
    """SSE endpoint streaming live stats every second."""
    return StreamingResponse(
        _stats_event_generator(),
        media_type="text/event-stream",
        headers={
            "Cache-Control": "no-cache",
            "X-Accel-Buffering": "no",
        },
    )

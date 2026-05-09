"""
Sentinel Guard — Main FastAPI Application.
Resilient Agent Request Layer: semantic caching + adaptive rate limiting + queuing.
"""

import asyncio
import json
import logging
import time
from typing import Any, Optional

import httpx
from fastapi import FastAPI, HTTPException, Request
from fastapi.middleware.cors import CORSMiddleware
from fastapi.responses import HTMLResponse, StreamingResponse
from pydantic import BaseModel

from app.cache import CacheLayer
from app.config import config
from app.monitor import DASHBOARD_HTML
from app.queue_manager import QueueManager
from app.rate_limiter import RateLimiter
from app.store import (
    Decision,
    cache_hits,
    cache_misses,
    cache_semantic_hits,
    decision_log,
    get_all_known_agents,
    increment_counter,
    increment_float,
    record_decision,
    requests_forwarded,
    requests_queued,
    requests_rejected,
    total_latency_ms,
    total_requests,
    rate_limits,
)

logging.basicConfig(
    level=logging.INFO,
    format="%(asctime)s [%(levelname)s] %(name)s: %(message)s",
)
logger = logging.getLogger("sentinel.main")

# ── FastAPI App ────────────────────────────────────────────────────────────────

app = FastAPI(
    title="Sentinel Guard",
    description="Resilient Agent Request Layer — semantic caching, adaptive rate limiting, and request queuing",
    version="1.0.0",
)

app.add_middleware(
    CORSMiddleware,
    allow_origins=["*"],
    allow_credentials=True,
    allow_methods=["*"],
    allow_headers=["*"],
)

# ── Singletons ─────────────────────────────────────────────────────────────────

cache_layer = CacheLayer()
rate_limiter = RateLimiter()
queue_manager = QueueManager()

# ── Request / Response Models ──────────────────────────────────────────────────


class ProxyRequest(BaseModel):
    agent: str
    query: str
    session_id: Optional[str] = None
    payload: Optional[dict] = None


class RateLimitUpdate(BaseModel):
    rpm: int


# ── Endpoints ──────────────────────────────────────────────────────────────────


@app.get("/health")
async def health():
    """Health check endpoint."""
    return {
        "status": "healthy",
        "service": "sentinel-guard",
        "version": "1.0.0",
        "cache": cache_layer.stats(),
        "uptime_seconds": time.time() - _start_time,
    }


@app.post("/proxy")
async def proxy_request(req: ProxyRequest):
    """
    Main proxy endpoint.
    Flow: cache check → rate limit → queue or forward → cache store.
    """
    start = time.time()
    agent = req.agent.strip()
    query = req.query.strip()

    if not agent or not query:
        raise HTTPException(status_code=400, detail="agent and query are required")

    increment_counter(total_requests, agent)

    # ── Step 1: Cache lookup ───────────────────────────────────────────────
    cached = cache_layer.lookup(query, agent)
    if cached is not None:
        latency = (time.time() - start) * 1000
        increment_float(total_latency_ms, agent, latency)
        # Remove internal metadata before returning
        cached.pop("_similarity", None)
        cached.pop("_cache_source", None)
        return {
            "source": "cache",
            "agent": agent,
            "latency_ms": round(latency, 2),
            "result": cached,
        }

    # ── Step 2: Rate limit check ───────────────────────────────────────────
    rate_result = rate_limiter.check(agent)

    if not rate_result["allowed"]:
        # Try to queue
        queue_result = queue_manager.enqueue(agent, query, req.payload)
        if not queue_result.get("queued"):
            raise HTTPException(
                status_code=429,
                detail={
                    "error": "rate_limited_and_queue_full",
                    "agent": agent,
                    "retry_after_ms": rate_result.get("retry_after_ms"),
                    "queue_depth": queue_result.get("depth"),
                    "max_queue_depth": queue_result.get("max_depth"),
                },
            )
        return {
            "source": "queued",
            "agent": agent,
            "position": queue_result["position"],
            "estimated_wait_ms": queue_result["estimated_wait_ms"],
            "message": f"Request queued at position {queue_result['position']}",
        }

    # ── Step 3: Forward to agent ───────────────────────────────────────────
    rate_limiter.record_request(agent)
    increment_counter(requests_forwarded, agent)

    agent_url = f"{config.NASIKO_BASE_URL}/agent-{agent}"
    try:
        agent_response = await _forward_to_agent(agent_url, query, req.session_id, req.payload)
    except Exception as exc:
        latency = (time.time() - start) * 1000
        record_decision(Decision(
            timestamp=time.time(), agent=agent, query=query,
            outcome="forward_error", latency_ms=latency,
        ))
        raise HTTPException(status_code=502, detail=f"Agent error: {str(exc)}")

    # ── Step 4: Cache the response ─────────────────────────────────────────
    cache_layer.store(query, agent_response, agent)

    latency = (time.time() - start) * 1000
    increment_float(total_latency_ms, agent, latency)
    record_decision(Decision(
        timestamp=time.time(), agent=agent, query=query,
        outcome="forwarded", latency_ms=latency,
    ))

    return {
        "source": "agent",
        "agent": agent,
        "latency_ms": round(latency, 2),
        "result": agent_response,
    }


@app.get("/cache/check")
async def cache_check(query: str, agent: str):
    """Check cache without forwarding. Returns cached response or null."""
    result = cache_layer.lookup(query, agent)
    return {
        "hit": result is not None,
        "agent": agent,
        "result": result,
    }


@app.post("/cache/store")
async def cache_store(req: ProxyRequest):
    """Manually store a query-response pair in cache."""
    cache_layer.store(req.query, req.payload or {}, req.agent)
    return {"stored": True, "agent": req.agent}


@app.post("/cache/flush")
async def cache_flush(agent: Optional[str] = None):
    """Flush all cache or per-agent cache."""
    flushed = cache_layer.flush(agent)
    return {"flushed": flushed, "agent": agent}


@app.get("/rate/check/{agent}")
async def rate_check(agent: str):
    """Check current rate limit status for an agent."""
    return rate_limiter.check(agent)


@app.put("/config/rate-limit/{agent}")
async def update_rate_limit(agent: str, body: RateLimitUpdate):
    """Update the RPM limit for a specific agent."""
    rate_limiter.set_limit(agent, body.rpm)
    return {"agent": agent, "new_limit_rpm": rate_limiter.get_limit(agent)}


@app.get("/queue/status")
async def queue_status():
    """Get queue depths for all agents."""
    return {"queues": queue_manager.get_all_depths()}


@app.get("/stats")
async def get_stats():
    """Full runtime statistics for all components."""
    return _build_stats_payload()


@app.get("/dashboard", response_class=HTMLResponse)
async def dashboard():
    """Serve the real-time monitoring dashboard."""
    return HTMLResponse(content=DASHBOARD_HTML)


@app.get("/events")
async def sse_events():
    """SSE stream for real-time dashboard updates."""
    async def event_generator():
        while True:
            data = _build_stats_payload()
            yield f"data: {json.dumps(data)}\n\n"
            await asyncio.sleep(config.DASHBOARD_SSE_INTERVAL)

    return StreamingResponse(
        event_generator(),
        media_type="text/event-stream",
        headers={
            "Cache-Control": "no-cache",
            "Connection": "keep-alive",
            "X-Accel-Buffering": "no",
        },
    )


# ── Internal helpers ───────────────────────────────────────────────────────────

_start_time = time.time()


async def _forward_to_agent(
    agent_url: str,
    query: str,
    session_id: Optional[str],
    payload: Optional[dict],
) -> dict:
    """Forward a request to the target agent via HTTP."""
    body: dict[str, Any] = {"query": query}
    if session_id:
        body["session_id"] = session_id
    if payload:
        body.update(payload)

    timeout = httpx.Timeout(60.0, connect=10.0)
    async with httpx.AsyncClient(timeout=timeout) as client:
        resp = await client.post(agent_url, json=body)
        resp.raise_for_status()
        return resp.json()


def _build_stats_payload() -> dict:
    """Build the full stats payload used by /stats and SSE."""
    agents = get_all_known_agents()
    queue_depths = queue_manager.get_all_depths()

    total_hits = sum(cache_hits.values())
    total_miss = sum(cache_misses.values())
    total_sem = sum(cache_semantic_hits.values())
    total_fwd = sum(requests_forwarded.values())
    total_q = sum(requests_queued.values())
    total_rej = sum(requests_rejected.values())
    total_req = sum(total_requests.values())
    total_lat = sum(total_latency_ms.values())

    hit_rate = (total_hits / (total_hits + total_miss) * 100) if (total_hits + total_miss) > 0 else 0
    avg_lat = (total_lat / total_hits) if total_hits > 0 else 0

    per_agent: dict[str, dict] = {}
    for ag in agents:
        hits = cache_hits.get(ag, 0)
        misses = cache_misses.get(ag, 0)
        t = total_requests.get(ag, 0)
        per_agent[ag] = {
            "total": t,
            "hits": hits,
            "misses": misses,
            "semantic_hits": cache_semantic_hits.get(ag, 0),
            "forwarded": requests_forwarded.get(ag, 0),
            "queued": requests_queued.get(ag, 0),
            "rejected": requests_rejected.get(ag, 0),
            "hit_rate_pct": round(hits / t * 100, 1) if t > 0 else 0,
            "rate_limit": rate_limiter.get_limit(ag),
            "queue_depth": queue_depths.get(ag, 0),
        }

    recent = [
        {
            "timestamp": d.timestamp,
            "agent": d.agent,
            "query": d.query[:100],
            "outcome": d.outcome,
            "latency_ms": d.latency_ms,
            "similarity": d.similarity,
            "queue_position": d.queue_position,
            "estimated_wait_ms": d.estimated_wait_ms,
            "source": d.source,
        }
        for d in list(decision_log)[-50:]
    ]
    recent.reverse()

    return {
        "summary": {
            "total_requests": total_req,
            "total_cache_hits": total_hits,
            "total_cache_misses": total_miss,
            "total_semantic_hits": total_sem,
            "total_forwarded": total_fwd,
            "total_queued": total_q,
            "total_rejected": total_rej,
            "cache_hit_rate_pct": round(hit_rate, 1),
            "avg_cache_hit_latency_ms": round(avg_lat, 2),
            "total_queue_depth": sum(queue_depths.values()),
        },
        "per_agent": per_agent,
        "recent_decisions": recent,
        "cache": cache_layer.stats(),
        "rate_limits": rate_limiter.get_stats(),
    }


# ── Background task: queue cleanup ─────────────────────────────────────────────

@app.on_event("startup")
async def startup_tasks():
    logger.info("Sentinel Guard starting up")
    asyncio.create_task(_queue_cleanup_loop())


async def _queue_cleanup_loop():
    """Periodically clean up expired queue items."""
    while True:
        try:
            removed = queue_manager.cleanup_expired()
            if removed:
                logger.info(f"Cleaned up {removed} expired queue items")
        except Exception as exc:
            logger.error(f"Queue cleanup error: {exc}")
        await asyncio.sleep(30)

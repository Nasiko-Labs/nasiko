import os
import time
from typing import Any

import httpx
from fastapi import FastAPI, Response
from fastapi.responses import JSONResponse
from pydantic import BaseModel

from app import cache, rate
from app.store import decision_log, record_decision, Decision

NASIKO_BASE_URL = os.getenv("NASIKO_BASE_URL", "http://localhost:9100/agents")

app = FastAPI(title="SENTINEL Guard", version="1.0.0")


# ── Request / Response models ──────────────────────────────────────
class ProxyRequest(BaseModel):
    target_agent: str
    query: str
    payload: dict[str, Any]
    endpoint: str = "/run"
    force_refresh: bool = False


class FlushRequest(BaseModel):
    agent_name: str


class LimitRequest(BaseModel):
    agent_name: str
    requests_per_minute: int


class ThresholdRequest(BaseModel):
    agent_name: str
    threshold: float


# ── Health ─────────────────────────────────────────────────────────
@app.get("/health")
async def health():
    return {"status": "healthy", "agent": "sentinel-guard", "version": "1.0.0"}


# ── Main proxy endpoint ───────────────────────────────────────────
@app.post("/proxy")
async def proxy(req: ProxyRequest, response: Response):
    start = time.perf_counter()

    # 1. Semantic cache lookup (unless force_refresh)
    if not req.force_refresh:
        cached_response, similarity = cache.lookup(req.target_agent, req.query)
        if cached_response is not None:
            latency = (time.perf_counter() - start) * 1000
            record_decision(Decision(
                timestamp=time.time(),
                agent=req.target_agent,
                query=req.query[:120],
                outcome="CACHE_HIT",
                similarity=round(similarity, 4),
                latency_ms=round(latency, 2),
            ))
            response.headers["X-Cache"] = "HIT"
            response.headers["X-Similarity"] = str(round(similarity, 4))
            response.headers["X-Latency-Ms"] = str(round(latency, 2))
            response.headers["X-Served-By"] = "sentinel-guard"
            return {
                "source": "cache",
                "similarity": round(similarity, 4),
                "target_agent": req.target_agent,
                "response": cached_response,
                "latency_ms": round(latency, 2),
            }

    # 2. Rate-limit check
    allowed, rate_info = rate.check_and_consume(req.target_agent)
    if not allowed:
        latency = (time.perf_counter() - start) * 1000
        record_decision(Decision(
            timestamp=time.time(),
            agent=req.target_agent,
            query=req.query[:120],
            outcome="RATE_LIMITED",
            queue_position=rate_info["queue_position"],
            estimated_wait_ms=rate_info["estimated_wait_ms"],
            latency_ms=round(latency, 2),
        ))
        return JSONResponse(
            status_code=429,
            content={
                "error": "rate_limit_exceeded",
                "queue_position": rate_info["queue_position"],
                "estimated_wait_ms": rate_info["estimated_wait_ms"],
                "retry_after_seconds": rate_info["retry_after_seconds"],
                "target_agent": req.target_agent,
            },
            headers={
                "X-Queue-Position": str(rate_info["queue_position"]),
                "X-Estimated-Wait-Ms": str(rate_info["estimated_wait_ms"]),
                "Retry-After": str(rate_info["retry_after_seconds"]),
                "X-Served-By": "sentinel-guard",
            },
        )

    # 3. Forward to the actual agent
    target_url = f"{NASIKO_BASE_URL}/{req.target_agent}{req.endpoint}"
    try:
        async with httpx.AsyncClient(timeout=30.0) as client:
            r = await client.post(target_url, json=req.payload)
            r.raise_for_status()
            agent_response = r.json()
    except httpx.HTTPStatusError as e:
        return JSONResponse(
            status_code=e.response.status_code,
            content={
                "error": "upstream_error",
                "target_agent": req.target_agent,
                "detail": str(e),
            },
        )
    except Exception as e:
        return JSONResponse(
            status_code=502,
            content={
                "error": "upstream_unreachable",
                "target_agent": req.target_agent,
                "detail": str(e),
            },
        )

    # 4. Store in cache & return
    cache.store(req.target_agent, req.query, agent_response)
    latency = (time.perf_counter() - start) * 1000
    record_decision(Decision(
        timestamp=time.time(),
        agent=req.target_agent,
        query=req.query[:120],
        outcome="FORWARDED",
        latency_ms=round(latency, 2),
    ))
    response.headers["X-Cache"] = "MISS"
    response.headers["X-Latency-Ms"] = str(round(latency, 2))
    response.headers["X-Served-By"] = "sentinel-guard"
    return {
        "source": "agent",
        "target_agent": req.target_agent,
        "response": agent_response,
        "latency_ms": round(latency, 2),
        "cached": True,
    }


# ── Cache management ──────────────────────────────────────────────
@app.post("/cache/flush")
async def flush_agent_cache(req: FlushRequest):
    count = cache.flush(req.agent_name)
    return {"flushed": True, "agent": req.agent_name, "entries_cleared": count}


@app.post("/cache/flush/all")
async def flush_all_cache():
    return {"flushed": True, "entries_cleared": cache.flush_all()}


@app.get("/cache/stats")
async def cache_stats(agent: str | None = None):
    return cache.get_stats(agent)


@app.post("/cache/threshold")
async def set_threshold(req: ThresholdRequest):
    cache.set_threshold(req.agent_name, req.threshold)
    return {"updated": True, "agent": req.agent_name, "new_threshold": req.threshold}


# ── Rate-limit management ─────────────────────────────────────────
@app.post("/limits/set")
async def set_rate_limit(req: LimitRequest):
    rate.set_limit(req.agent_name, req.requests_per_minute)
    return {"updated": True, "agent": req.agent_name, "new_limit": req.requests_per_minute}


@app.get("/rate/stats")
async def rate_stats(agent: str | None = None):
    return rate.get_stats(agent)


# ── Decision log ──────────────────────────────────────────────────
@app.get("/decisions")
async def decisions():
    return {
        "count": len(decision_log),
        "decisions": [
            {
                "timestamp": d.timestamp,
                "agent": d.agent,
                "query": d.query,
                "outcome": d.outcome,
                "similarity": d.similarity,
                "queue_position": d.queue_position,
                "estimated_wait_ms": d.estimated_wait_ms,
                "latency_ms": d.latency_ms,
            }
            for d in reversed(decision_log)
        ],
    }


# ── Combined stats ────────────────────────────────────────────────
@app.get("/stats")
async def combined_stats():
    cache_s = cache.get_stats()
    rate_s = rate.get_stats()
    all_agents = set(list(cache_s.keys()) + list(rate_s.keys()))
    return {
        "agents": {
            ag: {"cache": cache_s.get(ag, {}), "rate": rate_s.get(ag, {})}
            for ag in all_agents
        },
        "decision_log_size": len(decision_log),
    }

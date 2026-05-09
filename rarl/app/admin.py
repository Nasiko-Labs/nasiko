import asyncio
import time
from typing import Any

import httpx
from fastapi import APIRouter, Query, Request
from pydantic import BaseModel
from starlette.responses import JSONResponse

router = APIRouter(prefix="/admin", tags=["admin"])


class AgentConfig(BaseModel):
    rps: float | None = None
    burst: int | None = None
    ttl: int | None = None
    max_inflight: int | None = None


class SpikeTestRequest(BaseModel):
    agent_id: str = "translator"
    concurrency: int = 200
    requests: int = 200
    vary: bool = False
    endpoint: str = "/health"
    payload: dict[str, Any] = {}


class AdaptiveToggle(BaseModel):
    enabled: bool


def _lane_stats(lane) -> dict[str, Any]:
    return {
        "agent_id": lane.agent_id,
        "rps": lane.bucket.rate,
        "burst": lane.bucket.capacity,
        "tokens": round(lane.bucket.tokens, 2),
        "queue_depth": lane.queue_depth,
        "max_inflight": lane.max_inflight,
        "p95_ms": round(lane.p95 * 1000, 2),
        "ema_ms": round(lane.ema * 1000, 2),
        "served": lane.served,
        "queued_total": lane.queued_total,
        "rejected": lane.rejected,
    }


@router.get("/agents")
async def list_agents(request: Request) -> JSONResponse:
    lanes = request.app.state.lanes
    return JSONResponse([_lane_stats(l) for l in lanes.values()])


@router.get("/agents/{agent_id}")
async def agent_detail(agent_id: str, request: Request) -> JSONResponse:
    lanes = request.app.state.lanes
    if agent_id not in lanes:
        return JSONResponse({"error": "unknown agent"}, status_code=404)
    return JSONResponse(_lane_stats(lanes[agent_id]))


@router.put("/agents/{agent_id}/config")
async def update_agent_config(
    agent_id: str, config: AgentConfig, request: Request
) -> JSONResponse:
    from app.main import get_or_create_lane

    lane = get_or_create_lane(request.app, agent_id)

    if config.rps is not None:
        lane.bucket.rate = float(config.rps)
    if config.burst is not None:
        lane.bucket.capacity = int(config.burst)
        lane.bucket.tokens = min(lane.bucket.tokens, lane.bucket.capacity)
    if config.max_inflight is not None:
        lane.semaphore = asyncio.Semaphore(config.max_inflight)
        lane.max_inflight = config.max_inflight

    return JSONResponse({"updated": True, **_lane_stats(lane)})


@router.post("/cache/purge")
async def purge_cache(request: Request, agent: str | None = Query(None)) -> JSONResponse:
    purged = await request.app.state.cache.purge(agent)
    return JSONResponse({"purged": purged})


@router.get("/cache/stats")
async def cache_stats(request: Request) -> JSONResponse:
    cache = request.app.state.cache
    sf = request.app.state.singleflight
    keys = await cache.keys_count()
    return JSONResponse({
        "hits": cache.hits,
        "misses": cache.misses,
        "hit_rate": round(cache.hit_rate, 4),
        "coalesced": sf.coalesced_count,
        "keys_count": keys,
    })


@router.get("/explain")
async def explain(
    request: Request,
    request_id: str | None = Query(None),
    last: int = Query(1),
) -> JSONResponse:
    decisions: list[dict] = list(request.app.state.recent_decisions)
    settings_rps_burst = {
        aid: {"rps": l.bucket.rate, "burst": l.bucket.capacity}
        for aid, l in request.app.state.lanes.items()
    }

    if request_id:
        match = next((d for d in reversed(decisions) if d.get("request_id") == request_id), None)
        if not match:
            return JSONResponse({"error": "not found"}, status_code=404)
        return JSONResponse(_enrich_decision(match, settings_rps_burst))

    recent = decisions[-last:] if last > 0 else decisions
    return JSONResponse([_enrich_decision(d, settings_rps_burst) for d in reversed(recent)])


def _enrich_decision(d: dict, rps_burst: dict) -> dict:
    out = dict(d)
    out["timestamp_iso"] = time.strftime(
        "%Y-%m-%dT%H:%M:%SZ", time.gmtime(d.get("timestamp", 0))
    )
    cache = d.get("cache", "")
    pos = d.get("queue_position", 0)
    eta = d.get("eta", 0.0)
    agent_id = d.get("agent_id", "")
    rate_info = rps_burst.get(agent_id, {})

    if cache == "HIT":
        reason = "Served from Redis cache — zero upstream calls made"
    elif cache == "COALESCED":
        reason = (
            f"Coalesced into in-flight request — saved 1 agent call. "
            f"Queue position: {pos}, ETA was {eta:.2f}s"
        )
    elif cache == "MISS":
        if pos == 0:
            reason = "Forwarded directly — bucket had tokens, queue empty"
        else:
            reason = f"Queued at position {pos}, ETA {eta:.2f}s"
    elif cache == "QUEUE_OVERFLOW":
        reason = "Rejected — queue was full at time of request"
    else:
        reason = "Forwarded directly — non-cacheable method"

    out["reason"] = reason
    out["agent_rate_at_decision"] = rate_info
    return out


@router.post("/spike-test")
async def spike_test(body: SpikeTestRequest, request: Request) -> JSONResponse:
    concurrency = min(body.concurrency, 500)
    n_requests = min(body.requests, 1000)
    url = f"http://localhost:8010/agents/{body.agent_id}{body.endpoint}"
    start = time.monotonic()

    async def fire(i: int) -> dict:
        payload = dict(body.payload)
        if body.vary:
            payload["_seq"] = i
        t0 = time.monotonic()
        try:
            async with httpx.AsyncClient(timeout=60.0) as client:
                resp = await client.post(url, json=payload if payload else None)
            return {
                "status": resp.status_code,
                "latency_ms": (time.monotonic() - t0) * 1000,
                "x_cache": resp.headers.get("x-cache", "NONE"),
            }
        except Exception as e:
            return {"status": 0, "latency_ms": (time.monotonic() - t0) * 1000, "x_cache": "ERROR", "error": str(e)}

    sem = asyncio.Semaphore(concurrency)

    async def bounded(i: int) -> dict:
        async with sem:
            return await fire(i)

    results = await asyncio.gather(*[bounded(i) for i in range(n_requests)])
    duration = time.monotonic() - start

    status_codes: dict[str, int] = {}
    x_cache_breakdown: dict[str, int] = {}
    latencies = []
    for r in results:
        k = str(r["status"])
        status_codes[k] = status_codes.get(k, 0) + 1
        xc = r["x_cache"]
        x_cache_breakdown[xc] = x_cache_breakdown.get(xc, 0) + 1
        latencies.append(r["latency_ms"])

    latencies.sort()
    n = len(latencies)

    def pct(p: float) -> float:
        return latencies[max(0, int(p * n) - 1)] if latencies else 0.0

    return JSONResponse({
        "status_codes": status_codes,
        "x_cache_breakdown": x_cache_breakdown,
        "p50_ms": round(pct(0.50), 2),
        "p95_ms": round(pct(0.95), 2),
        "p99_ms": round(pct(0.99), 2),
        "max_ms": round(max(latencies) if latencies else 0, 2),
        "duration_s": round(duration, 3),
    })


@router.post("/adaptive")
async def toggle_adaptive(body: AdaptiveToggle, request: Request) -> JSONResponse:
    from app.main import adaptive_tuner
    from app.config import get_settings

    settings = get_settings()
    if body.enabled and not hasattr(request.app.state, "adaptive_task"):
        request.app.state.adaptive_task = asyncio.create_task(
            adaptive_tuner(request.app, settings.target_p95_latency)
        )
        return JSONResponse({"adaptive": "enabled"})
    elif not body.enabled and hasattr(request.app.state, "adaptive_task"):
        request.app.state.adaptive_task.cancel()
        del request.app.state.adaptive_task
        return JSONResponse({"adaptive": "disabled"})
    return JSONResponse({"adaptive": "enabled" if body.enabled else "disabled"})

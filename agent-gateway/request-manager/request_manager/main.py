from __future__ import annotations

import httpx
import redis.asyncio as redis
from fastapi import Body, FastAPI, Query, Request

from request_manager import redis_keys
from request_manager.cache import CachePolicy, RedisResponseCache
from request_manager.circuit_breaker import CircuitBreaker
from request_manager.dashboard import dashboard_html
from request_manager.limiter import RequestLimiter
from request_manager.metrics import MetricsRecorder
from request_manager.models import AgentLimits
from request_manager.proxy import RequestProxy
from request_manager.settings import get_settings
from request_manager.singleflight import SingleFlight
from request_manager.target_resolver import LimitResolver, TargetResolver

settings = get_settings()

app = FastAPI(title="Nasiko Request Manager", version="0.1.0")


@app.on_event("startup")
async def startup() -> None:
    redis_client = redis.from_url(
        settings.redis_url,
        decode_responses=True,
        socket_timeout=settings.redis_timeout_seconds,
        socket_connect_timeout=settings.redis_timeout_seconds,
    )
    http_client = httpx.AsyncClient(timeout=settings.upstream_timeout_seconds)
    app.state.redis = redis_client
    app.state.http_client = http_client
    app.state.metrics = MetricsRecorder(redis_client)
    app.state.limit_resolver = LimitResolver(redis_client, settings)
    app.state.proxy = RequestProxy(
        target_resolver=TargetResolver(redis_client),
        limit_resolver=app.state.limit_resolver,
        cache=RedisResponseCache(redis_client),
        cache_policy=CachePolicy(),
        singleflight=SingleFlight(redis_client, settings.singleflight_wait_ms),
        limiter=RequestLimiter(redis_client, settings.global_active_cap),
        circuit_breaker=CircuitBreaker(
            redis_client,
            window_size=settings.circuit_window_size,
            min_failures=settings.circuit_min_failures,
            failure_ratio=settings.circuit_failure_ratio,
            open_seconds=settings.circuit_open_seconds,
        ),
        metrics=app.state.metrics,
        http_client=http_client,
    )


@app.on_event("shutdown")
async def shutdown() -> None:
    await app.state.http_client.aclose()
    await app.state.redis.aclose()


@app.get("/")
async def dashboard():
    return dashboard_html()


@app.get("/health")
async def health() -> dict[str, object]:
    redis_available = False
    circuits = {}
    try:
        redis_available = bool(await app.state.redis.ping())
        for agent_id in await _agent_ids():
            circuits[agent_id] = await _agent_circuit_state(agent_id)
    except Exception:
        redis_available = False
    return {
        "status": "healthy" if redis_available else "degraded",
        "service": settings.service_name,
        "redis_available": redis_available,
        "circuits": circuits,
    }


@app.get("/control/stats")
async def control_stats():
    agents = []
    for agent_id in await _agent_ids():
        limits = await app.state.limit_resolver.resolve(agent_id)
        agents.append(await app.state.metrics.agent_stats(agent_id, limits, await _agent_circuit_state(agent_id)))
    redis_available = False
    try:
        redis_available = bool(await app.state.redis.ping())
    except Exception:
        redis_available = False
    return await app.state.metrics.global_stats(redis_available, agents)


@app.get("/control/agents/{agent_id}/stats")
async def agent_stats(agent_id: str):
    limits = await app.state.limit_resolver.resolve(agent_id)
    return await app.state.metrics.agent_stats(agent_id, limits, await _agent_circuit_state(agent_id))


@app.get("/control/limits")
async def control_limits():
    return {
        agent_id: (await app.state.limit_resolver.resolve(agent_id)).model_dump()
        for agent_id in await _agent_ids()
    }


@app.put("/control/limits/{agent_id}")
async def update_limits(agent_id: str, limits: AgentLimits = Body()):
    return await app.state.limit_resolver.update(agent_id, limits)


@app.delete("/control/cache")
async def clear_cache(agent: str | None = Query(default=None)):
    cleared = await app.state.proxy.cache.clear(agent_id=agent)
    return {"cleared": cleared, "agent": agent}


@app.api_route("/agents/{path:path}", methods=["GET", "POST", "PUT", "DELETE", "PATCH", "OPTIONS"])
async def proxy_agents(request: Request):
    return await app.state.proxy.handle(request)


async def _agent_ids() -> list[str]:
    try:
        return sorted(await app.state.redis.smembers(redis_keys.targets_index()))
    except Exception:
        return []


async def _agent_circuit_state(agent_id: str) -> str:
    try:
        circuit = await app.state.redis.hgetall(redis_keys.circuit(agent_id))
        return circuit.get("state", "closed")
    except Exception:
        return "degraded"

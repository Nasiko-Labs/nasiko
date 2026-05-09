import collections
from contextlib import asynccontextmanager

import redis.asyncio as aioredis
import uvicorn
from fastapi import FastAPI, Request
from starlette.responses import Response

from app.config import get_settings


def get_or_create_lane(app: FastAPI, agent_id: str):
    from app.queue_lane import AgentLane

    if agent_id not in app.state.lanes:
        s = get_settings()
        app.state.lanes[agent_id] = AgentLane(
            agent_id=agent_id,
            rps=s.default_rps,
            burst=s.default_burst,
            max_inflight=s.default_max_inflight,
            max_queue=s.default_max_queue,
        )
    return app.state.lanes[agent_id]


@asynccontextmanager
async def lifespan(app: FastAPI):
    from app.cache import RedisCache
    from app.coalescer import SingleFlight
    from app.metrics import Metrics

    settings = get_settings()
    redis_client = aioredis.from_url(settings.redis_url, decode_responses=False)
    app.state.redis = redis_client
    app.state.cache = RedisCache(redis_client)
    app.state.singleflight = SingleFlight()
    app.state.lanes = {}
    app.state.metrics = Metrics()
    app.state.recent_decisions: collections.deque = collections.deque(maxlen=50)

    metrics_task = app.state.metrics.start_tick_task()

    if settings.adaptive_enabled:
        import asyncio
        app.state.adaptive_task = asyncio.create_task(
            adaptive_tuner(app, settings.target_p95_latency)
        )

    yield

    metrics_task.cancel()
    if hasattr(app.state, "adaptive_task"):
        app.state.adaptive_task.cancel()
    await redis_client.aclose()


async def adaptive_tuner(app: FastAPI, target_p95: float = 1.0) -> None:
    import asyncio

    while True:
        await asyncio.sleep(10)
        for lane in app.state.lanes.values():
            if len(lane.latencies) < 10:
                continue
            observed_p95 = lane.p95
            ratio = max(0.5, min(1.5, target_p95 / max(observed_p95, 0.05)))
            new_rate = max(1.0, lane.bucket.rate * ratio)
            lane.bucket.rate = new_rate
            lane.bucket.capacity = max(2, int(new_rate * 2))


app = FastAPI(title="RARL", version="0.1.0", lifespan=lifespan)


@app.get("/health")
async def health() -> dict:
    return {"status": "ok", "service": "rarl"}


@app.api_route(
    "/agents/{agent_id}/{path:path}",
    methods=["GET", "POST", "PUT", "DELETE", "PATCH"],
)
async def proxy(agent_id: str, path: str, request: Request) -> Response:
    from app.proxy import handle_request

    return await handle_request(agent_id, path, request)


def _include_routers() -> None:
    from app.admin import router as admin_router
    from app.dashboard import router as dashboard_router

    app.include_router(admin_router)
    app.include_router(dashboard_router)


_include_routers()


if __name__ == "__main__":
    uvicorn.run("app.main:app", host="0.0.0.0", port=8010, log_level="info")

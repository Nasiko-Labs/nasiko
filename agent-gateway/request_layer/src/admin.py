"""Admin REST API and SSE event sink."""
import asyncio
import logging
from collections import deque
from datetime import datetime, timezone

from fastapi import APIRouter, HTTPException, Request
from fastapi.responses import JSONResponse, StreamingResponse
from pydantic import BaseModel, Field

from request_layer.src.cache import exact as exact_cache
from request_layer.src.cache import router_cache
from request_layer.src.cache import semantic as semantic_cache
from request_layer.src.cache.policy import set_override
from request_layer.src.queue import depth, predicted_wait_ms
from request_layer.src.types import CacheEvent, RecommendationItem

logger = logging.getLogger(__name__)


class EventSink:
    """In-process fan-out for SSE consumers.

    Each subscriber gets its own ``asyncio.Queue``; producers call
    :meth:`publish` and every subscriber receives a copy.
    """

    def __init__(self, max_buffer: int = 200) -> None:
        self._max_buffer = max_buffer
        self._subscribers: list[asyncio.Queue[CacheEvent]] = []
        self._history: deque[CacheEvent] = deque(maxlen=max_buffer)

    def publish(self, event: CacheEvent) -> None:
        self._history.append(event)
        for queue in list(self._subscribers):
            try:
                queue.put_nowait(event)
            except asyncio.QueueFull:
                # Drop oldest to keep up with a slow subscriber.
                try:
                    queue.get_nowait()
                except asyncio.QueueEmpty:
                    pass
                queue.put_nowait(event)

    def subscribe(self) -> asyncio.Queue[CacheEvent]:
        queue: asyncio.Queue[CacheEvent] = asyncio.Queue(maxsize=self._max_buffer)
        for past in self._history:
            queue.put_nowait(past)
        self._subscribers.append(queue)
        return queue

    def unsubscribe(self, queue: asyncio.Queue[CacheEvent]) -> None:
        try:
            self._subscribers.remove(queue)
        except ValueError:
            pass


class PolicyOverride(BaseModel):
    cache_ttl_seconds: int | None = Field(default=None, ge=1)
    semantic_threshold: float | None = Field(default=None, ge=0.0, le=1.0)
    rps_limit: int | None = Field(default=None, ge=0)
    cost_cap_usd_per_min: float | None = Field(default=None, ge=0.0)


class CacheClearBody(BaseModel):
    layer: str = Field(default="all", pattern="^(all|L1|L2|L3)$")
    agent: str | None = None


def build_admin_router() -> APIRouter:
    """Build the admin sub-router. Pipeline is resolved from ``request.app``."""

    router = APIRouter(prefix="/admin", tags=["admin"])

    @router.get("/stats")
    async def stats(request: Request) -> JSONResponse:
        pipeline = request.app.state.pipeline
        agents = list(pipeline.adapter.policies.keys())
        per_agent_depth: dict[str, dict[str, int]] = {}
        for agent in agents:
            per_agent_depth[agent] = await depth(pipeline.redis, agent)
        return JSONResponse(
            {
                "counters": pipeline.counters,
                "agents": agents,
                "queue_depth": per_agent_depth,
                "router_cache_enabled": pipeline.settings.request_layer_router_cache_enabled,
            }
        )

    @router.get("/stream")
    async def stream(request: Request) -> StreamingResponse:
        pipeline = request.app.state.pipeline
        sink: EventSink = pipeline.event_sink
        queue = sink.subscribe()

        async def generate():
            try:
                while True:
                    if await request.is_disconnected():
                        return
                    try:
                        event = await asyncio.wait_for(queue.get(), timeout=15.0)
                        yield f"data: {event.model_dump_json()}\n\n"
                    except asyncio.TimeoutError:
                        # Heartbeat to keep proxies happy.
                        yield ": heartbeat\n\n"
            finally:
                sink.unsubscribe(queue)

        return StreamingResponse(generate(), media_type="text/event-stream")

    @router.get("/policies")
    async def list_policies(request: Request) -> JSONResponse:
        pipeline = request.app.state.pipeline
        return JSONResponse(
            {
                agent: policy.model_dump()
                for agent, policy in pipeline.adapter.policies.items()
            }
        )

    @router.get("/policies/{agent}")
    async def get_policy(agent: str, request: Request) -> JSONResponse:
        pipeline = request.app.state.pipeline
        policy = pipeline.adapter.policies.get(agent)
        if policy is None:
            raise HTTPException(status_code=404, detail=f"agent {agent!r} not registered")
        return JSONResponse(policy.model_dump())

    @router.patch("/policies/{agent}")
    async def patch_policy(
        agent: str,
        override: PolicyOverride,
        request: Request,
    ) -> JSONResponse:
        pipeline = request.app.state.pipeline
        if agent not in pipeline.adapter.policies:
            raise HTTPException(status_code=404, detail=f"agent {agent!r} not registered")
        partial = override.model_dump(exclude_none=True)
        if not partial:
            raise HTTPException(status_code=400, detail="no fields supplied")
        await set_override(pipeline.redis, agent, partial)
        return JSONResponse({"agent": agent, "override": partial})

    @router.post("/cache/clear")
    async def cache_clear(body: CacheClearBody, request: Request) -> JSONResponse:
        pipeline = request.app.state.pipeline
        deleted = 0
        if body.layer in {"all", "L1"}:
            deleted += await exact_cache.clear(pipeline.redis, body.agent)
        if body.layer in {"all", "L2"}:
            deleted += await semantic_cache.clear(pipeline.redis, body.agent)
        if body.layer in {"all", "L3"}:
            deleted += await router_cache.invalidate_all(pipeline.redis)
        return JSONResponse({"deleted": deleted, "layer": body.layer, "agent": body.agent})

    @router.get("/queue/{agent}")
    async def queue_status(agent: str, request: Request) -> JSONResponse:
        pipeline = request.app.state.pipeline
        sizes = await depth(pipeline.redis, agent)
        # Use a coarse 200ms p95 heuristic until per-agent latency is tracked.
        wait_ms = predicted_wait_ms(
            queue_depth=sum(v for k, v in sizes.items() if k != "processing"),
            p95_latency_ms=200.0,
            parallelism=1,
        )
        return JSONResponse({"agent": agent, "depth": sizes, "predicted_wait_ms": wait_ms})

    @router.get("/recommendations")
    async def recommendations(request: Request) -> JSONResponse:
        pipeline = request.app.state.pipeline
        items = await _generate_recommendations(pipeline)
        return JSONResponse({"recommendations": [item.model_dump() for item in items]})

    return router


async def _generate_recommendations(pipeline) -> list[RecommendationItem]:
    """Very simple advisor: surface obvious tuning suggestions.

    A future iteration would consume Redis-side counters; for now we keep
    this lightweight and deterministic so reviewers can audit it easily.
    """

    items: list[RecommendationItem] = []
    counters = pipeline.counters
    requests = max(1, counters.get("requests_total", 0))
    hit_rate = (counters.get("hits_l1", 0) + counters.get("hits_l2", 0)) / requests
    if requests >= 50 and hit_rate < 0.1:
        for agent, policy in pipeline.adapter.policies.items():
            items.append(
                RecommendationItem(
                    id=f"{agent}-threshold",
                    agent=agent,
                    field="semantic_threshold",
                    current_value=policy.semantic_threshold,
                    suggested_value=max(0.85, policy.semantic_threshold - 0.03),
                    reason=(
                        f"observed hit rate {hit_rate:.0%} over {int(requests)} "
                        "requests; loosening the threshold may reclaim paraphrases"
                    ),
                    generated_at=datetime.now(timezone.utc),
                )
            )
    return items

"""FastAPI entry point for the request management layer."""
import asyncio
import logging
from contextlib import asynccontextmanager
from typing import AsyncIterator

import redis.asyncio as redis_async
from fastapi import FastAPI, HTTPException, Request, Response

from request_layer.src import embedding, phoenix
from request_layer.src.admin import EventSink, build_admin_router
from request_layer.src.agentcard import NasikoAdapter
from request_layer.src.config import Settings, get_settings
from request_layer.src.forward import Forwarder
from request_layer.src.proxy import ProxyPipeline

logger = logging.getLogger(__name__)


@asynccontextmanager
async def lifespan(app: FastAPI) -> AsyncIterator[None]:
    """Wire dependencies, warm the embedding model, start background tasks."""

    settings: Settings = get_settings()
    logging.basicConfig(
        level=getattr(logging, settings.request_layer_log_level.upper(), logging.INFO),
        format="%(asctime)s %(levelname)s %(name)s: %(message)s",
    )

    # Tracing — uses Nasiko's shared bootstrapper when available.
    if settings.request_layer_tracing_enabled:
        try:
            phoenix.bootstrap(
                project_name=settings.request_layer_phoenix_project,
                endpoint=settings.request_layer_phoenix_endpoint,
            )
        except Exception:  # noqa: BLE001 — tracing must not block startup
            logger.exception("phoenix bootstrap failed; continuing without tracing")

    # Embedding model — load on a worker thread so the event loop stays responsive.
    await asyncio.to_thread(embedding.load_model, settings.request_layer_embedding_model)

    # Redis (Request layer owns its own redis-stack instance for vector search).
    redis = redis_async.from_url(settings.request_layer_redis_url, decode_responses=False)
    await redis.ping()

    forwarder = Forwarder(
        timeout_seconds=settings.request_layer_forward_timeout_seconds,
        max_connections=settings.request_layer_forward_max_connections,
    )

    adapter = NasikoAdapter(settings)
    await adapter.refresh()
    poll_task = asyncio.create_task(adapter.poll_loop(), name="request_layer-registry-poll")

    sink = EventSink(max_buffer=settings.request_layer_admin_stream_max_events)

    pipeline = ProxyPipeline(
        settings=settings,
        redis=redis,
        forwarder=forwarder,
        adapter=adapter,
        event_sink=sink,
    )

    app.state.settings = settings
    app.state.redis = redis
    app.state.adapter = adapter
    app.state.pipeline = pipeline
    app.state.forwarder = forwarder
    app.state.event_sink = sink

    logger.info("request_layer ready (port=%s)", settings.request_layer_port)

    try:
        yield
    finally:
        poll_task.cancel()
        try:
            await poll_task
        except (asyncio.CancelledError, Exception):  # noqa: BLE001
            pass
        await forwarder.close()
        await adapter.close()
        try:
            await redis.aclose()
        except Exception:  # noqa: BLE001
            pass


app = FastAPI(
    title="Request layer",
    description="Resilient Agent Request Layer for Nasiko",
    version="0.1.0",
    lifespan=lifespan,
)
app.include_router(build_admin_router())


@app.get("/health")
async def health(request: Request) -> dict:
    """Liveness probe used by Docker healthcheck and Kong."""

    state = request.app.state
    redis_ok = True
    try:
        await state.redis.ping()
    except Exception:  # noqa: BLE001
        redis_ok = False
    return {
        "status": "healthy" if redis_ok else "degraded",
        "model_loaded": embedding.is_loaded(),
        "redis_connected": redis_ok,
        "adapter": "nasiko",
        "agents": len(state.adapter.policies),
        "router_cache_enabled": state.settings.request_layer_router_cache_enabled,
    }


@app.api_route(
    "/router/route",
    methods=["GET", "POST"],
    include_in_schema=False,
)
async def router_short_circuit(request: Request) -> Response:
    """L3 short-circuit. Returns 404-equivalent if the cache misses."""

    pipeline: ProxyPipeline = request.app.state.pipeline
    response = await pipeline.handle_router_request(request)
    if response is None:
        # On miss, signal to Kong that this request should fall through to
        # the regular router service. We return a 404 with a hint header so
        # Kong can be configured to retry against the upstream router.
        return Response(
            status_code=404,
            headers={"X-Request layer-Layer": "miss", "X-Request layer-Fallthrough": "router"},
        )
    return response


@app.api_route(
    "/{agent}/{path:path}",
    methods=["GET", "POST", "PUT", "PATCH", "DELETE"],
    include_in_schema=False,
)
async def agent_proxy(agent: str, path: str, request: Request) -> Response:
    """Catch-all that drives every inbound request through the pipeline."""

    if agent.startswith("admin") or agent in {"health", "metrics"}:
        raise HTTPException(status_code=404, detail="not an agent path")
    pipeline: ProxyPipeline = request.app.state.pipeline
    return await pipeline.handle_agent_request(agent, path, request)

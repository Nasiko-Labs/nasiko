import logging
from contextlib import asynccontextmanager

import httpx
import redis.asyncio as aioredis
from fastapi import FastAPI, Request, Response
from fastapi.responses import JSONResponse
from fastapi.staticfiles import StaticFiles

from src.config import settings
from src.monitor import router as monitor_router

logging.basicConfig(
    level=getattr(logging, settings.LOG_LEVEL),
    format="%(asctime)s [%(levelname)s] %(name)s: %(message)s",
)
logger = logging.getLogger(__name__)

_drain_workers: list = []
_ghost_workers: list = []
_detector: object = None


@asynccontextmanager
async def lifespan(app: FastAPI):
    global _detector

    logger.info("request-layer starting...")

    app.state.http_client = httpx.AsyncClient(timeout=settings.REQUEST_TIMEOUT)
    app.state.redis = aioredis.from_url(settings.REDIS_URL, decode_responses=False)

    try:
        from src.embeddings import load_model
        load_model()
    except Exception as exc:
        logger.warning(f"embedding model load failed — cache disabled: {exc}")

    from src.detector import DetectorLoop
    _detector = DetectorLoop(app.state.redis)
    _detector.start()

    try:
        from src.kong_client import register_request_layer
        register_request_layer()
    except Exception as exc:
        logger.warning(f"Kong registration skipped: {exc}")

    # Store app ref for drain workers to access http_client
    app.state.app_ref = app

    logger.info("request-layer ready")
    yield

    logger.info("request-layer shutting down...")

    if _detector:
        _detector.stop()
    for w in _drain_workers + _ghost_workers:
        w.stop()

    await app.state.redis.aclose()
    await app.state.http_client.aclose()


app = FastAPI(
    title="Nasiko Request Layer",
    description="Resilient agent request layer — semantic cache, slot queue, demand detector, reallocation engine.",
    version="1.0.0",
    lifespan=lifespan,
)

app.include_router(monitor_router)
app.mount("/static", StaticFiles(directory="src/static"), name="static")


@app.get("/health")
async def health(request: Request):
    try:
        await request.app.state.redis.ping()
        redis_ok = True
    except Exception:
        redis_ok = False
    return {
        "status": "ok" if redis_ok else "degraded",
        "service": "request-layer",
        "redis": redis_ok,
    }


@app.get("/")
async def dashboard_redirect():
    from fastapi.responses import RedirectResponse
    return RedirectResponse(url="/static/dashboard.html")


@app.get("/status/{job_id}")
async def job_status(job_id: str, request: Request):
    from src.queue import get_job_result
    result = await get_job_result(request.app.state.redis, job_id)
    if result is None:
        return JSONResponse(status_code=200, content={"status": "pending", "job_id": job_id})
    return JSONResponse(status_code=200, content=result)


async def _forward_request(method: str, url: str, headers: dict, body: bytes) -> tuple[int, bytes, dict]:
    """Forward a request to an upstream agent. Used by drain/ghost workers."""
    async with httpx.AsyncClient(timeout=settings.REQUEST_TIMEOUT) as client:
        resp = await client.request(method=method, url=url, headers=headers, content=body)
        resp_headers = {k: v for k, v in resp.headers.items()}
        return resp.status_code, resp.content, resp_headers


def _ensure_drain_worker(redis_conn: aioredis.Redis, agent_name: str) -> None:
    """Lazily start a DrainWorker + GhostQueueDrainWorker for an agent."""
    known = {getattr(w, '_agent_name', None) for w in _drain_workers}
    if agent_name in known:
        return
    from src.queue import DrainWorker
    from src.ghost_queue import GhostQueueDrainWorker
    dw = DrainWorker(redis_conn, agent_name, _forward_request)
    dw.start()
    _drain_workers.append(dw)
    gw = GhostQueueDrainWorker(redis_conn, agent_name, _forward_request)
    gw.start()
    _ghost_workers.append(gw)
    logger.info(f"started drain + ghost workers for {agent_name}")


@app.api_route(
    "/{full_path:path}",
    methods=["GET", "POST", "PUT", "DELETE", "PATCH", "OPTIONS", "HEAD"],
)
async def proxy(request: Request, full_path: str):
    agent_name, target_base, forward_path = _resolve_target(full_path)

    if target_base is None:
        return JSONResponse(
            status_code=404,
            content={"detail": f"no upstream for path /{full_path}"},
        )

    redis: aioredis.Redis = request.app.state.redis
    client: httpx.AsyncClient = request.app.state.http_client

    # Start drain workers for this agent if not already running
    if agent_name:
        _ensure_drain_worker(redis, agent_name)

    # Async port probe on first request for this agent (non-blocking)
    if agent_name and agent_name not in _port_cache:
        port = await _probe_and_cache_port(agent_name, client)
        target_base = f"http://{agent_name}:{port}"

    body = await request.body()
    headers = {k: v for k, v in request.headers.items() if k.lower() != "host"}

    upstream_url = f"{target_base}/{forward_path}".rstrip("/") or target_base
    if request.url.query:
        upstream_url = f"{upstream_url}?{request.url.query}"

    input_text = _extract_input_text(body, request)

    if agent_name and input_text:
        try:
            from src.cache import get_cached_response
            cached = await get_cached_response(redis, agent_name, input_text)
            if cached is not None:
                return Response(
                    content=cached["body"].encode(),
                    status_code=cached["status_code"],
                    headers={"X-Cache": "HIT"},
                    media_type="application/json",
                )
        except Exception as exc:
            logger.warning(f"cache lookup failed (skipping): {exc}")

    if agent_name:
        hibernated = await redis.exists(f"fleet:hibernated:{agent_name}")
        if hibernated:
            from src.queue import generate_job_id
            from src.ghost_queue import enqueue_ghost
            job_id = generate_job_id()
            await enqueue_ghost(redis, agent_name, job_id, request.method, upstream_url, headers, body)
            return JSONResponse(
                status_code=202,
                content={"job_id": job_id, "status": "queued", "reason": "agent hibernated"},
            )

    if agent_name:
        from src.queue import claim_slot, release_slot, enqueue_request, estimate_wait_seconds, generate_job_id, init_slots
        await init_slots(redis, agent_name)
        claimed = await claim_slot(redis, agent_name)
        if not claimed:
            job_id = generate_job_id()
            wait = await estimate_wait_seconds(redis, agent_name)
            await enqueue_request(redis, agent_name, job_id, request.method, upstream_url, headers, body)
            return JSONResponse(
                status_code=202,
                content={"job_id": job_id, "estimated_wait_s": wait, "status": "queued"},
            )

    try:
        upstream_resp = await client.request(
            method=request.method,
            url=upstream_url,
            headers=headers,
            content=body,
        )
    except httpx.RequestError as exc:
        logger.error(f"upstream error {upstream_url}: {exc}")
        if agent_name:
            from src.queue import release_slot
            await release_slot(redis, agent_name)
        return JSONResponse(status_code=502, content={"detail": "upstream unavailable"})

    if agent_name:
        from src.queue import release_slot
        await release_slot(redis, agent_name)
        if input_text and upstream_resp.status_code < 400:
            try:
                from src.cache import store_response
                await store_response(redis, agent_name, input_text, upstream_resp.content, upstream_resp.status_code)
            except Exception as exc:
                logger.warning(f"cache store failed (skipping): {exc}")

    logger.info(f"proxy {request.method} /{full_path} -> {upstream_resp.status_code}")

    return Response(
        content=upstream_resp.content,
        status_code=upstream_resp.status_code,
        headers={"X-Cache": "MISS", **dict(upstream_resp.headers)},
        media_type=upstream_resp.headers.get("content-type"),
    )





def _resolve_target(path: str) -> tuple[str | None, str | None, str]:
    """
    Returns (agent_name, base_url, forward_path).
    Strips the /agents/{name} prefix so agents receive requests at their own root.
    Port is resolved dynamically from Kong, then Docker, then defaults to 5000.
    """
    if path.startswith("agents/"):
        parts = path.split("/", 2)
        if len(parts) >= 2:
            agent_name = parts[1]
            forward_path = parts[2] if len(parts) == 3 else ""
            container_name = f"agent-{agent_name}" if not agent_name.startswith("agent-") else agent_name
            port = _resolve_agent_port(container_name)
            return container_name, f"http://{container_name}:{port}", forward_path
    if path.startswith("router"):
        forward_path = path[len("router"):].lstrip("/")
        return None, "http://nasiko-router:8000", forward_path
    return None, None, path


_port_cache: dict[str, int] = {}


def _port_from_kong(agent_name: str) -> int | None:
    """Try to resolve the agent port from Kong service config."""
    try:
        import requests as sync_requests
        resp = sync_requests.get(
            f"{settings.KONG_ADMIN_URL}/services/agent-{agent_name}",
            timeout=3,
        )
        if resp.status_code == 200:
            url = resp.json().get("url", "")
            # url looks like "http://agent-name:PORT"
            if ":" in url.rsplit("/", 1)[-1]:
                return int(url.rsplit(":", 1)[-1].rstrip("/"))
    except Exception:
        pass
    return None


def _resolve_agent_port(agent_name: str) -> int:
    """
    Resolve the internal port the agent container listens on.
    Priority: cached → Kong service config → default 5000.
    Port probing is done async — see _probe_and_cache_port.
    """
    if agent_name in _port_cache:
        return _port_cache[agent_name]
    port = _port_from_kong(agent_name)
    if port:
        _port_cache[agent_name] = port
        return port
    return 5000


async def _probe_and_cache_port(agent_name: str, client: httpx.AsyncClient) -> int:
    """
    Async port probe — tries common A2A ports until agent card responds.
    Result is cached in _port_cache for subsequent requests.
    """
    if agent_name in _port_cache:
        return _port_cache[agent_name]
    candidates = [5000, 10002, 10007, 10008, 8080, 8000]
    for port in candidates:
        try:
            r = await client.get(
                f"http://{agent_name}:{port}/.well-known/agent.json",
                timeout=1.0,
            )
            if r.status_code == 200:
                _port_cache[agent_name] = port
                return port
        except Exception:
            continue
    _port_cache[agent_name] = 5000
    return 5000


def _extract_input_text(body: bytes, request: Request) -> str | None:
    content_type = request.headers.get("content-type", "")
    if not body:
        return None
    if "application/json" in content_type:
        import json
        try:
            data = json.loads(body)
            for field in ("message", "query", "text", "input", "content", "params"):
                if isinstance(data.get(field), str):
                    return data[field]
            return str(data)
        except Exception:
            return None
    if "text/plain" in content_type:
        return body.decode(errors="replace")
    return None

import asyncio
import base64
import importlib
import json
import os
import time
from contextlib import asynccontextmanager, suppress
from typing import Any
from urllib.parse import urlencode

import httpx
import redis.asyncio as redis
from dotenv import load_dotenv
from fastapi import FastAPI, HTTPException, Request, Response


load_dotenv()

REDIS_HOST = os.getenv("REDIS_HOST", "localhost")
REDIS_PORT = int(os.getenv("REDIS_PORT", "6379"))
CACHE_TTL = int(os.getenv("CACHE_TTL", "300"))
RATE_LIMIT_REQUESTS = int(os.getenv("RATE_LIMIT_REQUESTS", "10"))
RATE_LIMIT_WINDOW = int(os.getenv("RATE_LIMIT_WINDOW", "60"))
AGENT_FLEET_BASE_URL = os.getenv(
    "AGENT_FLEET_BASE_URL", "http://localhost:9100/agents"
).rstrip("/")

if not AGENT_FLEET_BASE_URL.startswith(("http://", "https://")):
    AGENT_FLEET_BASE_URL = f"http://{AGENT_FLEET_BASE_URL}"

START_TIME = time.monotonic()
HOP_BY_HOP_HEADERS = {
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailers",
    "transfer-encoding",
    "upgrade",
    "content-encoding",
    "content-length",
}


def _optional_import(module_name: str) -> Any:
    return importlib.import_module(module_name)


cache_module = _optional_import("cache")
ratelimit_module = _optional_import("ratelimit")
monitoring_module = _optional_import("monitoring")
dashboard_module = _optional_import("dashboard")


async def drain_queue_worker(app: FastAPI) -> None:
    while True:
        await asyncio.sleep(1)


@asynccontextmanager
async def lifespan(app: FastAPI):
    app.state.redis = redis.Redis(
        host=REDIS_HOST,
        port=REDIS_PORT,
        decode_responses=False,
        socket_connect_timeout=2,
        socket_timeout=2,
    )
    app.state.http_client = httpx.AsyncClient(timeout=60.0)
    app.state.queue_worker = asyncio.create_task(drain_queue_worker(app))

    try:
        yield
    finally:
        app.state.queue_worker.cancel()
        with suppress(asyncio.CancelledError):
            await app.state.queue_worker
        await app.state.http_client.aclose()
        await app.state.redis.aclose()


app = FastAPI(title="Agent Gateway", version="1.0.0", lifespan=lifespan)


if hasattr(monitoring_module, "router"):
    app.include_router(monitoring_module.router)

if hasattr(dashboard_module, "router"):
    app.include_router(dashboard_module.router)

if hasattr(cache_module, "cache_middleware"):
    app.middleware("http")(cache_module.cache_middleware)

if hasattr(ratelimit_module, "rate_limit_middleware"):
    app.middleware("http")(ratelimit_module.rate_limit_middleware)


def _cache_key(request: Request, agent_name: str, path: str) -> str:
    query = urlencode(sorted(request.query_params.multi_items()))
    return f"agent-gateway:cache:{request.method}:{agent_name}:{path}:{query}"


def _rate_limit_key(request: Request, agent_name: str) -> str:
    forwarded_for = request.headers.get("x-forwarded-for", "")
    client_ip = forwarded_for.split(",", 1)[0].strip()
    if not client_ip and request.client:
        client_ip = request.client.host
    return f"agent-gateway:ratelimit:{agent_name}:{client_ip or 'unknown'}"


def _filter_headers(headers: httpx.Headers) -> dict[str, str]:
    return {
        key: value
        for key, value in headers.items()
        if key.lower() not in HOP_BY_HOP_HEADERS
    }


async def get_cached_response(request: Request, agent_name: str, path: str) -> Response | None:
    if request.method.upper() != "GET":
        return None

    cached = await request.app.state.redis.get(_cache_key(request, agent_name, path))
    if not cached:
        return None

    payload = json.loads(cached.decode("utf-8"))
    return Response(
        content=base64.b64decode(payload["body"]),
        status_code=payload["status_code"],
        headers=payload.get("headers", {}),
        media_type=payload.get("media_type"),
    )


async def cache_response(
    request: Request,
    agent_name: str,
    path: str,
    upstream_response: httpx.Response,
) -> None:
    if request.method.upper() != "GET" or upstream_response.status_code >= 400:
        return

    payload = {
        "status_code": upstream_response.status_code,
        "headers": _filter_headers(upstream_response.headers),
        "media_type": upstream_response.headers.get("content-type"),
        "body": base64.b64encode(upstream_response.content).decode("ascii"),
    }
    await request.app.state.redis.setex(
        _cache_key(request, agent_name, path),
        CACHE_TTL,
        json.dumps(payload),
    )


async def enforce_rate_limit(request: Request, agent_name: str) -> None:
    redis_client = request.app.state.redis
    key = _rate_limit_key(request, agent_name)

    current = await redis_client.incr(key)
    if current == 1:
        await redis_client.expire(key, RATE_LIMIT_WINDOW)

    if current > RATE_LIMIT_REQUESTS:
        ttl = await redis_client.ttl(key)
        raise HTTPException(
            status_code=429,
            detail={
                "error": "rate_limit_exceeded",
                "limit": RATE_LIMIT_REQUESTS,
                "window_seconds": RATE_LIMIT_WINDOW,
                "retry_after_seconds": max(ttl, 0),
            },
        )


@app.get("/health")
async def health(request: Request) -> dict[str, Any]:
    try:
        redis_ping = bool(await request.app.state.redis.ping())
    except Exception:
        redis_ping = False

    return {
        "status": "healthy" if redis_ping else "degraded",
        "redis_ping": redis_ping,
        "uptime_seconds": round(time.monotonic() - START_TIME, 3),
    }


@app.api_route(
    "/agents/{agent_name}/{path:path}",
    methods=["GET", "POST", "PUT", "PATCH", "DELETE", "OPTIONS", "HEAD"],
)
async def proxy_agent(request: Request, agent_name: str, path: str) -> Response:
    cached_response = await get_cached_response(request, agent_name, path)
    if cached_response:
        cached_response.headers["x-cache"] = "HIT"
        return cached_response

    query = request.url.query
    target_url = f"{AGENT_FLEET_BASE_URL}/{agent_name}/{path}"
    if query:
        target_url = f"{target_url}?{query}"

    body = await request.body()
    headers = {
        key: value
        for key, value in request.headers.items()
        if key.lower() not in HOP_BY_HOP_HEADERS and key.lower() != "host"
    }

    try:
        upstream_response = await request.app.state.http_client.request(
            method=request.method,
            url=target_url,
            content=body,
            headers=headers,
        )
    except httpx.RequestError as exc:
        raise HTTPException(
            status_code=502,
            detail=f"Failed to reach agent upstream: {exc}",
        ) from exc

    await cache_response(request, agent_name, path, upstream_response)

    response = Response(
        content=upstream_response.content,
        status_code=upstream_response.status_code,
        headers=_filter_headers(upstream_response.headers),
        media_type=upstream_response.headers.get("content-type"),
    )
    response.headers["x-cache"] = "MISS"
    return response


if __name__ == "__main__":
    import uvicorn

    uvicorn.run("main:app", host="0.0.0.0", port=8090)

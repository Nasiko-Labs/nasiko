import asyncio
import json
import os
import time
from urllib.parse import urlencode

from fastapi import Request
from starlette.responses import JSONResponse


RATE_LIMIT_REQUESTS = int(os.getenv("RATE_LIMIT_REQUESTS", "10"))
RATE_LIMIT_WINDOW = int(os.getenv("RATE_LIMIT_WINDOW", "60"))


def _extract_agent_name(path: str) -> str | None:
    parts = path.strip("/").split("/")
    if len(parts) >= 2 and parts[0] == "agents" and parts[1]:
        return parts[1]
    return None


def _agent_subpath(path: str, agent_name: str) -> str:
    prefix = f"/agents/{agent_name}/"
    if path.startswith(prefix):
        return path[len(prefix):]
    return ""


def _cache_key(request: Request, agent_name: str) -> str:
    query = urlencode(sorted(request.query_params.multi_items()))
    path = _agent_subpath(request.url.path, agent_name)
    return f"agent-gateway:cache:{request.method}:{agent_name}:{path}:{query}"


async def _get_agent_limit(redis_client, agent_name: str) -> tuple[int, int]:
    override = await redis_client.get(f"limit_override:{agent_name}")
    if not override:
        return RATE_LIMIT_REQUESTS, RATE_LIMIT_WINDOW

    if isinstance(override, bytes):
        override = override.decode("utf-8")

    try:
        data = json.loads(override)
        return int(data["requests"]), int(data["window"])
    except (KeyError, TypeError, ValueError, json.JSONDecodeError):
        return RATE_LIMIT_REQUESTS, RATE_LIMIT_WINDOW


def _headers(remaining: int, reset: int, queue_depth: int) -> dict[str, str]:
    return {
        "X-RateLimit-Remaining": str(max(remaining, 0)),
        "X-RateLimit-Reset": str(max(reset, 0)),
        "X-Queue-Depth": str(max(queue_depth, 0)),
    }


async def rate_limit_middleware(request: Request, call_next):
    agent_name = _extract_agent_name(request.url.path)
    if not agent_name:
        return await call_next(request)

    redis_client = request.app.state.redis
    limit, window = await _get_agent_limit(redis_client, agent_name)
    now = time.time()
    key = f"agent-gateway:ratelimit:{agent_name}"
    queue_key = f"queue:{agent_name}"

    if request.method.upper() == "GET" and await redis_client.exists(
        _cache_key(request, agent_name)
    ):
        queue_depth = await redis_client.llen(queue_key)
        response = await call_next(request)
        for header, value in _headers(limit, int(now + window), queue_depth).items():
            response.headers.setdefault(header, value)
        return response

    await redis_client.zadd(key, {str(now): now})
    await redis_client.zremrangebyscore(key, 0, now - window)
    await redis_client.expire(key, window)

    count = await redis_client.zcard(key)
    queue_depth = await redis_client.llen(queue_key)
    reset = int(now + window)

    if count <= limit:
        response = await call_next(request)
        for header, value in _headers(limit - count, reset, queue_depth).items():
            response.headers[header] = value
        return response

    body = await request.body()
    queued_request = {
        "method": request.method,
        "path": request.url.path,
        "headers": dict(request.headers),
        "body": body.decode("utf-8", errors="replace"),
        "timestamp": now,
    }
    queue_depth = await redis_client.rpush(queue_key, json.dumps(queued_request))

    return JSONResponse(
        status_code=202,
        content={
            "status": "queued",
            "position": queue_depth,
            "agent": agent_name,
            "message": "Request queued",
        },
        headers=_headers(0, reset, queue_depth),
    )


async def get_rate_limit_stats(redis_client) -> dict:
    queues = {}
    total_queued = 0

    async for key in redis_client.scan_iter("queue:*"):
        key_str = key.decode("utf-8") if isinstance(key, bytes) else key
        agent_name = key_str.split(":", 1)[1]
        depth = await redis_client.llen(key)
        queues[agent_name] = depth
        total_queued += depth

    return {"queues": queues, "total_queued": total_queued}


async def set_agent_limit(redis_client, agent_name, requests, window) -> None:
    await redis_client.set(
        f"limit_override:{agent_name}",
        json.dumps({"requests": requests, "window": window}),
    )

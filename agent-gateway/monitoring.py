import json
import os

from fastapi import APIRouter, Request
from pydantic import BaseModel


RATE_LIMIT_REQUESTS = int(os.getenv("RATE_LIMIT_REQUESTS", "10"))
RATE_LIMIT_WINDOW = int(os.getenv("RATE_LIMIT_WINDOW", "60"))

router = APIRouter(prefix="/control", tags=["monitoring"])


class LimitOverride(BaseModel):
    requests: int
    window: int


def _to_str(value) -> str:
    return value.decode("utf-8") if isinstance(value, bytes) else value


async def _scan_keys(redis, pattern: str) -> list:
    cursor = 0
    keys = []
    while True:
        cursor, batch = await redis.scan(cursor, match=pattern, count=100)
        keys.extend(batch)
        if cursor == 0:
            break
    return keys


async def _get_int(redis, key: str) -> int:
    value = await redis.get(key)
    return int(value or 0)


@router.get("/stats")
async def get_stats(request: Request) -> dict:
    redis = request.app.state.redis

    hits = await _get_int(redis, "stats:cache_hits")
    misses = await _get_int(redis, "stats:cache_misses")
    total = hits + misses
    hit_rate_percent = (hits / total) * 100 if total > 0 else 0

    per_agent = {}
    for key in await _scan_keys(redis, "stats:agent:*:requests"):
        key_str = _to_str(key)
        agent_name = key_str.removeprefix("stats:agent:").removesuffix(":requests")
        per_agent[agent_name] = await _get_int(redis, key_str)

    queues = {}
    for key in await _scan_keys(redis, "queue:*"):
        key_str = _to_str(key)
        agent_name = key_str.removeprefix("queue:")
        queues[agent_name] = await redis.llen(key)

    return {
        "cache": {
            "hits": hits,
            "misses": misses,
            "hit_rate_percent": hit_rate_percent,
        },
        "per_agent_requests": per_agent,
        "queues": queues,
    }


@router.get("/cache")
async def list_cache(request: Request) -> list[dict]:
    redis = request.app.state.redis
    items = []

    for key in await _scan_keys(redis, "agent-gateway:cache:*"):
        key_str = _to_str(key)
        items.append({"key": key_str, "ttl_remaining": await redis.ttl(key)})

    return items


@router.delete("/cache")
async def clear_cache(request: Request) -> dict:
    redis = request.app.state.redis
    keys = await _scan_keys(redis, "agent-gateway:cache:*")

    deleted = 0
    if keys:
        deleted = await redis.delete(*keys)

    return {"deleted": deleted}


@router.delete("/cache/{agent_name}")
async def clear_agent_cache(request: Request, agent_name: str) -> dict:
    redis = request.app.state.redis
    keys = await _scan_keys(redis, f"agent-gateway:cache:*:{agent_name}:*")

    deleted = 0
    if keys:
        deleted = await redis.delete(*keys)

    return {"deleted": deleted}


@router.get("/limits")
async def get_limits(request: Request) -> dict:
    redis = request.app.state.redis
    overrides = {}

    for key in await _scan_keys(redis, "limit_override:*"):
        key_str = _to_str(key)
        agent_name = key_str.removeprefix("limit_override:")
        value = await redis.get(key)
        if value:
            overrides[agent_name] = json.loads(_to_str(value))

    return {
        "default": {
            "requests": RATE_LIMIT_REQUESTS,
            "window": RATE_LIMIT_WINDOW,
        },
        "overrides": overrides,
    }


@router.put("/limits/{agent_name}")
async def set_limit(request: Request, agent_name: str, body: LimitOverride) -> dict:
    redis = request.app.state.redis
    payload = {"requests": body.requests, "window": body.window}
    await redis.set(f"limit_override:{agent_name}", json.dumps(payload))

    return {
        "agent": agent_name,
        "requests": body.requests,
        "window": body.window,
        "applied": True,
    }


@router.get("/queue")
async def get_queue(request: Request) -> dict:
    redis = request.app.state.redis
    queues = {}

    for key in await _scan_keys(redis, "queue:*"):
        key_str = _to_str(key)
        agent_name = key_str.removeprefix("queue:")
        queues[agent_name] = await redis.llen(key)

    return queues

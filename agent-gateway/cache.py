from fastapi import Request, Response


def _extract_agent_name(path: str) -> str | None:
    parts = path.strip("/").split("/")
    if len(parts) >= 2 and parts[0] == "agents" and parts[1]:
        return parts[1]
    return None


async def cache_middleware(request: Request, call_next) -> Response:
    agent_name = _extract_agent_name(request.url.path)
    if not agent_name:
        return await call_next(request)

    redis_client = request.app.state.redis
    await redis_client.incr(f"stats:agent:{agent_name}:requests")

    response = await call_next(request)
    if response.status_code == 202:
        return response

    cache_status = response.headers.get("X-Cache", "MISS").upper()

    if cache_status == "HIT":
        await redis_client.incr("stats:cache_hits")
        response.headers["X-Cache"] = "HIT"
    else:
        await redis_client.incr("stats:cache_misses")
        response.headers["X-Cache"] = "MISS"

    return response


async def get_cache_stats(redis_client) -> dict:
    hits_raw = await redis_client.get("stats:cache_hits")
    misses_raw = await redis_client.get("stats:cache_misses")

    hits = int(hits_raw or 0)
    misses = int(misses_raw or 0)
    total = hits + misses
    hit_rate_percent = round((hits / total) * 100, 2) if total else 0.0

    return {
        "hits": hits,
        "misses": misses,
        "hit_rate_percent": hit_rate_percent,
    }

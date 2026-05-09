import hashlib
import json
import logging
import time
from typing import Any

import numpy as np
import redis.asyncio as aioredis

from src.config import settings
from src.embeddings import cosine_similarity, get_embedding

logger = logging.getLogger(__name__)

_AGENT_TTL_DEFAULTS: dict[str, int] = {
    "translation": 3600,
    "compliance": 86400,
    "github": 300,
}
_DEFAULT_TTL = 1800


def _ttl_for_agent(agent_name: str) -> int:
    for tag, ttl in _AGENT_TTL_DEFAULTS.items():
        if tag in agent_name.lower():
            return ttl
    return _DEFAULT_TTL


def _vector_hash(text: str) -> str:
    return hashlib.sha256(text.encode()).hexdigest()[:16]


async def get_cached_response(
    redis: aioredis.Redis,
    agent_name: str,
    input_text: str,
) -> dict | None:
    query_vec = get_embedding(input_text)
    pattern = f"cache:{agent_name}:*"

    async for key in redis.scan_iter(match=pattern, count=100):
        raw = await redis.hgetall(key)
        if not raw:
            continue
        stored_vec = np.frombuffer(raw[b"vector"], dtype=np.float32)
        score = cosine_similarity(query_vec, stored_vec)
        if score >= settings.CACHE_SIMILARITY_THRESHOLD:
            await redis.hincrby(key, "hit_count", 1)
            await redis.zincrby("cache:top_queries", 1, raw[b"input_text"].decode())
            logger.debug(f"cache HIT for {agent_name} (score={score:.3f})")
            return json.loads(raw[b"response"])

    return None


async def store_response(
    redis: aioredis.Redis,
    agent_name: str,
    input_text: str,
    response_body: bytes,
    status_code: int,
) -> None:
    vec = get_embedding(input_text)
    key = f"cache:{agent_name}:{_vector_hash(input_text)}"
    ttl = _ttl_for_agent(agent_name)

    payload = json.dumps({"body": response_body.decode(errors="replace"), "status_code": status_code})

    await redis.hset(
        key,
        mapping={
            "vector": vec.astype(np.float32).tobytes(),
            "input_text": input_text,
            "response": payload,
            "timestamp": int(time.time()),
            "hit_count": 0,
        },
    )
    await redis.expire(key, ttl)
    logger.debug(f"cache STORE for {agent_name} ttl={ttl}s")


async def purge_agent_cache(redis: aioredis.Redis, agent_name: str) -> int:
    keys = [k async for k in redis.scan_iter(match=f"cache:{agent_name}:*")]
    if keys:
        await redis.delete(*keys)
    return len(keys)


async def purge_all_cache(redis: aioredis.Redis) -> int:
    keys = [k async for k in redis.scan_iter(match="cache:*")]
    if keys:
        await redis.delete(*keys)
    await redis.delete("cache:top_queries")
    return len(keys)


async def get_cache_stats(redis: aioredis.Redis) -> dict[str, Any]:
    total_entries = 0
    total_hits = 0
    async for key in redis.scan_iter(match="cache:*:*"):
        total_entries += 1
        raw = await redis.hget(key, "hit_count")
        if raw:
            total_hits += int(raw)

    top = await redis.zrevrange("cache:top_queries", 0, 4, withscores=True)

    return {
        "total_entries": total_entries,
        "total_hits": total_hits,
        "top_queries": [{"query": q.decode(), "hits": int(s)} for q, s in top],
    }


async def get_agent_cache_entries(redis: aioredis.Redis, agent_name: str) -> list[dict]:
    entries = []
    async for key in redis.scan_iter(match=f"cache:{agent_name}:*"):
        raw = await redis.hgetall(key)
        if raw:
            entries.append({
                "key": key.decode(),
                "input_text": raw.get(b"input_text", b"").decode(),
                "hit_count": int(raw.get(b"hit_count", 0)),
                "timestamp": int(raw.get(b"timestamp", 0)),
            })
    return entries

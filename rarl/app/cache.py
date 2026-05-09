import hashlib
import json
from typing import Any

import redis.asyncio as aioredis

CACHEABLE_METHODS = {"GET", "POST"}
SKIP_HEADERS = {"x-request-id", "x-trace-id", "authorization", "cookie", "user-agent"}


def make_cache_key(
    agent_id: str, method: str, path: str, query: str, body: bytes, headers: dict
) -> str:
    """SHA-256 of canonicalized request. Returns 'cache:{agent_id}:{hexdigest}'."""
    try:
        parsed = json.loads(body) if body else None
        canonical_body = (
            json.dumps(parsed, sort_keys=True, separators=(",", ":"))
            if parsed is not None
            else ""
        )
    except (json.JSONDecodeError, UnicodeDecodeError):
        canonical_body = body.decode("utf-8", errors="replace")

    relevant_headers = sorted(
        f"{k.lower()}:{v}"
        for k, v in headers.items()
        if k.lower() not in SKIP_HEADERS and not k.lower().startswith("x-rarl")
    )

    payload = f"{method.upper()}|{path}|{query}|{canonical_body}|{'|'.join(relevant_headers)}"
    digest = hashlib.sha256(payload.encode("utf-8")).hexdigest()
    return f"cache:{agent_id}:{digest}"


class RedisCache:
    def __init__(self, redis: aioredis.Redis) -> None:
        self.redis = redis
        self.hits = 0
        self.misses = 0

    async def get(self, key: str) -> dict[str, Any] | None:
        raw = await self.redis.get(key)
        if raw is None:
            self.misses += 1
            return None
        self.hits += 1
        return json.loads(raw)

    async def set(self, key: str, value: dict[str, Any], ttl: int) -> None:
        await self.redis.set(key, json.dumps(value), ex=ttl)

    async def purge(self, agent_id: str | None = None) -> int:
        pattern = f"cache:{agent_id}:*" if agent_id else "cache:*"
        count = 0
        async for k in self.redis.scan_iter(match=pattern, count=200):
            await self.redis.delete(k)
            count += 1
        return count

    async def keys_count(self) -> int:
        count = 0
        async for _ in self.redis.scan_iter(match="cache:*", count=200):
            count += 1
        return count

    @property
    def hit_rate(self) -> float:
        total = self.hits + self.misses
        return self.hits / total if total else 0.0

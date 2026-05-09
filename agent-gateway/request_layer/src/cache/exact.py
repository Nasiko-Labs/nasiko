"""Exact-match (SHA-256 keyed) response cache."""
import base64
import hashlib
import logging
from datetime import datetime, timezone

from redis.asyncio import Redis

from request_layer.src.types import CacheEntry

logger = logging.getLogger(__name__)

_KEY_PREFIX = "request_layer:exact"


def make_key(agent: str, normalized_body: str) -> str:
    """Build the Redis key for an (agent, normalized_body) pair."""

    digest = hashlib.sha256(normalized_body.encode("utf-8")).hexdigest()
    return f"{_KEY_PREFIX}:{agent}:{digest}"


async def get(redis: Redis, agent: str, normalized_body: str) -> CacheEntry | None:
    """Return the cached entry for this query, or ``None`` on miss."""

    key = make_key(agent, normalized_body)
    raw = await redis.get(key)
    if raw is None:
        return None
    try:
        return CacheEntry.model_validate_json(raw)
    except ValueError:
        # Corrupt entry — best to drop it and miss.
        logger.warning("dropping corrupt L1 entry at %s", key)
        await redis.delete(key)
        return None


async def set(
    redis: Redis,
    agent: str,
    normalized_body: str,
    entry: CacheEntry,
    ttl_seconds: int,
) -> None:
    """Store ``entry`` under ``(agent, normalized_body)`` with a TTL."""

    key = make_key(agent, normalized_body)
    payload = entry.model_dump_json()
    await redis.set(key, payload, ex=ttl_seconds)


async def clear(redis: Redis, agent: str | None = None) -> int:
    """Remove all L1 entries (optionally for one agent only).

    Returns the number of keys deleted.
    """

    pattern = f"{_KEY_PREFIX}:{agent or '*'}:*"
    deleted = 0
    async for key in redis.scan_iter(match=pattern):
        await redis.delete(key)
        deleted += 1
    return deleted


def serialize_entry(
    *,
    status_code: int,
    headers: dict[str, str],
    body: bytes | str,
    cost_usd: float,
    latency_ms: float,
    matched_query: str | None = None,
) -> CacheEntry:
    """Convenience constructor used by the proxy pipeline."""

    if isinstance(body, bytes):
        try:
            body_str = body.decode("utf-8")
        except UnicodeDecodeError:
            # Base64 fallback keeps binary bodies cacheable; rare for agents.
            body_str = "b64:" + base64.b64encode(body).decode("ascii")
    else:
        body_str = body

    return CacheEntry(
        status_code=status_code,
        headers=headers,
        body=body_str,
        cost_usd=cost_usd,
        latency_ms=latency_ms,
        cached_at=datetime.now(timezone.utc),
        matched_query=matched_query,
    )


def decode_body(entry: CacheEntry) -> bytes:
    """Reverse the encoding done by :func:`serialize_entry`."""

    if entry.body.startswith("b64:"):
        return base64.b64decode(entry.body[4:])
    return entry.body.encode("utf-8")


def stable_hash(payload: str) -> str:
    """Expose the SHA-256 helper for tests and external callers."""

    return hashlib.sha256(payload.encode("utf-8")).hexdigest()

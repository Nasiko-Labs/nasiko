"""In-flight request coalescer (Redis SETNX + pubsub)."""
import asyncio
import logging
from contextlib import asynccontextmanager
from typing import AsyncIterator

from redis.asyncio import Redis

logger = logging.getLogger(__name__)


def _inflight_key(agent: str, query_hash: str) -> str:
    return f"request_layer:inflight:{agent}:{query_hash}"


def _broadcast_channel(agent: str, query_hash: str) -> str:
    return f"request_layer:bcast:{agent}:{query_hash}"


@asynccontextmanager
async def acquire_leader(
    redis: Redis,
    agent: str,
    query_hash: str,
    ttl_seconds: int,
) -> AsyncIterator[bool]:
    """Try to become the leader for this request key.

    Yields ``True`` if this caller is the leader (and must run the agent
    and publish), ``False`` if the caller is a follower (and should call
    :func:`wait_for_broadcast` instead).

    On exit (regardless of success), the inflight key is deleted so future
    requests are not blocked.
    """

    key = _inflight_key(agent, query_hash)
    is_leader = await redis.set(key, "1", nx=True, ex=ttl_seconds)
    try:
        yield bool(is_leader)
    finally:
        if is_leader:
            await redis.delete(key)


async def wait_for_broadcast(
    redis: Redis,
    agent: str,
    query_hash: str,
    timeout_seconds: int,
) -> bytes | None:
    """Block until the leader publishes a result, or until timeout.

    Returns the broadcast payload on success, ``None`` on timeout. If the
    leader crashed (i.e. its inflight TTL expired without a publish), this
    function also returns ``None`` so the follower can promote itself.
    """

    channel = _broadcast_channel(agent, query_hash)
    pubsub = redis.pubsub()
    await pubsub.subscribe(channel)
    try:
        deadline = asyncio.get_running_loop().time() + timeout_seconds
        while True:
            remaining = deadline - asyncio.get_running_loop().time()
            if remaining <= 0:
                return None
            message = await pubsub.get_message(
                ignore_subscribe_messages=True,
                timeout=remaining,
            )
            if message is None:
                return None
            data = message.get("data")
            if data is None:
                continue
            if isinstance(data, str):
                data = data.encode("utf-8")
            return data
    finally:
        try:
            await pubsub.unsubscribe(channel)
        finally:
            await pubsub.aclose()


async def broadcast(
    redis: Redis,
    agent: str,
    query_hash: str,
    payload: bytes | str,
) -> int:
    """Publish ``payload`` to followers waiting on ``(agent, query_hash)``.

    Returns the number of subscribers that received the message.
    """

    channel = _broadcast_channel(agent, query_hash)
    return int(await redis.publish(channel, payload))

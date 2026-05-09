"""Three-lane priority queue."""
import logging
from typing import Literal

from redis.asyncio import Redis

logger = logging.getLogger(__name__)

Lane = Literal["high", "normal", "low"]
LANES: tuple[Lane, ...] = ("high", "normal", "low")


def _queue_key(agent: str, lane: Lane) -> str:
    return f"request_layer:q:{agent}:{lane}"


def _processing_key(agent: str) -> str:
    return f"request_layer:q:{agent}:processing"


def resolve_priority(
    header_value: str | None,
    agent_tags: set[str],
) -> Lane:
    """Map an inbound request's metadata to a queue lane."""

    if header_value:
        candidate = header_value.strip().lower()
        if candidate in LANES:
            return candidate  # type: ignore[return-value]
    if "critical" in {tag.lower() for tag in agent_tags}:
        return "high"
    return "normal"


async def enqueue(
    redis: Redis,
    agent: str,
    lane: Lane,
    request_id: str,
) -> int:
    """Push ``request_id`` onto the lane and return the new lane depth."""

    return int(await redis.lpush(_queue_key(agent, lane), request_id))


async def dequeue_one(redis: Redis, agent: str) -> tuple[Lane, str] | None:
    """Pop the highest-priority request available for ``agent``.

    Returns ``(lane, request_id)`` or ``None`` if all lanes are empty.
    """

    for lane in LANES:
        item = await redis.rpoplpush(_queue_key(agent, lane), _processing_key(agent))
        if item is not None:
            if isinstance(item, bytes):
                item = item.decode("utf-8")
            return lane, item
    return None


async def mark_done(redis: Redis, agent: str, request_id: str) -> int:
    """Remove ``request_id`` from the processing lane after it completes."""

    return int(await redis.lrem(_processing_key(agent), 1, request_id))


async def depth(redis: Redis, agent: str) -> dict[str, int]:
    """Return per-lane queue depth for ``agent``."""

    pipe = redis.pipeline()
    for lane in LANES:
        pipe.llen(_queue_key(agent, lane))
    pipe.llen(_processing_key(agent))
    sizes = await pipe.execute()
    return {
        "high": int(sizes[0]),
        "normal": int(sizes[1]),
        "low": int(sizes[2]),
        "processing": int(sizes[3]),
    }


def predicted_wait_ms(
    queue_depth: int,
    p95_latency_ms: float,
    parallelism: int = 1,
) -> int:
    """Rough estimate displayed on the dashboard."""

    parallelism = max(1, parallelism)
    return int((queue_depth * p95_latency_ms) / parallelism)

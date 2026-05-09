"""
Redis-backed event emitter for real-time system observability.

Events are pushed to a Redis list (monitoring:events) and capped at MAX_EVENTS.
The dashboard polls /monitoring/events and renders them directly — no inference.
"""

import json
import logging
from datetime import datetime

import redis.asyncio as aioredis

logger = logging.getLogger(__name__)

EVENT_KEY = "monitoring:events"
MAX_EVENTS = 100

# Only request_completed is deduplicated (2s window) to suppress completion noise
# under burst load. cache_hit / cache_miss are NEVER suppressed — full visibility
# is required for the demo event stream.
_DEDUP_KEY_PREFIX = "monitoring:event_dedup:"
_DEDUP_TTL = 2  # seconds


class EventEmitter:
    """
    Emits structured events to Redis for real-time dashboard consumption.

    Must be initialised with a live redis client — not optional.
    All services that emit events receive this emitter as a required constructor arg.
    """

    def __init__(self, redis: aioredis.Redis):
        self.redis = redis

    async def emit(self, type: str, agent: str = "", **kwargs) -> None:
        """Push an event to the Redis event list."""
        # Dedup only request_completed — cache_hit/miss must appear verbatim
        if type == "request_completed":
            dedup_key = f"{_DEDUP_KEY_PREFIX}{type}:{agent}"
            try:
                already = await self.redis.set(dedup_key, "1", nx=True, ex=_DEDUP_TTL)
                if not already:
                    return
            except Exception:
                pass  # dedup failure is non-fatal — emit the event anyway

        event = {
            "type": type,
            "agent": agent,
            "ts": datetime.utcnow().isoformat() + "Z",
            **kwargs,
        }
        try:
            await self.redis.lpush(EVENT_KEY, json.dumps(event))
            await self.redis.ltrim(EVENT_KEY, 0, MAX_EVENTS - 1)
        except Exception as e:
            logger.warning(f"EventEmitter.emit failed ({type}): {e}")

    async def get_recent(self, n: int = 50) -> list:
        """Return the n most recent events, newest first."""
        try:
            raw = await self.redis.lrange(EVENT_KEY, 0, n - 1)
            return [json.loads(e) for e in raw]
        except Exception as e:
            logger.warning(f"EventEmitter.get_recent failed: {e}")
            return []

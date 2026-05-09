"""
Request queue manager for Sentinel Guard.
Redis-backed priority queue (FIFO via sorted sets) with overflow control.
"""

import asyncio
import json
import logging
import time
import uuid
from typing import Any, Optional

import redis

from app.config import config
from app.store import (
    Decision, requests_queued,
    increment_counter, record_decision,
)

logger = logging.getLogger("sentinel.queue")


class QueueManager:
    """Redis-backed request queue with depth limits and timeout expiry."""

    def __init__(self) -> None:
        try:
            self._redis = redis.Redis(
                host=config.REDIS_HOST, port=config.REDIS_PORT,
                db=config.REDIS_DB, decode_responses=True,
                socket_connect_timeout=5,
            )
            self._redis.ping()
            self._redis_ok = True
            logger.info("Queue manager connected to Redis")
        except Exception as exc:
            logger.warning(f"Redis unavailable for queue – using in-memory: {exc}")
            self._redis_ok = False
            self._redis = None
        self._memory_queues: dict[str, list[dict]] = {}

    def _queue_key(self, agent: str) -> str:
        return f"sentinel:queue:{agent}"

    def _data_key(self, agent: str) -> str:
        return f"sentinel:queue_data:{agent}"

    def enqueue(self, agent: str, query: str, payload: Any) -> dict:
        """Add a request to the agent's queue. Returns position and wait estimate."""
        now = time.time()
        item_id = str(uuid.uuid4())[:12]
        item = {
            "id": item_id, "agent": agent, "query": query,
            "payload": payload, "enqueued_at": now,
        }

        depth = self.get_depth(agent)
        if depth >= config.MAX_QUEUE_DEPTH:
            record_decision(Decision(
                timestamp=now, agent=agent, query=query,
                outcome="queue_full",
            ))
            return {
                "queued": False, "reason": "queue_full",
                "depth": depth, "max_depth": config.MAX_QUEUE_DEPTH,
            }

        if self._redis_ok and self._redis:
            try:
                self._redis.zadd(self._queue_key(agent), {item_id: now})
                self._redis.hset(self._data_key(agent), item_id, json.dumps(item))
                self._redis.expire(self._queue_key(agent), config.QUEUE_ITEM_TIMEOUT_SECONDS + 60)
                self._redis.expire(self._data_key(agent), config.QUEUE_ITEM_TIMEOUT_SECONDS + 60)
            except Exception as exc:
                logger.warning(f"Redis queue error: {exc}")
                self._memory_enqueue(agent, item)
        else:
            self._memory_enqueue(agent, item)

        position = self.get_depth(agent)
        est_wait = int(position * 1000 * config.RATE_LIMIT_WINDOW_SECONDS / max(config.RATE_LIMIT_DEFAULT_RPM, 1))

        increment_counter(requests_queued, agent)
        record_decision(Decision(
            timestamp=now, agent=agent, query=query,
            outcome="queued", queue_position=position,
            estimated_wait_ms=est_wait,
        ))

        logger.info(f"Enqueued: agent={agent} pos={position} wait~{est_wait}ms")
        return {
            "queued": True, "id": item_id, "position": position,
            "estimated_wait_ms": est_wait,
        }

    def dequeue(self, agent: str) -> Optional[dict]:
        """Pop the oldest item from the queue."""
        if self._redis_ok and self._redis:
            try:
                items = self._redis.zpopmin(self._queue_key(agent), 1)
                if items:
                    item_id = items[0][0]
                    raw = self._redis.hget(self._data_key(agent), item_id)
                    self._redis.hdel(self._data_key(agent), item_id)
                    return json.loads(raw) if raw else None
            except Exception as exc:
                logger.warning(f"Redis dequeue error: {exc}")

        q = self._memory_queues.get(agent, [])
        return q.pop(0) if q else None

    def get_depth(self, agent: str) -> int:
        if self._redis_ok and self._redis:
            try:
                return int(self._redis.zcard(self._queue_key(agent)))
            except Exception:
                pass
        return len(self._memory_queues.get(agent, []))

    def get_all_depths(self) -> dict[str, int]:
        agents: set[str] = set()
        if self._redis_ok and self._redis:
            try:
                for key in self._redis.scan_iter("sentinel:queue:*", count=100):
                    ag = key.replace("sentinel:queue:", "")
                    if not ag.startswith("data:"):
                        agents.add(ag)
            except Exception:
                pass
        agents.update(self._memory_queues.keys())
        return {ag: self.get_depth(ag) for ag in agents}

    def cleanup_expired(self) -> int:
        """Remove queue items older than QUEUE_ITEM_TIMEOUT_SECONDS."""
        cutoff = time.time() - config.QUEUE_ITEM_TIMEOUT_SECONDS
        removed = 0
        if self._redis_ok and self._redis:
            try:
                for key in self._redis.scan_iter("sentinel:queue:*", count=100):
                    if key.startswith("sentinel:queue_data:"):
                        continue
                    removed += self._redis.zremrangebyscore(key, "-inf", cutoff)
            except Exception:
                pass
        for agent, q in self._memory_queues.items():
            before = len(q)
            self._memory_queues[agent] = [i for i in q if i.get("enqueued_at", 0) > cutoff]
            removed += before - len(self._memory_queues[agent])
        return removed

    def _memory_enqueue(self, agent: str, item: dict) -> None:
        if agent not in self._memory_queues:
            self._memory_queues[agent] = []
        self._memory_queues[agent].append(item)

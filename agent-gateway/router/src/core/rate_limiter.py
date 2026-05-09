"""
Distributed rate limiter using Redis sorted sets.

Design:
- Sliding window per agent (and per user×agent) implemented as a Redis sorted set.
- Excess requests enter a Redis-backed queue (also a sorted set) and are served
  with exponential-backoff polling — no asyncio.Queue, safe across replicas.
- User ID is extracted from the JWT payload (already validated by Kong).
"""

import asyncio
import base64
import json
import logging
import time
import uuid
from typing import Any, Dict, Optional, Tuple

import redis.asyncio as aioredis

from router.src.core.events import EventEmitter

logger = logging.getLogger(__name__)

_WINDOW_SECONDS = 60  # sliding window width


def _extract_user_id(token: str) -> str:
    """Decode JWT payload (no verification — Kong already validated) to get subject."""
    try:
        parts = token.split(".")
        if len(parts) < 2:
            return "anon"
        payload_b64 = parts[1]
        # Restore base64 padding
        payload_b64 += "=" * (-len(payload_b64) % 4)
        payload = json.loads(base64.b64decode(payload_b64))
        return str(
            payload.get("sub")
            or payload.get("user_id")
            or payload.get("id")
            or "anon"
        )
    except Exception:
        return "anon"


class AgentRateLimiter:
    """
    Per-agent (and per-user) sliding-window rate limiter backed entirely by Redis.

    Rate limit config is stored in Redis so it can be updated at runtime via the
    monitoring API without restarting the router.
    """

    def __init__(
        self,
        redis: aioredis.Redis,
        default_rpm: int = 60,
        max_queue_size: int = 50,
        queue_timeout: float = 30.0,
        emitter: EventEmitter = None,
    ):
        if emitter is None:
            raise TypeError("AgentRateLimiter requires an EventEmitter — pass emitter=")
        self.redis = redis
        self.default_rpm = default_rpm
        self.max_queue_size = max_queue_size
        self.queue_timeout = queue_timeout
        self.emitter = emitter

    # -------------------------------------------------------------------------
    # Config (stored in Redis for live updates)
    # -------------------------------------------------------------------------

    async def get_config(self, agent_name: str) -> Dict[str, Any]:
        config_key = f"ratelimit:config:{agent_name}"
        try:
            raw = await self.redis.hgetall(config_key)
            if raw:
                return {
                    "rpm": int(raw.get("rpm", self.default_rpm)),
                    "max_queue": int(raw.get("max_queue", self.max_queue_size)),
                }
        except Exception as e:
            logger.warning(f"Rate limit config fetch error: {e}")
        return {"rpm": self.default_rpm, "max_queue": self.max_queue_size}

    async def set_config(self, agent_name: str, rpm: int, max_queue: int) -> None:
        config_key = f"ratelimit:config:{agent_name}"
        try:
            await self.redis.hset(config_key, mapping={"rpm": rpm, "max_queue": max_queue})
            logger.info(f"Rate limit config updated for '{agent_name}': rpm={rpm}, max_queue={max_queue}")
        except Exception as e:
            logger.warning(f"Rate limit config set error: {e}")

    # -------------------------------------------------------------------------
    # Sliding window check + registration
    # -------------------------------------------------------------------------

    async def _check_and_register(
        self, key: str, limit: int, member: str, register: bool = True
    ) -> bool:
        """
        Atomic-ish sliding window check on a sorted set.
        Returns True if request is within the limit.
        If register=True and within limit, adds the member to the window.
        """
        now = time.time()
        window_start = now - _WINDOW_SECONDS
        try:
            pipe = self.redis.pipeline()
            pipe.zremrangebyscore(key, 0, window_start)
            pipe.zcard(key)
            results = await pipe.execute()
            count = results[1]
            if count < limit:
                if register:
                    await self.redis.zadd(key, {member: now})
                    await self.redis.expire(key, _WINDOW_SECONDS + 10)
                return True
            return False
        except Exception as e:
            logger.warning(f"Sliding window check error on {key}: {e}")
            return True  # fail open to avoid blocking legitimate traffic

    async def _check_only(self, key: str, limit: int) -> bool:
        """Check limit without registering (used inside the wait loop)."""
        now = time.time()
        window_start = now - _WINDOW_SECONDS
        try:
            pipe = self.redis.pipeline()
            pipe.zremrangebyscore(key, 0, window_start)
            pipe.zcard(key)
            results = await pipe.execute()
            return results[1] < limit
        except Exception:
            return True

    # -------------------------------------------------------------------------
    # Public acquire interface
    # -------------------------------------------------------------------------

    async def acquire(
        self, agent_name: str, token: str = ""
    ) -> Tuple[bool, Optional[float]]:
        """
        Try to acquire a request slot for the given agent.

        Returns (True, None) if allowed immediately.
        Queues excess requests and waits up to queue_timeout before giving up.
        Returns (False, retry_after_seconds) when limit is exceeded + queue full.
        """
        config = await self.get_config(agent_name)
        agent_rpm: int = config["rpm"]
        max_queue: int = config["max_queue"]
        user_rpm = max(1, agent_rpm // 5)  # user gets 20% of agent RPM

        user_id = _extract_user_id(token)
        request_id = str(uuid.uuid4())

        agent_key = f"ratelimit:agent:{agent_name}"
        user_key = f"ratelimit:user:{user_id}:{agent_name}"
        queue_key = f"queue:agent:{agent_name}"

        member = f"{request_id}:{time.time()}"

        # Fast path: both agent and user windows allow it
        agent_ok = await self._check_and_register(agent_key, agent_rpm, member)
        if agent_ok:
            user_ok = await self._check_and_register(user_key, user_rpm, member)
            if user_ok:
                return True, None
            # Undo agent slot registration if user limit hit
            try:
                await self.redis.zrem(agent_key, member)
            except Exception:
                pass

        # Rate limited — try to queue
        try:
            queue_depth = await self.redis.zcard(queue_key)
        except Exception:
            queue_depth = 0

        if queue_depth >= max_queue:
            retry_after = _WINDOW_SECONDS - (time.time() % _WINDOW_SECONDS)
            await self._incr_stat(agent_name, "rejected")
            await self.emitter.emit("rate_limited", agent=agent_name, retry_after=round(retry_after, 1))
            logger.warning(f"Rate limit queue full for agent '{agent_name}' (depth={queue_depth})")
            return False, retry_after

        # Enqueue and wait with exponential backoff polling
        await self._incr_stat(agent_name, "queued")
        await self.emitter.emit("queued", agent=agent_name, queue_depth=int(queue_depth))
        try:
            await self.redis.zadd(queue_key, {request_id: time.time()})
            await self.redis.expire(queue_key, int(self.queue_timeout) + 10)
        except Exception as e:
            logger.warning(f"Queue enqueue error: {e}")

        deadline = time.time() + self.queue_timeout
        poll_interval = 0.25

        while time.time() < deadline:
            # Re-check both windows without registering yet
            agent_ok = await self._check_only(agent_key, agent_rpm)
            user_ok = await self._check_only(user_key, user_rpm)
            if agent_ok and user_ok:
                # Slot available — register and dequeue
                member2 = f"{request_id}:{time.time()}"
                agent_ok2 = await self._check_and_register(agent_key, agent_rpm, member2)
                user_ok2 = await self._check_and_register(user_key, user_rpm, member2) if agent_ok2 else False
                if agent_ok2 and user_ok2:
                    try:
                        await self.redis.zrem(queue_key, request_id)
                    except Exception:
                        pass
                    return True, None
                if agent_ok2:
                    try:
                        await self.redis.zrem(agent_key, member2)
                    except Exception:
                        pass

            remaining = deadline - time.time()
            if remaining <= 0:
                break
            await asyncio.sleep(min(poll_interval, remaining))
            poll_interval = min(poll_interval * 1.5, 2.0)

        # Timed out — dequeue and reject
        try:
            await self.redis.zrem(queue_key, request_id)
        except Exception:
            pass
        await self._incr_stat(agent_name, "rejected")
        retry_after = _WINDOW_SECONDS - (time.time() % _WINDOW_SECONDS)
        await self.emitter.emit("rate_limited", agent=agent_name, retry_after=round(retry_after, 1))
        logger.warning(f"Request for '{agent_name}' timed out in queue after {self.queue_timeout}s")
        return False, retry_after

    # -------------------------------------------------------------------------
    # Stats
    # -------------------------------------------------------------------------

    async def _incr_stat(self, agent_name: str, stat: str) -> None:
        try:
            await self.redis.incr(f"ratelimit:stats:{agent_name}:{stat}")
        except Exception:
            pass

    async def get_stats(self) -> Dict[str, Any]:
        """Return per-agent queued/rejected counts and current queue depths."""
        agents: Dict[str, Any] = {}
        try:
            # Scan all stat keys
            cursor = 0
            while True:
                cursor, keys = await self.redis.scan(
                    cursor, match="ratelimit:stats:*", count=100
                )
                for key in keys:
                    parts = key.split(":")
                    if len(parts) == 4:
                        _, _, agent, stat = parts
                        if agent not in agents:
                            agents[agent] = {"queued": 0, "rejected": 0, "queue_depth": 0}
                        try:
                            agents[agent][stat] = int(await self.redis.get(key) or 0)
                        except Exception:
                            pass
                if cursor == 0:
                    break

            # Add live queue depths
            for agent in agents:
                try:
                    agents[agent]["queue_depth"] = await self.redis.zcard(
                        f"queue:agent:{agent}"
                    )
                except Exception:
                    pass

        except Exception as e:
            logger.warning(f"Rate limit stats error: {e}")

        return {
            "agents": agents,
            "defaults": {
                "rpm": self.default_rpm,
                "max_queue": self.max_queue_size,
                "queue_timeout_seconds": self.queue_timeout,
            },
        }

    async def get_all_configs(self) -> Dict[str, Any]:
        """Return all per-agent configs stored in Redis."""
        configs: Dict[str, Any] = {}
        try:
            cursor = 0
            while True:
                cursor, keys = await self.redis.scan(
                    cursor, match="ratelimit:config:*", count=100
                )
                for key in keys:
                    agent = key.split(":", 2)[2]
                    configs[agent] = await self.get_config(agent)
                if cursor == 0:
                    break
        except Exception as e:
            logger.warning(f"get_all_configs error: {e}")
        return configs

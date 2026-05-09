import asyncio
import logging
import time
import uuid
from dataclasses import dataclass, field
from typing import Any, Callable, Dict, Optional, Tuple

import redis.asyncio as aioredis

from src.config import settings

logger = logging.getLogger(__name__)

RL_NS = "rm:rl"
RL_CFG_NS = "rm:rl_cfg"
RL_STATS_NS = "rm:rl_stats"

# Atomic sliding window Lua script.
# Returns 1 if the request is allowed, 0 if rate limit exceeded.
_SLIDING_WINDOW_LUA = """
local key    = KEYS[1]
local now    = tonumber(ARGV[1])
local window = tonumber(ARGV[2])
local limit  = tonumber(ARGV[3])
local req_id = ARGV[4]
local cutoff = now - window

redis.call('ZREMRANGEBYSCORE', key, '-inf', cutoff)
local count = redis.call('ZCARD', key)

if count < limit then
    redis.call('ZADD', key, now, req_id)
    redis.call('EXPIRE', key, window + 10)
    return 1
else
    return 0
end
"""


@dataclass
class _QueuedRequest:
    future: asyncio.Future
    upstream_url: str
    body: dict
    headers: dict
    enqueued_at: float = field(default_factory=time.monotonic)


class RateLimiterAndQueue:
    def __init__(self, redis_client: aioredis.Redis):
        self._r = redis_client
        self._lua_sha: Optional[str] = None
        self._queues: Dict[str, asyncio.Queue] = {}
        self._drain_tasks: Dict[str, asyncio.Task] = {}

    # ── Lua script management ───────────────────────────────────────────────

    async def _get_lua_sha(self) -> str:
        if self._lua_sha is None:
            self._lua_sha = await self._r.script_load(_SLIDING_WINDOW_LUA)
        return self._lua_sha

    # ── Per-agent limit config ──────────────────────────────────────────────

    async def get_limit(self, agent_name: str) -> Tuple[int, int]:
        """Return (rpm, window_seconds) — custom if set, else global default."""
        val = await self._r.hgetall(f"{RL_CFG_NS}:{agent_name}")
        if val:
            rpm = int(val.get(b"rpm", settings.DEFAULT_RATE_LIMIT_RPM))
            window = int(val.get(b"window", settings.RATE_LIMIT_WINDOW_SECONDS))
            return rpm, window
        return settings.DEFAULT_RATE_LIMIT_RPM, settings.RATE_LIMIT_WINDOW_SECONDS

    async def set_limit(self, agent_name: str, rpm: int) -> None:
        await self._r.hset(
            f"{RL_CFG_NS}:{agent_name}",
            mapping={"rpm": rpm, "window": settings.RATE_LIMIT_WINDOW_SECONDS},
        )

    async def reset_limit(self, agent_name: str) -> None:
        await self._r.delete(f"{RL_CFG_NS}:{agent_name}")

    async def get_all_limits(self) -> dict:
        keys = await self._r.keys(f"{RL_CFG_NS}:*")
        result = {}
        for key in keys:
            key_str = key.decode() if isinstance(key, bytes) else key
            agent_name = key_str.split(":")[-1]
            rpm, window = await self.get_limit(agent_name)
            result[agent_name] = {"requests_per_minute": rpm, "window_seconds": window}
        return result

    # ── Rate limit check ────────────────────────────────────────────────────

    async def check(self, agent_name: str) -> bool:
        """Atomically check and record a request. Returns True if allowed."""
        rpm, window = await self.get_limit(agent_name)
        key = f"{RL_NS}:{agent_name}"
        now = time.time()
        req_id = str(uuid.uuid4())

        sha = await self._get_lua_sha()
        try:
            result = await self._r.evalsha(sha, 1, key, now, window, rpm, req_id)
        except aioredis.exceptions.NoScriptError:
            # Redis restarted and evicted the script — reload and retry once
            self._lua_sha = None
            sha = await self._get_lua_sha()
            result = await self._r.evalsha(sha, 1, key, now, window, rpm, req_id)

        allowed = bool(result)
        stat = "allowed" if allowed else "limited"
        await self._r.hincrby(f"{RL_STATS_NS}:{agent_name}", stat, 1)
        return allowed

    # ── Queue management ────────────────────────────────────────────────────

    def _get_queue(self, agent_name: str) -> asyncio.Queue:
        if agent_name not in self._queues:
            self._queues[agent_name] = asyncio.Queue(maxsize=settings.QUEUE_MAX_DEPTH)
        return self._queues[agent_name]

    async def enqueue(
        self,
        agent_name: str,
        upstream_url: str,
        body: dict,
        headers: dict,
        proxy_fn: Callable,
    ) -> Any:
        """
        Queue a rate-limited request.

        Returns the upstream response when the drain loop processes it.
        Raises asyncio.QueueFull if the queue is at capacity.
        Raises asyncio.TimeoutError if the request isn't processed within
        QUEUE_TIMEOUT_SECONDS.
        """
        queue = self._get_queue(agent_name)
        loop = asyncio.get_event_loop()
        future: asyncio.Future = loop.create_future()

        queued = _QueuedRequest(
            future=future,
            upstream_url=upstream_url,
            body=body,
            headers=headers,
        )

        queue.put_nowait(queued)  # raises QueueFull immediately if at capacity
        await self._r.hincrby(f"{RL_STATS_NS}:{agent_name}", "queued", 1)

        self._ensure_drain(agent_name, proxy_fn)

        # shield prevents the Future from being cancelled when the wait_for
        # deadline fires — the drain loop can still resolve it for cleanup
        return await asyncio.wait_for(
            asyncio.shield(future), timeout=settings.QUEUE_TIMEOUT_SECONDS
        )

    def _ensure_drain(self, agent_name: str, proxy_fn: Callable) -> None:
        task = self._drain_tasks.get(agent_name)
        if task is None or task.done():
            self._drain_tasks[agent_name] = asyncio.create_task(
                self._drain_loop(agent_name, proxy_fn)
            )

    async def _drain_loop(self, agent_name: str, proxy_fn: Callable) -> None:
        """
        Per-agent background task that drains the queue one request at a time.
        Re-checks the rate limit before each item so it respects the sliding window.
        Self-terminates after 5 consecutive seconds of queue idleness.
        """
        queue = self._get_queue(agent_name)
        idle_ticks = 0

        while True:
            try:
                queued: _QueuedRequest = queue.get_nowait()
            except asyncio.QueueEmpty:
                idle_ticks += 1
                if idle_ticks >= 5:
                    self._drain_tasks.pop(agent_name, None)
                    return
                await asyncio.sleep(1.0)
                continue

            idle_ticks = 0

            if queued.future.cancelled():
                queue.task_done()
                continue

            # Wait until the rate limit window allows the next request
            while not await self.check(agent_name):
                await asyncio.sleep(0.5)

            try:
                response = await proxy_fn(queued.upstream_url, queued.body, queued.headers)
                if not queued.future.done():
                    queued.future.set_result(response)
            except Exception as exc:
                if not queued.future.done():
                    queued.future.set_exception(exc)
            finally:
                queue.task_done()

    # ── Stats ───────────────────────────────────────────────────────────────

    def queue_stats(self) -> dict:
        return {
            agent: {
                "depth": q.qsize(),
                "max_depth": settings.QUEUE_MAX_DEPTH,
                "timeout_seconds": settings.QUEUE_TIMEOUT_SECONDS,
            }
            for agent, q in self._queues.items()
        }

    async def rate_limit_stats(self) -> dict:
        keys = await self._r.keys(f"{RL_STATS_NS}:*")
        result = {}
        for key in keys:
            key_str = key.decode() if isinstance(key, bytes) else key
            agent_name = key_str.split(":")[-1]
            raw = await self._r.hgetall(key)
            result[agent_name] = {
                (k.decode() if isinstance(k, bytes) else k): int(v)
                for k, v in raw.items()
            }
        return result

from __future__ import annotations

import asyncio
import time
from typing import Any

from request_manager import redis_keys
from request_manager.models import AcquireResult, AgentLimits


class LocalFallbackLimiter:
    def __init__(self, global_active_cap: int) -> None:
        self.global_semaphore = asyncio.Semaphore(global_active_cap)
        self.agent_semaphores: dict[str, asyncio.Semaphore] = {}

    def _agent_semaphore(self, agent_id: str, limits: AgentLimits) -> asyncio.Semaphore:
        if agent_id not in self.agent_semaphores:
            self.agent_semaphores[agent_id] = asyncio.Semaphore(max(1, limits.max_concurrency))
        return self.agent_semaphores[agent_id]

    async def acquire(self, agent_id: str, limits: AgentLimits, request_id: str) -> AcquireResult:
        del request_id

        agent_semaphore = self._agent_semaphore(agent_id, limits)
        start = time.monotonic()
        deadline = start + (limits.max_queue_wait_ms / 1000)
        global_acquired = False
        agent_acquired = False

        try:
            await asyncio.wait_for(self.global_semaphore.acquire(), timeout=limits.max_queue_wait_ms / 1000)
            global_acquired = True

            remaining = max(deadline - time.monotonic(), 0)
            await asyncio.wait_for(agent_semaphore.acquire(), timeout=remaining)
            agent_acquired = True

            wait_ms = int((time.monotonic() - start) * 1000)
            return AcquireResult(acquired=True, queued=wait_ms > 0, queue_wait_ms=wait_ms, degraded=True)
        except asyncio.TimeoutError:
            if agent_acquired:
                agent_semaphore.release()
            if global_acquired:
                self.global_semaphore.release()
            return AcquireResult(
                acquired=False,
                queued=True,
                reason="degraded-local-timeout",
                retry_after_seconds=1,
                degraded=True,
            )

    async def release(self, agent_id: str) -> None:
        agent_semaphore = self.agent_semaphores.get(agent_id)
        if agent_semaphore is not None:
            agent_semaphore.release()
        self.global_semaphore.release()


class RequestLimiter:
    def __init__(self, redis_client: Any, global_active_cap: int) -> None:
        self.redis = redis_client
        self.global_active_cap = global_active_cap
        self.local = LocalFallbackLimiter(global_active_cap)

    async def acquire(self, agent_id: str, limits: AgentLimits, request_id: str) -> AcquireResult:
        try:
            return await self._distributed_acquire(agent_id, limits, request_id)
        except Exception:
            return await self.local.acquire(agent_id, limits, request_id)

    async def _distributed_acquire(self, agent_id: str, limits: AgentLimits, request_id: str) -> AcquireResult:
        if await self._try_acquire_capacity(agent_id, limits):
            return AcquireResult(acquired=True)

        queue_key = redis_keys.queue(agent_id)
        queue_length = await self.redis.llen(queue_key)
        if queue_length >= limits.max_queue_depth:
            return AcquireResult(acquired=False, reason="queue-full", retry_after_seconds=1)

        await self.redis.rpush(queue_key, request_id)
        start = time.monotonic()
        deadline = start + (limits.max_queue_wait_ms / 1000)

        try:
            while time.monotonic() < deadline:
                head = await self.redis.lindex(queue_key, 0)
                if self._decode(head) == request_id and await self._try_acquire_capacity(agent_id, limits):
                    await self.redis.lpop(queue_key)
                    wait_ms = int((time.monotonic() - start) * 1000)
                    return AcquireResult(acquired=True, queued=True, queue_wait_ms=wait_ms)
                await asyncio.sleep(0.05)

            return AcquireResult(acquired=False, queued=True, reason="queue-timeout", retry_after_seconds=1)
        finally:
            await self.redis.lrem(queue_key, 0, request_id)

    async def release(self, agent_id: str, degraded: bool = False) -> None:
        if degraded:
            await self.local.release(agent_id)
            return

        await self._safe_decr(redis_keys.active(agent_id))
        await self._safe_decr(redis_keys.active_global())

    async def _try_acquire_capacity(self, agent_id: str, limits: AgentLimits) -> bool:
        agent_active = await self._get_int(redis_keys.active(agent_id))
        global_active = await self._get_int(redis_keys.active_global())

        if agent_active >= limits.max_concurrency:
            return False
        if global_active >= self.global_active_cap:
            return False
        if not await self._consume_token(agent_id, limits):
            return False

        await self.redis.incr(redis_keys.active(agent_id))
        await self.redis.incr(redis_keys.active_global())
        return True

    async def _consume_token(self, agent_id: str, limits: AgentLimits) -> bool:
        key = redis_keys.bucket(agent_id)
        bucket = await self.redis.hgetall(key)
        now = time.monotonic()

        tokens = float(self._decode(bucket.get("tokens")) or limits.burst_capacity)
        updated_at = float(self._decode(bucket.get("updated_at")) or now)
        elapsed = max(now - updated_at, 0)
        tokens = min(limits.burst_capacity, tokens + elapsed * limits.sustained_rps)

        if tokens < 1:
            await self.redis.hset(key, mapping={"tokens": tokens, "updated_at": now})
            await self.redis.expire(key, 3600)
            return False

        await self.redis.hset(key, mapping={"tokens": tokens - 1, "updated_at": now})
        await self.redis.expire(key, 3600)
        return True

    async def _safe_decr(self, key: str) -> None:
        current = await self._get_int(key)
        if current <= 0:
            await self.redis.set(key, "0")
            return

        updated = await self.redis.decr(key)
        if int(self._decode(updated) or "0") < 0:
            await self.redis.set(key, "0")

    async def _get_int(self, key: str) -> int:
        value = await self.redis.get(key)
        return int(self._decode(value) or "0")

    def _decode(self, value: Any) -> str | None:
        if value is None:
            return None
        if isinstance(value, bytes):
            return value.decode("utf-8")
        return str(value)

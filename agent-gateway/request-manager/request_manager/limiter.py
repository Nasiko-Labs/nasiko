from __future__ import annotations

import asyncio
import time
from typing import Any, Awaitable, Callable, TypeVar

from request_manager import redis_keys
from request_manager.models import AcquireResult, AgentLimits

T = TypeVar("T")


class RedisLimiterUnavailable(Exception):
    """Raised when a Redis command needed by the limiter cannot complete."""


_TRY_ACQUIRE_SCRIPT = """
local active_agent = tonumber(redis.call("GET", KEYS[1]) or "0")
local active_global = tonumber(redis.call("GET", KEYS[2]) or "0")
local request_id = ARGV[6]

if redis.call("SISMEMBER", KEYS[4], request_id) == 1 then
  return 1
end

if active_agent >= tonumber(ARGV[1]) then
  return 0
end
if active_global >= tonumber(ARGV[2]) then
  return 0
end

local burst_capacity = tonumber(ARGV[3])
local sustained_rps = tonumber(ARGV[4])
local now = tonumber(ARGV[5])
local tokens = tonumber(redis.call("HGET", KEYS[3], "tokens") or ARGV[3])
local updated_at = tonumber(redis.call("HGET", KEYS[3], "updated_at") or ARGV[5])
local elapsed = math.max(now - updated_at, 0)
tokens = math.min(burst_capacity, tokens + elapsed * sustained_rps)

if tokens < 1 then
  redis.call("HSET", KEYS[3], "tokens", tokens, "updated_at", now)
  redis.call("EXPIRE", KEYS[3], 3600)
  return 0
end

redis.call("HSET", KEYS[3], "tokens", tokens - 1, "updated_at", now)
redis.call("EXPIRE", KEYS[3], 3600)
redis.call("INCR", KEYS[1])
redis.call("INCR", KEYS[2])
redis.call("SADD", KEYS[4], request_id)
return 1
"""


_ENQUEUE_SCRIPT = """
if redis.call("LLEN", KEYS[1]) >= tonumber(ARGV[1]) then
  return 0
end

redis.call("RPUSH", KEYS[1], ARGV[2])
return 1
"""


_TRY_ACQUIRE_HEAD_SCRIPT = """
local request_id = ARGV[6]
if redis.call("LINDEX", KEYS[1], 0) ~= request_id then
  return 0
end

if redis.call("SISMEMBER", KEYS[5], request_id) == 1 then
  local popped = redis.call("LPOP", KEYS[1])
  if popped == request_id then
    return 1
  end
  return 0
end

local active_agent = tonumber(redis.call("GET", KEYS[2]) or "0")
local active_global = tonumber(redis.call("GET", KEYS[3]) or "0")
if active_agent >= tonumber(ARGV[1]) then
  return 0
end
if active_global >= tonumber(ARGV[2]) then
  return 0
end

local burst_capacity = tonumber(ARGV[3])
local sustained_rps = tonumber(ARGV[4])
local now = tonumber(ARGV[5])
local tokens = tonumber(redis.call("HGET", KEYS[4], "tokens") or ARGV[3])
local updated_at = tonumber(redis.call("HGET", KEYS[4], "updated_at") or ARGV[5])
local elapsed = math.max(now - updated_at, 0)
tokens = math.min(burst_capacity, tokens + elapsed * sustained_rps)

if tokens < 1 then
  redis.call("HSET", KEYS[4], "tokens", tokens, "updated_at", now)
  redis.call("EXPIRE", KEYS[4], 3600)
  return 0
end

local popped = redis.call("LPOP", KEYS[1])
if popped ~= request_id then
  return 0
end

redis.call("HSET", KEYS[4], "tokens", tokens - 1, "updated_at", now)
redis.call("EXPIRE", KEYS[4], 3600)
redis.call("INCR", KEYS[2])
redis.call("INCR", KEYS[3])
redis.call("SADD", KEYS[5], request_id)
return 1
"""


_RELEASE_SCRIPT = """
local request_id = ARGV[1]
if request_id == "" or redis.call("SISMEMBER", KEYS[3], request_id) == 0 then
  return 0
end

redis.call("SREM", KEYS[3], request_id)

local active_agent = tonumber(redis.call("GET", KEYS[1]) or "0")
local active_global = tonumber(redis.call("GET", KEYS[2]) or "0")
redis.call("SET", KEYS[1], math.max(active_agent - 1, 0))
redis.call("SET", KEYS[2], math.max(active_global - 1, 0))
return 1
"""


class LocalFallbackLimiter:
    def __init__(self, global_active_cap: int) -> None:
        self.global_semaphore = asyncio.BoundedSemaphore(global_active_cap)
        self.agent_semaphores: dict[str, asyncio.BoundedSemaphore] = {}
        self.active_requests: dict[str, str] = {}
        self._lock = asyncio.Lock()

    def _agent_semaphore(self, agent_id: str, limits: AgentLimits) -> asyncio.BoundedSemaphore:
        if agent_id not in self.agent_semaphores:
            self.agent_semaphores[agent_id] = asyncio.BoundedSemaphore(max(1, limits.max_concurrency))
        return self.agent_semaphores[agent_id]

    async def acquire(self, agent_id: str, limits: AgentLimits, request_id: str) -> AcquireResult:
        async with self._lock:
            if request_id in self.active_requests:
                return AcquireResult(acquired=True, degraded=True)

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

            async with self._lock:
                if request_id in self.active_requests:
                    agent_semaphore.release()
                    self.global_semaphore.release()
                    return AcquireResult(acquired=True, degraded=True)
                self.active_requests[request_id] = agent_id

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

    async def release(self, agent_id: str, request_id: str | None = None) -> None:
        if request_id is None:
            return

        async with self._lock:
            if self.active_requests.get(request_id) != agent_id:
                return
            del self.active_requests[request_id]

        agent_semaphore = self.agent_semaphores.get(agent_id)
        if agent_semaphore is not None:
            agent_semaphore.release()
        self.global_semaphore.release()


class RequestLimiter:
    def __init__(self, redis_client: Any, global_active_cap: int) -> None:
        self.redis = redis_client
        self.global_active_cap = global_active_cap
        self.local = LocalFallbackLimiter(global_active_cap)
        self._fallback_lock = asyncio.Lock()

    async def acquire(self, agent_id: str, limits: AgentLimits, request_id: str) -> AcquireResult:
        try:
            return await self._distributed_acquire(agent_id, limits, request_id)
        except RedisLimiterUnavailable:
            return await self.local.acquire(agent_id, limits, request_id)

    async def _distributed_acquire(self, agent_id: str, limits: AgentLimits, request_id: str) -> AcquireResult:
        if await self._try_acquire_capacity(agent_id, limits, request_id):
            return AcquireResult(acquired=True)

        if not await self._enqueue(agent_id, limits, request_id):
            return AcquireResult(acquired=False, reason="queue-full", retry_after_seconds=1)

        start = time.monotonic()
        deadline = start + (limits.max_queue_wait_ms / 1000)

        try:
            while time.monotonic() < deadline:
                if await self._try_acquire_head(agent_id, limits, request_id):
                    wait_ms = int((time.monotonic() - start) * 1000)
                    return AcquireResult(acquired=True, queued=True, queue_wait_ms=wait_ms)

                remaining = max(deadline - time.monotonic(), 0)
                if remaining <= 0:
                    break
                await asyncio.sleep(min(0.05, remaining))

            return AcquireResult(acquired=False, queued=True, reason="queue-timeout", retry_after_seconds=1)
        finally:
            try:
                await self._remove_from_queue(agent_id, request_id)
            except RedisLimiterUnavailable:
                # Do not switch to local fallback after Redis may have granted a distributed slot.
                pass

    async def release(
        self,
        agent_id: str,
        request_id: str | None = None,
        degraded: bool = False,
    ) -> None:
        if degraded:
            await self.local.release(agent_id, request_id)
            return
        if request_id is None:
            return

        if self._has_eval:
            await self._release_with_lua(agent_id, request_id)
            return

        async with self._fallback_lock:
            await self._release_with_commands(agent_id, request_id)

    async def _try_acquire_capacity(self, agent_id: str, limits: AgentLimits, request_id: str) -> bool:
        if self._has_eval:
            result = await self._eval(
                _TRY_ACQUIRE_SCRIPT,
                [
                    redis_keys.active(agent_id),
                    redis_keys.active_global(),
                    redis_keys.bucket(agent_id),
                    self._active_request_set(agent_id),
                ],
                self._acquire_args(limits, request_id),
            )
            return self._truthy(result)

        async with self._fallback_lock:
            return await self._try_acquire_capacity_with_commands(agent_id, limits, request_id)

    async def _enqueue(self, agent_id: str, limits: AgentLimits, request_id: str) -> bool:
        if self._has_eval:
            result = await self._eval(
                _ENQUEUE_SCRIPT,
                [redis_keys.queue(agent_id)],
                [limits.max_queue_depth, request_id],
            )
            return self._truthy(result)

        async with self._fallback_lock:
            queue_key = redis_keys.queue(agent_id)
            queue_length = int(await self._call(self.redis.llen, queue_key))
            if queue_length >= limits.max_queue_depth:
                return False
            await self._call(self.redis.rpush, queue_key, request_id)
            return True

    async def _try_acquire_head(self, agent_id: str, limits: AgentLimits, request_id: str) -> bool:
        if self._has_eval:
            result = await self._eval(
                _TRY_ACQUIRE_HEAD_SCRIPT,
                [
                    redis_keys.queue(agent_id),
                    redis_keys.active(agent_id),
                    redis_keys.active_global(),
                    redis_keys.bucket(agent_id),
                    self._active_request_set(agent_id),
                ],
                self._acquire_args(limits, request_id),
            )
            return self._truthy(result)

        async with self._fallback_lock:
            queue_key = redis_keys.queue(agent_id)
            head = await self._call(self.redis.lindex, queue_key, 0)
            if self._decode(head) != request_id:
                return False
            if not await self._try_acquire_capacity_with_commands(agent_id, limits, request_id):
                return False
            popped = await self._call(self.redis.lpop, queue_key)
            if self._decode(popped) != request_id:
                await self._release_with_commands(agent_id, request_id)
                return False
            return True

    async def _try_acquire_capacity_with_commands(
        self,
        agent_id: str,
        limits: AgentLimits,
        request_id: str,
    ) -> bool:
        active_request_key = self._active_request_set(agent_id)
        active_requests = await self._call(self.redis.smembers, active_request_key)
        if request_id in {self._decode(value) for value in active_requests}:
            return True

        agent_active = await self._get_int(redis_keys.active(agent_id))
        global_active = await self._get_int(redis_keys.active_global())

        if agent_active >= limits.max_concurrency:
            return False
        if global_active >= self.global_active_cap:
            return False
        if not await self._consume_token(agent_id, limits):
            return False

        await self._call(self.redis.incr, redis_keys.active(agent_id))
        await self._call(self.redis.incr, redis_keys.active_global())
        await self._call(self.redis.sadd, active_request_key, request_id)
        return True

    async def _consume_token(self, agent_id: str, limits: AgentLimits) -> bool:
        key = redis_keys.bucket(agent_id)
        bucket = await self._call(self.redis.hgetall, key)
        now = time.time()

        tokens = float(self._bucket_value(bucket, "tokens") or limits.burst_capacity)
        updated_at = float(self._bucket_value(bucket, "updated_at") or now)
        elapsed = max(now - updated_at, 0)
        tokens = min(limits.burst_capacity, tokens + elapsed * limits.sustained_rps)

        if tokens < 1:
            await self._call(self.redis.hset, key, mapping={"tokens": tokens, "updated_at": now})
            await self._call(self.redis.expire, key, 3600)
            return False

        await self._call(self.redis.hset, key, mapping={"tokens": tokens - 1, "updated_at": now})
        await self._call(self.redis.expire, key, 3600)
        return True

    async def _release_with_lua(self, agent_id: str, request_id: str) -> None:
        await self._eval(
            _RELEASE_SCRIPT,
            [
                redis_keys.active(agent_id),
                redis_keys.active_global(),
                self._active_request_set(agent_id),
            ],
            [request_id],
        )

    async def _release_with_commands(self, agent_id: str, request_id: str) -> None:
        active_request_key = self._active_request_set(agent_id)
        active_requests = await self._call(self.redis.smembers, active_request_key)
        if request_id not in {self._decode(value) for value in active_requests}:
            return

        await self._call(self.redis.srem, active_request_key, request_id)
        await self._safe_decr(redis_keys.active(agent_id))
        await self._safe_decr(redis_keys.active_global())

    async def _remove_from_queue(self, agent_id: str, request_id: str) -> None:
        await self._call(self.redis.lrem, redis_keys.queue(agent_id), 0, request_id)

    async def _safe_decr(self, key: str) -> None:
        current = await self._get_int(key)
        await self._call(self.redis.set, key, str(max(current - 1, 0)))

    async def _get_int(self, key: str) -> int:
        value = await self._call(self.redis.get, key)
        return int(self._decode(value) or "0")

    async def _eval(self, script: str, keys: list[str], args: list[Any]) -> Any:
        return await self._call(self.redis.eval, script, len(keys), *keys, *args)

    async def _call(self, command: Callable[..., Awaitable[T]], *args: Any, **kwargs: Any) -> T:
        try:
            return await command(*args, **kwargs)
        except (AttributeError, NameError, TypeError, ValueError):
            raise
        except Exception as exc:
            raise RedisLimiterUnavailable(str(exc)) from exc

    def _acquire_args(self, limits: AgentLimits, request_id: str) -> list[Any]:
        return [
            limits.max_concurrency,
            self.global_active_cap,
            limits.burst_capacity,
            limits.sustained_rps,
            time.time(),
            request_id,
        ]

    def _active_request_set(self, agent_id: str) -> str:
        return f"{redis_keys.active(agent_id)}:requests"

    @property
    def _has_eval(self) -> bool:
        return callable(getattr(self.redis, "eval", None))

    def _bucket_value(self, bucket: dict[Any, Any], field: str) -> str | None:
        if field in bucket:
            return self._decode(bucket[field])
        field_bytes = field.encode("utf-8")
        if field_bytes in bucket:
            return self._decode(bucket[field_bytes])
        return None

    def _truthy(self, value: Any) -> bool:
        return self._decode(value) == "1"

    def _decode(self, value: Any) -> str | None:
        if value is None:
            return None
        if isinstance(value, bytes):
            return value.decode("utf-8")
        return str(value)

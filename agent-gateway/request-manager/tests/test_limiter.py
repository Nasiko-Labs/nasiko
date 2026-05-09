from __future__ import annotations

import asyncio

import pytest

from request_manager.limiter import LocalFallbackLimiter, RequestLimiter
from request_manager.models import AgentLimits


class EvalRedis:
    def __init__(self) -> None:
        self.calls: list[tuple[str, int, tuple[object, ...]]] = []

    async def eval(self, script: str, numkeys: int, *keys_and_args: object) -> int:
        self.calls.append((script, numkeys, keys_and_args))
        return 1


def limits(
    max_concurrency: int = 1,
    max_queue_depth: int = 1,
    max_queue_wait_ms: int = 50,
    sustained_rps: float = 100,
    burst_capacity: int = 100,
) -> AgentLimits:
    return AgentLimits(
        cache_ttl_seconds=600,
        max_concurrency=max_concurrency,
        sustained_rps=sustained_rps,
        burst_capacity=burst_capacity,
        max_queue_depth=max_queue_depth,
        max_queue_wait_ms=max_queue_wait_ms,
        cache_enabled=True,
    )


@pytest.mark.asyncio
async def test_acquires_when_capacity_available(fake_redis):
    limiter = RequestLimiter(fake_redis, global_active_cap=50)

    result = await limiter.acquire("agent-a2a-demo", limits(), request_id="req-1")

    assert result.acquired is True
    assert result.queued is False


@pytest.mark.asyncio
async def test_uses_lua_eval_when_available():
    redis = EvalRedis()
    limiter = RequestLimiter(redis, global_active_cap=50)

    result = await limiter.acquire("agent-a2a-demo", limits(), request_id="req-1")

    assert result.acquired is True
    assert len(redis.calls) == 1
    assert "SISMEMBER" in redis.calls[0][0]


@pytest.mark.asyncio
async def test_releases_capacity(fake_redis):
    limiter = RequestLimiter(fake_redis, global_active_cap=50)
    await limiter.acquire("agent-a2a-demo", limits(), request_id="req-1")

    await limiter.release("agent-a2a-demo", request_id="req-1")

    assert int(await fake_redis.get("request-manager:active:agent-a2a-demo")) == 0
    assert int(await fake_redis.get("request-manager:active:global")) == 0


@pytest.mark.asyncio
async def test_returns_overflow_when_queue_full(fake_redis):
    limiter = RequestLimiter(fake_redis, global_active_cap=50)
    await limiter.acquire("agent-a2a-demo", limits(), request_id="req-1")
    await fake_redis.rpush("request-manager:queue:agent-a2a-demo", "already-queued")

    result = await limiter.acquire("agent-a2a-demo", limits(), request_id="req-2")

    assert result.acquired is False
    assert result.reason == "queue-full"


@pytest.mark.asyncio
async def test_concurrent_acquire_admits_only_one_request(fake_redis):
    limiter = RequestLimiter(fake_redis, global_active_cap=50)

    results = await asyncio.gather(
        limiter.acquire("agent-a2a-demo", limits(max_queue_depth=0), request_id="req-1"),
        limiter.acquire("agent-a2a-demo", limits(max_queue_depth=0), request_id="req-2"),
    )

    assert sum(result.acquired for result in results) == 1
    assert int(await fake_redis.get("request-manager:active:agent-a2a-demo")) == 1
    assert int(await fake_redis.get("request-manager:active:global")) == 1


@pytest.mark.asyncio
async def test_token_bucket_does_not_immediately_admit_second_request(fake_redis):
    limiter = RequestLimiter(fake_redis, global_active_cap=50)
    agent_limits = limits(
        max_concurrency=10,
        max_queue_depth=0,
        sustained_rps=0.001,
        burst_capacity=1,
    )

    first = await limiter.acquire("agent-a2a-demo", agent_limits, request_id="req-1")
    second = await limiter.acquire("agent-a2a-demo", agent_limits, request_id="req-2")

    assert first.acquired is True
    assert second.acquired is False
    assert second.reason == "queue-full"
    assert int(await fake_redis.get("request-manager:active:agent-a2a-demo")) == 1


@pytest.mark.asyncio
async def test_queued_request_acquires_after_release_preserving_fifo(fake_redis):
    limiter = RequestLimiter(fake_redis, global_active_cap=50)
    agent_limits = limits(max_queue_depth=2, max_queue_wait_ms=200)
    first = await limiter.acquire("agent-a2a-demo", agent_limits, request_id="req-1")

    queued = asyncio.create_task(limiter.acquire("agent-a2a-demo", agent_limits, request_id="req-2"))
    queue_key = "request-manager:queue:agent-a2a-demo"
    for _ in range(10):
        if list(fake_redis.lists[queue_key]) == ["req-2"]:
            break
        await asyncio.sleep(0.01)
    await limiter.release("agent-a2a-demo", request_id="req-1")
    result = await queued

    assert first.acquired is True
    assert result.acquired is True
    assert result.queued is True
    assert list(fake_redis.lists[queue_key]) == []
    assert await fake_redis.smembers("request-manager:active:agent-a2a-demo:requests") == {"req-2"}


@pytest.mark.asyncio
async def test_queue_timeout_cleans_request_id_from_queue(fake_redis):
    limiter = RequestLimiter(fake_redis, global_active_cap=50)
    agent_limits = limits(max_queue_depth=1, max_queue_wait_ms=1)
    await limiter.acquire("agent-a2a-demo", agent_limits, request_id="req-1")

    result = await limiter.acquire("agent-a2a-demo", agent_limits, request_id="req-2")

    assert result.acquired is False
    assert result.reason == "queue-timeout"
    assert list(fake_redis.lists["request-manager:queue:agent-a2a-demo"]) == []


@pytest.mark.asyncio
async def test_atomic_pop_does_not_remove_another_request_when_head_differs(fake_redis):
    limiter = RequestLimiter(fake_redis, global_active_cap=50)
    agent_limits = limits(max_concurrency=2)
    await fake_redis.rpush("request-manager:queue:agent-a2a-demo", "req-head")

    acquired = await limiter._try_acquire_head("agent-a2a-demo", agent_limits, "req-other")

    assert acquired is False
    assert list(fake_redis.lists["request-manager:queue:agent-a2a-demo"]) == ["req-head"]
    assert await fake_redis.get("request-manager:active:agent-a2a-demo") is None


@pytest.mark.asyncio
async def test_release_with_wrong_or_missing_request_id_does_not_decrement(fake_redis):
    limiter = RequestLimiter(fake_redis, global_active_cap=50)
    await limiter.acquire("agent-a2a-demo", limits(), request_id="req-1")

    await limiter.release("agent-a2a-demo", request_id="wrong")
    await limiter.release("agent-a2a-demo")

    assert int(await fake_redis.get("request-manager:active:agent-a2a-demo")) == 1
    assert int(await fake_redis.get("request-manager:active:global")) == 1
    assert await fake_redis.smembers("request-manager:active:agent-a2a-demo:requests") == {"req-1"}


@pytest.mark.asyncio
async def test_local_fallback_duplicate_release_does_not_over_release():
    limiter = LocalFallbackLimiter(global_active_cap=1)
    agent_limits = limits(max_queue_wait_ms=1)
    result = await limiter.acquire("agent-a2a-demo", agent_limits, request_id="req-1")

    await limiter.release("agent-a2a-demo", request_id="req-1")
    await limiter.release("agent-a2a-demo", request_id="req-1")
    reacquired = await limiter.acquire("agent-a2a-demo", agent_limits, request_id="req-2")
    blocked = await limiter.acquire("agent-a2a-demo", agent_limits, request_id="req-3")

    assert result.acquired is True
    assert reacquired.acquired is True
    assert blocked.acquired is False
    assert blocked.reason == "degraded-local-timeout"

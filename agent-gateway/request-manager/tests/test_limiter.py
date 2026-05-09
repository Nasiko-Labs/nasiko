import pytest

from request_manager.limiter import RequestLimiter
from request_manager.models import AgentLimits


def limits(
    max_concurrency: int = 1,
    max_queue_depth: int = 1,
    max_queue_wait_ms: int = 50,
) -> AgentLimits:
    return AgentLimits(
        cache_ttl_seconds=600,
        max_concurrency=max_concurrency,
        sustained_rps=100,
        burst_capacity=100,
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
async def test_releases_capacity(fake_redis):
    limiter = RequestLimiter(fake_redis, global_active_cap=50)
    await limiter.acquire("agent-a2a-demo", limits(), request_id="req-1")

    await limiter.release("agent-a2a-demo")

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

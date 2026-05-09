import pytest
import time
from gateway.rate_limiter.token_bucket import RateLimiter


@pytest.fixture
def limiter():
    rl = RateLimiter()
    rl._use_redis = False  # local mode
    return rl


@pytest.mark.asyncio
async def test_allows_within_limit(limiter):
    limiter.configure_agent("agent-a", rps=10, burst=5)
    # Should allow up to burst
    results = [await limiter.is_allowed("agent-a") for _ in range(5)]
    assert all(results)


@pytest.mark.asyncio
async def test_denies_over_burst(limiter):
    limiter.configure_agent("agent-a", rps=1, burst=2)
    results = [await limiter.is_allowed("agent-a") for _ in range(5)]
    # First 2 allowed, rest denied
    assert results[:2] == [True, True]
    assert not all(results[2:])


@pytest.mark.asyncio
async def test_tokens_refill_over_time(limiter):
    limiter.configure_agent("agent-a", rps=10, burst=1)
    await limiter.is_allowed("agent-a")  # consume token
    # Should be denied immediately
    assert not await limiter.is_allowed("agent-a")
    # Wait for refill
    await __import__('asyncio').sleep(0.15)
    assert await limiter.is_allowed("agent-a")


@pytest.mark.asyncio
async def test_per_agent_isolation(limiter):
    limiter.configure_agent("agent-a", rps=100, burst=100)
    limiter.configure_agent("agent-b", rps=1, burst=1)

    assert await limiter.is_allowed("agent-a")
    await limiter.is_allowed("agent-b")  # consume
    assert not await limiter.is_allowed("agent-b")  # denied
    assert await limiter.is_allowed("agent-a")  # still fine


@pytest.mark.asyncio
async def test_dynamic_config_update(limiter):
    limiter.configure_agent("agent-a", rps=1, burst=1)
    await limiter.is_allowed("agent-a")  # consume
    assert not await limiter.is_allowed("agent-a")

    # Increase burst
    limiter.update_config("agent-a", burst=100)
    # Re-initialize local state for test
    limiter._local_tokens["agent-a"] = 100
    assert await limiter.is_allowed("agent-a")


@pytest.mark.asyncio
async def test_stats_tracking(limiter):
    limiter.configure_agent("agent-a", rps=1, burst=2)
    await limiter.is_allowed("agent-a")
    await limiter.is_allowed("agent-a")
    await limiter.is_allowed("agent-a")  # should be denied

    stats = limiter.get_stats("agent-a")
    assert stats["allowed"] == 2
    assert stats["denied"] == 1

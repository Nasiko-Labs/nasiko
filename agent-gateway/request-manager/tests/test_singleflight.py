import pytest

from request_manager.singleflight import SingleFlight


@pytest.mark.asyncio
async def test_first_request_owns_singleflight_lock(fake_redis):
    singleflight = SingleFlight(fake_redis, wait_ms=1000)

    claim = await singleflight.claim("cache-key")

    assert claim.owner is True
    assert claim.cache_key == "cache-key"


@pytest.mark.asyncio
async def test_second_request_waits_when_lock_exists(fake_redis):
    singleflight = SingleFlight(fake_redis, wait_ms=50)
    first = await singleflight.claim("cache-key")
    second = await singleflight.claim("cache-key")

    assert first.owner is True
    assert second.owner is False


@pytest.mark.asyncio
async def test_release_marks_ready_and_removes_lock(fake_redis):
    singleflight = SingleFlight(fake_redis, wait_ms=1000)
    claim = await singleflight.claim("cache-key")

    await singleflight.release(claim)

    assert await fake_redis.get("request-manager:singleflight:cache-key") is None
    assert await fake_redis.get("request-manager:singleflight:ready:cache-key") == "1"

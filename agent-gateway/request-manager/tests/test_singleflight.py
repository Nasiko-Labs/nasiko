import pytest

from request_manager.singleflight import SingleFlight, SingleFlightClaim


class GetFailingRedis:
    async def get(self, key: str):
        raise RuntimeError("redis unavailable")


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


@pytest.mark.asyncio
async def test_wait_until_ready_returns_true_when_ready_marker_exists(fake_redis):
    singleflight = SingleFlight(fake_redis, wait_ms=1000)
    await fake_redis.set("request-manager:singleflight:ready:cache-key", "1")

    assert await singleflight.wait_until_ready("cache-key") is True


@pytest.mark.asyncio
async def test_wait_until_ready_returns_false_when_lock_disappears_without_ready(fake_redis):
    singleflight = SingleFlight(fake_redis, wait_ms=1)
    claim = await singleflight.claim("cache-key")
    await fake_redis.delete("request-manager:singleflight:cache-key")

    assert claim.owner is True
    assert await singleflight.wait_until_ready("cache-key") is False


@pytest.mark.asyncio
async def test_wait_until_ready_returns_false_on_redis_get_exception():
    singleflight = SingleFlight(GetFailingRedis(), wait_ms=1000)

    assert await singleflight.wait_until_ready("cache-key") is False


@pytest.mark.asyncio
async def test_release_does_not_delete_lock_when_token_no_longer_matches(fake_redis):
    singleflight = SingleFlight(fake_redis, wait_ms=1000)
    claim = await singleflight.claim("cache-key")
    await fake_redis.set("request-manager:singleflight:cache-key", "different-token")

    await singleflight.release(claim)

    assert await fake_redis.get("request-manager:singleflight:cache-key") == "different-token"
    assert await fake_redis.get("request-manager:singleflight:ready:cache-key") is None


@pytest.mark.asyncio
async def test_release_does_nothing_for_non_owner_claim(fake_redis):
    singleflight = SingleFlight(fake_redis, wait_ms=1000)
    claim = SingleFlightClaim(cache_key="cache-key", token="token", owner=False)
    await fake_redis.set("request-manager:singleflight:cache-key", "token")

    await singleflight.release(claim)

    assert await fake_redis.get("request-manager:singleflight:cache-key") == "token"
    assert await fake_redis.get("request-manager:singleflight:ready:cache-key") is None

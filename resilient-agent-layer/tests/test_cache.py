import pytest
import asyncio
from gateway.cache.cache_manager import CacheManager
from gateway.cache.key_builder import build_cache_key


@pytest.fixture
def cache():
    c = CacheManager()
    c._use_redis = False  # force LRU for unit tests
    return c


@pytest.mark.asyncio
async def test_cache_miss_returns_none(cache):
    result = await cache.get("agent-a", {"query": "hello"})
    assert result is None


@pytest.mark.asyncio
async def test_cache_set_and_get(cache):
    payload = {"query": "What is 2+2?"}
    response = {"result": "4", "agent": "agent-a"}

    await cache.set("agent-a", payload, response, ttl=60)
    result = await cache.get("agent-a", payload)

    assert result == response


@pytest.mark.asyncio
async def test_cache_key_is_deterministic(cache):
    payload = {"query": "hello", "ctx": "world"}
    k1 = build_cache_key("agent-a", payload)
    k2 = build_cache_key("agent-a", {"ctx": "world", "query": "hello"})  # different order
    assert k1 == k2


@pytest.mark.asyncio
async def test_cache_different_agents_dont_collide(cache):
    payload = {"query": "same query"}
    await cache.set("agent-a", payload, {"from": "a"}, ttl=60)
    await cache.set("agent-b", payload, {"from": "b"}, ttl=60)

    assert (await cache.get("agent-a", payload))["from"] == "a"
    assert (await cache.get("agent-b", payload))["from"] == "b"


@pytest.mark.asyncio
async def test_flush_all(cache):
    await cache.set("agent-a", {"q": "1"}, {"r": "1"}, ttl=60)
    await cache.flush_all()
    assert await cache.get("agent-a", {"q": "1"}) is None


@pytest.mark.asyncio
async def test_flush_agent(cache):
    await cache.set("agent-a", {"q": "1"}, {"r": "1"}, ttl=60)
    await cache.set("agent-b", {"q": "1"}, {"r": "2"}, ttl=60)
    await cache.flush_agent("agent-a")

    assert await cache.get("agent-a", {"q": "1"}) is None
    assert await cache.get("agent-b", {"q": "1"}) is not None


@pytest.mark.asyncio
async def test_hit_rate_tracking(cache):
    payload = {"q": "x"}
    await cache.set("agent-a", payload, {"r": "x"}, ttl=60)
    await cache.get("agent-a", payload)  # hit
    await cache.get("agent-a", {"q": "miss"})  # miss

    stats = cache.get_stats("agent-a")
    assert stats["hits"] == 1
    assert stats["misses"] == 1
    assert stats["hit_rate"] == 50.0

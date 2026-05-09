"""Unit tests for the L1 exact-match response cache."""

from datetime import datetime, timezone

import pytest

from request_layer.src.cache import exact as exact_cache
from request_layer.src.types import CacheEntry


def _make_entry(body: str = "hello world", status: int = 200) -> CacheEntry:
    return exact_cache.serialize_entry(
        status_code=status,
        headers={"content-type": "application/json"},
        body=body,
        cost_usd=0.0012,
        latency_ms=950.0,
    )


@pytest.mark.asyncio
async def test_round_trip_preserves_body(fake_redis) -> None:
    entry = _make_entry(body='{"translated":"bonjour"}')
    await exact_cache.set(fake_redis, "translator", "translate hello", entry, ttl_seconds=600)
    out = await exact_cache.get(fake_redis, "translator", "translate hello")
    assert out is not None
    assert out.status_code == 200
    assert out.body == '{"translated":"bonjour"}'
    assert exact_cache.decode_body(out) == b'{"translated":"bonjour"}'


@pytest.mark.asyncio
async def test_miss_returns_none(fake_redis) -> None:
    out = await exact_cache.get(fake_redis, "translator", "never seen")
    assert out is None


@pytest.mark.asyncio
async def test_distinct_agents_have_distinct_keys(fake_redis) -> None:
    entry_a = _make_entry(body="A")
    entry_b = _make_entry(body="B")
    await exact_cache.set(fake_redis, "agent_a", "same query", entry_a, ttl_seconds=600)
    await exact_cache.set(fake_redis, "agent_b", "same query", entry_b, ttl_seconds=600)

    out_a = await exact_cache.get(fake_redis, "agent_a", "same query")
    out_b = await exact_cache.get(fake_redis, "agent_b", "same query")
    assert out_a is not None and out_a.body == "A"
    assert out_b is not None and out_b.body == "B"


@pytest.mark.asyncio
async def test_clear_one_agent_only(fake_redis) -> None:
    entry = _make_entry()
    await exact_cache.set(fake_redis, "agent_a", "q1", entry, ttl_seconds=600)
    await exact_cache.set(fake_redis, "agent_a", "q2", entry, ttl_seconds=600)
    await exact_cache.set(fake_redis, "agent_b", "q3", entry, ttl_seconds=600)

    deleted = await exact_cache.clear(fake_redis, agent="agent_a")
    assert deleted == 2
    assert await exact_cache.get(fake_redis, "agent_a", "q1") is None
    assert await exact_cache.get(fake_redis, "agent_b", "q3") is not None


def test_serialize_entry_records_cached_at() -> None:
    entry = _make_entry()
    assert isinstance(entry.cached_at, datetime)
    assert entry.cached_at.tzinfo is not None


def test_stable_hash_is_deterministic_and_distinct() -> None:
    a = exact_cache.stable_hash("hello world")
    b = exact_cache.stable_hash("hello world")
    c = exact_cache.stable_hash("hello WORLD")
    assert a == b
    assert a != c

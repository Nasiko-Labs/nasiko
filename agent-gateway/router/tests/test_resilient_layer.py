"""
Tests for the resilient request layer embedded in the Nasiko router.

Run from agent-gateway/ with:
    uv run pytest router/tests/test_resilient_layer.py -v
"""

import asyncio
import time
import pytest

from router.src.core.resilient_executor import (
    InMemoryCache,
    CacheConfig,
    TokenBucketRateLimiter,
    RuntimeStats,
    RequestLayerContext,
    ResilientAgentExecutor,
)


# ── Helpers ────────────────────────────────────────────────────────────────────

def fresh_cache(ttl: int = 300, max_size: int = 100) -> InMemoryCache:
    return InMemoryCache(CacheConfig(ttl=ttl, max_size=max_size))


def fresh_limiter() -> TokenBucketRateLimiter:
    return TokenBucketRateLimiter()


def fresh_stats() -> RuntimeStats:
    return RuntimeStats()


# ── 1. Exact cache hit ─────────────────────────────────────────────────────────

def test_exact_cache_hit():
    cache = fresh_cache()
    cache.set("agent-a", "hello world", "stored-response")
    value, status, age = cache.get("agent-a", "hello world")
    assert value == "stored-response"
    assert status == "HIT"
    assert age >= 0.0


# ── 2. Exact cache miss ────────────────────────────────────────────────────────

def test_exact_cache_miss():
    cache = fresh_cache()
    value, status, age = cache.get("agent-a", "never stored query")
    assert value is None
    assert status == "MISS"
    assert age == 0.0


# ── 3. Cache bypass via no-cache (skip_read) ──────────────────────────────────

def test_cache_bypass_no_cache():
    cache = fresh_cache()
    cache.set("agent-a", "repeated query", "cached-response")
    # skip_read=True simulates Cache-Control: no-cache
    value, status, age = cache.get("agent-a", "repeated query", skip_read=True)
    assert value is None
    assert status == "MISS"


# ── 4. Cache bypass via no-store (skip_store) ─────────────────────────────────

def test_cache_no_store():
    cache = fresh_cache()
    # skip_store=True simulates Cache-Control: no-store
    cache.set("agent-b", "fresh query", "fresh-response", skip_store=True)
    value, status, _ = cache.get("agent-b", "fresh query")
    assert value is None
    assert status == "MISS"
    assert cache.stores == 0


# ── 5. TTL expiry ──────────────────────────────────────────────────────────────

def test_cache_ttl_expiry():
    cache = fresh_cache(ttl=1)
    cache.set("agent-c", "expiring query", "expiring-response")
    # Entry should be there immediately
    value, status, _ = cache.get("agent-c", "expiring query")
    assert status == "HIT"
    # Wait for TTL to pass
    time.sleep(1.1)
    value, status, _ = cache.get("agent-c", "expiring query")
    assert value is None
    assert status == "MISS"


# ── 6. File requests are never cached ─────────────────────────────────────────

def test_cache_file_requests_not_cached():
    cache = fresh_cache()
    # Attempt to store with has_files=True
    cache.set("agent-d", "file query", "file-response", has_files=True)
    # Attempt to read with has_files=True
    value, status, _ = cache.get("agent-d", "file query", has_files=True)
    assert value is None
    assert status == "MISS"
    assert cache.stores == 0


# ── 7. Rate limiter allows within limit ────────────────────────────────────────

@pytest.mark.asyncio
async def test_rate_limit_allows_within_limit():
    limiter = fresh_limiter()
    limiter.set_limit("agent-x", 10)
    acquired, wait = await limiter.acquire("agent-x")
    assert acquired is True
    assert wait == 0.0


# ── 8. Rate limiter rejects when queue is full ────────────────────────────────

@pytest.mark.asyncio
async def test_rate_limit_rejects_when_queue_full():
    """Window full + artificially-filled queue → immediate rejection path."""
    import asyncio as aio

    limiter = fresh_limiter()
    min_rpm = 2   # MIN_RPM clamps set_limit floor to 2
    limiter.set_limit("agent-rl", min_rpm)

    # Consume ALL allowed tokens (both slots)
    for _ in range(min_rpm):
        ok, _ = await limiter.acquire("agent-rl")
        assert ok is True

    # Pre-inject a full queue so q.full() → True → immediate rejection
    full_q = aio.Queue(maxsize=1)
    await full_q.put("placeholder")
    limiter._queues["agent-rl"] = full_q

    # Next request: window full and queue full → immediate rejection
    ok, wait = await limiter.acquire("agent-rl")
    assert ok is False
    assert wait == -1.0


# ── 9. Adaptive loop reduces RPM on high latency ──────────────────────────────

def test_adaptive_reduces_limit_on_high_latency():
    limiter = fresh_limiter()
    limiter.set_limit("slow-agent", 20)
    # Feed 5 samples averaging 3 000 ms → > 2 000 ms threshold
    for _ in range(5):
        limiter.record_latency("slow-agent", 3.0)
    limiter._adapt_all()
    new_limit = limiter.get_limit("slow-agent")
    assert new_limit < 20, f"Expected reduced limit, got {new_limit}"
    assert new_limit >= 2   # never below MIN_RPM


# ── 10. Adaptive loop increases RPM on low latency ───────────────────────────

def test_adaptive_increases_limit_on_low_latency():
    limiter = fresh_limiter()
    limiter.set_limit("fast-agent", 10)
    # Feed samples averaging 200 ms → < 500 ms threshold
    for _ in range(5):
        limiter.record_latency("fast-agent", 0.2)
    limiter._adapt_all()
    new_limit = limiter.get_limit("fast-agent")
    assert new_limit > 10, f"Expected increased limit, got {new_limit}"
    assert new_limit <= 100  # never above MAX_RPM


# ── 11. Prometheus metrics text contains required metric names ────────────────

def test_prometheus_metrics_format():
    cache = fresh_cache()
    limiter = fresh_limiter()
    stats = fresh_stats()

    # Inject some data so metrics are non-trivial
    cache.set("prom-agent", "query", "response")
    cache.get("prom-agent", "query")   # hit
    cache.get("prom-agent", "miss-q")  # miss
    limiter.set_limit("prom-agent", 5)
    stats.record("prom-agent", 0.12, 0.0, "MISS", error=False)
    stats.record("prom-agent", 0.0,  0.0, "HIT",  error=False)

    text = stats.prometheus_text(cache, limiter)

    required_metrics = [
        "gateway_cache_hits_total",
        "gateway_cache_misses_total",
        "gateway_cache_hit_ratio",
    ]
    for metric in required_metrics:
        assert metric in text, f"Missing metric: {metric}"

    # Verify it's valid Prometheus text (lines either # or metric value)
    for line in text.strip().splitlines():
        assert line.startswith("#") or " " in line, f"Bad Prometheus line: {line!r}"

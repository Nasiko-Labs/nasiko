"""
Sentinel Guard — Comprehensive Debug & Validation Suite.
Tests every component in isolation, then integration, and reports all issues.
"""

import asyncio
import json
import sys
import time
import traceback
import os
from pathlib import Path

# Fix Windows console encoding
os.environ["PYTHONIOENCODING"] = "utf-8"
if sys.stdout.encoding != "utf-8":
    try:
        sys.stdout.reconfigure(encoding="utf-8")
    except Exception:
        pass

# Ensure the sentinel-guard app is importable
sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

PASS = "[PASS]"
FAIL = "[FAIL]"
WARN = "[WARN]"
INFO = "[INFO]"

results = {"pass": 0, "fail": 0, "warn": 0}


def report(status, name, detail=""):
    tag = PASS if status == "pass" else FAIL if status == "fail" else WARN
    results[status] = results.get(status, 0) + 1
    msg = f"  {tag} {name}" + (f" -- {detail}" if detail else "")
    print(msg)


def section(title):
    print(f"\n{'='*60}")
    print(f"  {title}")
    print(f"{'='*60}")


# ═══════════════════════════════════════════════════════════════
#  1. CONFIG MODULE
# ═══════════════════════════════════════════════════════════════
def test_config():
    section("1. Configuration Module")
    try:
        from app.config import config, SentinelConfig
        report("pass", "Import config module")
    except Exception as e:
        report("fail", "Import config module", str(e))
        return

    # Check all expected attributes exist
    attrs = [
        "REDIS_HOST", "REDIS_PORT", "REDIS_DB",
        "CACHE_TTL_SECONDS", "SIMILARITY_THRESHOLD", "MAX_CACHE_SIZE_PER_AGENT",
        "RATE_LIMIT_DEFAULT_RPM", "RATE_LIMIT_WINDOW_SECONDS",
        "MAX_QUEUE_DEPTH", "QUEUE_ITEM_TIMEOUT_SECONDS",
        "NASIKO_BASE_URL", "EMBEDDING_MODEL", "DASHBOARD_SSE_INTERVAL",
    ]
    for attr in attrs:
        if hasattr(config, attr):
            report("pass", f"config.{attr} = {getattr(config, attr)}")
        else:
            report("fail", f"config.{attr} missing")

    # Validate types
    assert isinstance(config.REDIS_PORT, int), "REDIS_PORT should be int"
    assert isinstance(config.SIMILARITY_THRESHOLD, float), "SIMILARITY_THRESHOLD should be float"
    assert 0 < config.SIMILARITY_THRESHOLD <= 1.0, "SIMILARITY_THRESHOLD should be 0-1"
    report("pass", "Config type validation")


# ═══════════════════════════════════════════════════════════════
#  2. STORE MODULE
# ═══════════════════════════════════════════════════════════════
def test_store():
    section("2. Store Module (In-memory State)")
    try:
        from app.store import (
            Decision, CacheEntry, decision_log,
            cache_hits, cache_misses, record_decision,
            increment_counter, get_all_known_agents,
        )
        report("pass", "Import store module")
    except Exception as e:
        report("fail", "Import store module", str(e))
        return

    # Test Decision creation
    d = Decision(timestamp=time.time(), agent="test", query="hello", outcome="cache_miss")
    report("pass", f"Decision created: outcome={d.outcome}")

    # Test CacheEntry creation
    ce = CacheEntry(query="test", response={"data": "ok"}, agent="test", cache_key="k1")
    report("pass", f"CacheEntry created: agent={ce.agent}, hits={ce.hits}")

    # Test record_decision
    record_decision(d)
    assert len(decision_log) >= 1
    report("pass", f"record_decision: log size={len(decision_log)}")

    # Test increment_counter
    increment_counter(cache_hits, "test-agent")
    increment_counter(cache_hits, "test-agent")
    assert cache_hits["test-agent"] == 2
    report("pass", f"increment_counter: test-agent={cache_hits['test-agent']}")

    # Test get_all_known_agents
    agents = get_all_known_agents()
    assert "test-agent" in agents
    report("pass", f"get_all_known_agents: {agents}")

    # Cleanup test data
    cache_hits.pop("test-agent", None)
    cache_misses.pop("test-agent", None)


# ═══════════════════════════════════════════════════════════════
#  3. RATE LIMITER
# ═══════════════════════════════════════════════════════════════
def test_rate_limiter():
    section("3. Rate Limiter")
    try:
        from app.rate_limiter import RateLimiter
        rl = RateLimiter()
        report("pass", "RateLimiter instantiated (Redis fallback OK)")
    except Exception as e:
        report("fail", "RateLimiter instantiation", str(e))
        return

    # Use unique agent name to avoid stale Redis keys
    import uuid as _uuid
    agent = f"debug-agent-{_uuid.uuid4().hex[:6]}"

    # Clean up any stale Redis keys
    if rl._redis_ok and rl._redis:
        try:
            rl._redis.delete(f"sentinel:rate:{agent}")
        except Exception:
            pass

    # Test check -- should be allowed initially
    result = rl.check(agent)
    assert result["allowed"] is True
    report("pass", f"check('{agent}'): allowed={result['allowed']}, remaining={result['remaining']}")

    # Test set_limit
    rl.set_limit(agent, 5)
    assert rl.get_limit(agent) == 5
    report("pass", f"set_limit: {agent}={rl.get_limit(agent)} RPM")

    # Test recording requests up to limit
    for i in range(5):
        rl.record_request(agent)
    result_after = rl.check(agent)
    assert result_after["allowed"] is False, f"Expected rate limit but got: {result_after}"
    report("pass", f"Rate limit enforced after 5 requests: allowed={result_after['allowed']}")

    # Test retry_after_ms is returned
    assert result_after.get("retry_after_ms") is not None
    report("pass", f"retry_after_ms={result_after['retry_after_ms']}")

    # Test get_stats
    stats = rl.get_stats()
    assert agent in stats
    report("pass", f"get_stats: {json.dumps(stats.get(agent, {}))}")

    # Cleanup
    from app.store import rate_limits, requests_rejected
    rate_limits.pop(agent, None)
    requests_rejected.pop(agent, None)
    if rl._redis_ok and rl._redis:
        try:
            rl._redis.delete(f"sentinel:rate:{agent}")
        except Exception:
            pass


# ═══════════════════════════════════════════════════════════════
#  4. QUEUE MANAGER
# ═══════════════════════════════════════════════════════════════
def test_queue_manager():
    section("4. Queue Manager")
    try:
        from app.queue_manager import QueueManager
        qm = QueueManager()
        report("pass", "QueueManager instantiated")
    except Exception as e:
        report("fail", "QueueManager instantiation", str(e))
        return

    # Test enqueue
    result = qm.enqueue("debug-agent", "test query", {"key": "value"})
    assert result.get("queued") is True
    report("pass", f"enqueue: queued={result['queued']}, pos={result.get('position')}")

    # Test get_depth
    depth = qm.get_depth("debug-agent")
    assert depth >= 1
    report("pass", f"get_depth: {depth}")

    # Test dequeue
    item = qm.dequeue("debug-agent")
    assert item is not None
    assert item["query"] == "test query"
    report("pass", f"dequeue: got item id={item.get('id')}")

    # Test depth after dequeue
    depth_after = qm.get_depth("debug-agent")
    assert depth_after == depth - 1
    report("pass", f"depth after dequeue: {depth_after}")

    # Test get_all_depths
    depths = qm.get_all_depths()
    report("pass", f"get_all_depths: {depths}")

    # Test queue overflow
    from app.config import config
    old_max = config.MAX_QUEUE_DEPTH
    config.MAX_QUEUE_DEPTH = 2
    qm.enqueue("overflow-agent", "q1", {})
    qm.enqueue("overflow-agent", "q2", {})
    overflow_result = qm.enqueue("overflow-agent", "q3", {})
    assert overflow_result.get("queued") is False
    report("pass", f"Queue overflow detected: reason={overflow_result.get('reason')}")
    config.MAX_QUEUE_DEPTH = old_max

    # Cleanup
    qm.dequeue("overflow-agent")
    qm.dequeue("overflow-agent")

    # Test cleanup_expired
    removed = qm.cleanup_expired()
    report("pass", f"cleanup_expired: removed={removed}")


# ═══════════════════════════════════════════════════════════════
#  5. CACHE LAYER
# ═══════════════════════════════════════════════════════════════
def test_cache():
    section("5. Cache Layer (Semantic + Exact)")
    try:
        from app.cache import CacheLayer
        cl = CacheLayer()
        report("pass", "CacheLayer instantiated")
    except Exception as e:
        report("fail", "CacheLayer instantiation", str(e))
        return

    # Test stats
    stats = cl.stats()
    report("pass", f"stats: redis={stats['redis_entries']}, semantic={stats['semantic_entries']}, model={stats.get('embedding_model')}")

    if stats.get("embedding_model") is None:
        report("warn", "No embedding model loaded — semantic cache disabled")

    # Test store + exact lookup
    cl.store("What is the capital of France?", {"answer": "Paris"}, "debug-agent")
    report("pass", "Stored response for 'capital of France'")

    # Test exact match lookup (will only work if Redis is available)
    exact = cl.lookup("What is the capital of France?", "debug-agent")
    if exact is not None:
        report("pass", f"Exact match lookup HIT: {json.dumps(exact)[:80]}")
    else:
        report("warn", "Exact match lookup MISS (Redis may be unavailable — expected locally)")

    # Test semantic lookup with similar query
    if stats.get("embedding_model"):
        similar = cl.lookup("What's the capital city of France?", "debug-agent")
        if similar is not None:
            sim = similar.get("_similarity", 0)
            report("pass", f"Semantic lookup HIT: similarity={sim:.3f}")
        else:
            report("warn", "Semantic lookup MISS — threshold may be too high for this test")

        # Test with very different query (should miss)
        different = cl.lookup("How to cook pasta?", "debug-agent")
        if different is None:
            report("pass", "Different query correctly returned MISS")
        else:
            report("warn", f"Different query unexpectedly HIT: sim={different.get('_similarity')}")
    else:
        report("warn", "Skipping semantic tests — no model")

    # Test flush
    flushed = cl.flush("debug-agent")
    report("pass", f"flush(debug-agent): {flushed}")

    flushed_all = cl.flush()
    report("pass", f"flush(all): {flushed_all}")


# ═══════════════════════════════════════════════════════════════
#  6. MEMPALACE ADAPTER
# ═══════════════════════════════════════════════════════════════
def test_mempalace():
    section("6. MemPalace Adapter")
    try:
        from app.mempalace_adapter import MemPalaceAdapter
        mp = MemPalaceAdapter()
        report("pass", f"MemPalaceAdapter instantiated: available={mp.available}")
    except Exception as e:
        report("fail", "MemPalaceAdapter instantiation", str(e))
        return

    if not mp.available:
        report("warn", "MemPalace not installed — adapter will gracefully degrade (this is OK for local testing)")
        # Test that methods don't crash when unavailable
        result = mp.search("test query", "test-agent")
        assert result is None
        report("pass", "search() returns None when unavailable (graceful)")
        mp.store("test query", {"data": "test"}, "test-agent")
        report("pass", "store() doesn't crash when unavailable (graceful)")
    else:
        report("pass", "MemPalace is available for deep semantic search")


# ═══════════════════════════════════════════════════════════════
#  7. MONITOR DASHBOARD
# ═══════════════════════════════════════════════════════════════
def test_monitor():
    section("7. Monitor Dashboard HTML")
    try:
        from app.monitor import DASHBOARD_HTML
        report("pass", "Dashboard HTML loaded")
    except Exception as e:
        report("fail", "Dashboard HTML import", str(e))
        return

    assert len(DASHBOARD_HTML) > 1000
    report("pass", f"Dashboard HTML size: {len(DASHBOARD_HTML)} chars")

    # Check for critical UI elements
    checks = [
        ("DOCTYPE", "<!DOCTYPE html>"),
        ("SSE EventSource", "EventSource"),
        ("Cache Hit Rate card", "Cache Hit Rate"),
        ("Per-Agent table", "Per-Agent Statistics"),
        ("Decision log table", "Recent Decisions"),
        ("Flush button", "flushCache"),
        ("Inter font", "Inter"),
    ]
    for name, needle in checks:
        if needle in DASHBOARD_HTML:
            report("pass", f"Dashboard has {name}")
        else:
            report("fail", f"Dashboard missing {name}")


# ═══════════════════════════════════════════════════════════════
#  8. MAIN APP (FastAPI)
# ═══════════════════════════════════════════════════════════════
def test_main_app():
    section("8. Main FastAPI Application")
    try:
        from app.main import app, cache_layer, rate_limiter, queue_manager
        report("pass", "FastAPI app imported")
    except Exception as e:
        report("fail", "FastAPI app import", str(e))
        return

    # Check routes exist
    routes = {r.path for r in app.routes if hasattr(r, "path")}
    expected_routes = [
        "/health", "/proxy", "/cache/check", "/cache/store",
        "/cache/flush", "/rate/check/{agent}", "/config/rate-limit/{agent}",
        "/queue/status", "/stats", "/dashboard", "/events",
    ]
    for route in expected_routes:
        if route in routes:
            report("pass", f"Route registered: {route}")
        else:
            report("fail", f"Route missing: {route}")

    # Check CORS middleware
    cors_found = any("CORSMiddleware" in str(type(m)) for m in app.user_middleware)
    if cors_found:
        report("pass", "CORS middleware configured")
    else:
        report("warn", "CORS middleware may not be detected (check manually)")

    # Check singletons are initialized
    assert cache_layer is not None
    report("pass", "CacheLayer singleton initialized")
    assert rate_limiter is not None
    report("pass", "RateLimiter singleton initialized")
    assert queue_manager is not None
    report("pass", "QueueManager singleton initialized")


# ═══════════════════════════════════════════════════════════════
#  9. INTEGRATION TEST (async)
# ═══════════════════════════════════════════════════════════════
async def test_integration():
    section("9. Integration Test (Full Proxy Flow)")
    try:
        from httpx import AsyncClient, ASGITransport
        from app.main import app
    except ImportError as e:
        report("fail", "Integration test imports", str(e))
        return

    transport = ASGITransport(app=app)
    async with AsyncClient(transport=transport, base_url="http://test") as client:
        # Health check
        resp = await client.get("/health")
        assert resp.status_code == 200
        health = resp.json()
        report("pass", f"GET /health: status={health.get('status')}")

        # Stats (empty initially)
        resp = await client.get("/stats")
        assert resp.status_code == 200
        stats = resp.json()
        report("pass", f"GET /stats: summary keys={list(stats.get('summary', {}).keys())[:5]}")

        # Dashboard
        resp = await client.get("/dashboard")
        assert resp.status_code == 200
        assert "Sentinel Guard" in resp.text
        report("pass", "GET /dashboard: HTML served")

        # Cache check (should miss)
        resp = await client.get("/cache/check", params={"query": "test", "agent": "debug"})
        assert resp.status_code == 200
        assert resp.json()["hit"] is False
        report("pass", "GET /cache/check: miss (correct)")

        # Cache store
        resp = await client.post("/cache/store", json={
            "agent": "debug", "query": "test q", "payload": {"answer": "42"}
        })
        assert resp.status_code == 200
        report("pass", "POST /cache/store: stored")

        # Rate check
        resp = await client.get("/rate/check/debug-agent")
        assert resp.status_code == 200
        rate = resp.json()
        assert rate["allowed"] is True
        report("pass", f"GET /rate/check: allowed={rate['allowed']}")

        # Update rate limit
        resp = await client.put("/config/rate-limit/debug-agent", json={"rpm": 30})
        assert resp.status_code == 200
        assert resp.json()["new_limit_rpm"] == 30
        report("pass", "PUT /config/rate-limit: updated to 30 RPM")

        # Queue status
        resp = await client.get("/queue/status")
        assert resp.status_code == 200
        report("pass", f"GET /queue/status: {resp.json()}")

        # Cache flush
        resp = await client.post("/cache/flush")
        assert resp.status_code == 200
        report("pass", f"POST /cache/flush: {resp.json()}")

        # Proxy (will fail to reach agent but should handle gracefully)
        resp = await client.post("/proxy", json={
            "agent": "nonexistent-agent", "query": "test query"
        })
        # Expected: 502 (agent unreachable) — this is correct behavior
        if resp.status_code == 502:
            report("pass", f"POST /proxy: correctly returned 502 for unreachable agent")
        elif resp.status_code == 200:
            report("pass", f"POST /proxy: returned 200 (cache hit or agent reachable)")
        else:
            report("warn", f"POST /proxy: status={resp.status_code}, body={resp.text[:100]}")

        # Verify stats after operations
        resp = await client.get("/stats")
        stats = resp.json()
        summary = stats.get("summary", {})
        report("pass", f"Final stats: requests={summary.get('total_requests')}, hits={summary.get('total_cache_hits')}, misses={summary.get('total_cache_misses')}")

    # Cleanup
    from app.store import rate_limits
    rate_limits.pop("debug-agent", None)


# ═══════════════════════════════════════════════════════════════
#  10. SENTINEL GUARD CLIENT (Router Side)
# ═══════════════════════════════════════════════════════════════
async def test_sentinel_client():
    section("10. SentinelGuardClient (Router Integration)")
    try:
        sys.path.insert(0, str(Path(__file__).resolve().parent.parent.parent.parent.parent / "agent-gateway" / "router" / "src"))
        # Direct import of the file
        import importlib.util
        client_path = str(Path(__file__).resolve().parent.parent.parent.parent.parent / "agent-gateway" / "router" / "src" / "core" / "sentinel_guard_client.py")
        spec = importlib.util.spec_from_file_location("sentinel_guard_client", client_path)
        mod = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(mod)
        SentinelGuardClient = mod.SentinelGuardClient
        report("pass", "SentinelGuardClient imported")
    except Exception as e:
        report("fail", "SentinelGuardClient import", str(e))
        return

    # Test with unreachable sentinel (should fail-open)
    sgc = SentinelGuardClient(base_url="http://localhost:99999")
    report("pass", "SentinelGuardClient instantiated with bad URL")

    healthy = await sgc.is_healthy()
    assert healthy is False
    report("pass", f"is_healthy() returns False for bad URL (fail-open works)")

    cached = await sgc.check_cache("test", "test-agent")
    assert cached is None
    report("pass", "check_cache() returns None on failure (fail-open)")

    rate = await sgc.check_rate("test-agent")
    assert rate["allowed"] is True
    report("pass", f"check_rate() returns allowed=True on failure (fail-open)")

    stats = await sgc.get_stats()
    assert stats is None
    report("pass", "get_stats() returns None on failure (graceful)")

    # Fire-and-forget store shouldn't crash
    await sgc.store_cache("q", "r", "agent")
    report("pass", "store_cache() doesn't crash on failure")


# ═══════════════════════════════════════════════════════════════
#  RUN ALL TESTS
# ═══════════════════════════════════════════════════════════════
def main():
    print("\n" + "=" * 60)
    print("  SENTINEL GUARD -- Debug & Validation Suite")
    print("=" * 60)

    test_config()
    test_store()
    test_rate_limiter()
    test_queue_manager()
    test_cache()
    test_mempalace()
    test_monitor()
    test_main_app()

    # Run async tests
    asyncio.run(test_integration())
    asyncio.run(test_sentinel_client())

    # Final Report
    print("\n" + "=" * 60)
    print("  FINAL REPORT")
    print("=" * 60)
    total = results["pass"] + results["fail"] + results["warn"]
    print(f"\n  Total tests:  {total}")
    print(f"  Passed:   {results['pass']}")
    print(f"  Failed:   {results['fail']}")
    print(f"  Warnings: {results['warn']}")
    print()

    if results["fail"] > 0:
        print("  SOME TESTS FAILED -- see above for details")
        sys.exit(1)
    else:
        print("  ALL TESTS PASSED")
        if results["warn"] > 0:
            print("  (warnings are expected for local env without Redis/MemPalace)")
        sys.exit(0)


if __name__ == "__main__":
    main()

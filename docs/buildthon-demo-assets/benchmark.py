"""
Resilient Request Layer — live in-process benchmark.
Exercises cache, rate limiter, adaptive loop, and stats.
Saves results to latest-benchmark.json in the same directory.

Run from agent-gateway/:
    uv run python ../docs/buildthon-demo-assets/benchmark.py
"""

import asyncio
import json
import os
import sys
import time
from pathlib import Path

# Ensure agent-gateway is on the path (run from project root or agent-gateway/)
_agent_gw = Path(__file__).resolve().parents[2] / "agent-gateway"
if str(_agent_gw) not in sys.path:
    sys.path.insert(0, str(_agent_gw))

from router.src.core.resilient_executor import (
    InMemoryCache,
    CacheConfig,
    TokenBucketRateLimiter,
    RuntimeStats,
)


# ── Benchmark helpers ─────────────────────────────────────────────────────────

def bench_cache_exact(n_unique: int = 20, n_repeat: int = 5) -> dict:
    """Store n_unique entries, then fetch each n_repeat times."""
    cache = InMemoryCache(CacheConfig(ttl=60, max_size=500))
    queries = [f"summarise document #{i} for me" for i in range(n_unique)]

    # Store phase
    t0 = time.perf_counter()
    for q in queries:
        cache.set("summarizer-agent", q, f"Summary of doc #{queries.index(q)}")
    store_ms = (time.perf_counter() - t0) * 1000

    # Read phase
    hit_count = 0
    t0 = time.perf_counter()
    for _ in range(n_repeat):
        for q in queries:
            _, status, _ = cache.get("summarizer-agent", q)
            if status == "HIT":
                hit_count += 1
    read_ms = (time.perf_counter() - t0) * 1000

    total_reads  = n_unique * n_repeat
    stats        = cache.stats()
    return {
        "scenario":         "exact_cache",
        "unique_entries":   n_unique,
        "repeat_reads":     n_repeat,
        "total_reads":      total_reads,
        "hits":             hit_count,
        "misses":           total_reads - hit_count,
        "hit_ratio":        round(hit_count / total_reads, 4),
        "store_batch_ms":   round(store_ms, 2),
        "read_batch_ms":    round(read_ms, 2),
        "avg_read_us":      round(read_ms / total_reads * 1000, 2),
        "cache_stats":      stats,
    }


def bench_cache_no_cache_header(n: int = 50) -> dict:
    """Verify that skip_read=True always bypasses the cache."""
    cache = InMemoryCache(CacheConfig(ttl=300))
    q = "what is the quarterly revenue?"
    cache.set("qa-agent", q, "Revenue was $4.2M")

    bypassed = 0
    t0 = time.perf_counter()
    for _ in range(n):
        _, status, _ = cache.get("qa-agent", q, skip_read=True)
        if status == "MISS":
            bypassed += 1
    elapsed_ms = (time.perf_counter() - t0) * 1000

    return {
        "scenario":        "no_cache_header",
        "requests":        n,
        "bypassed":        bypassed,
        "bypass_ratio":    round(bypassed / n, 4),
        "total_ms":        round(elapsed_ms, 2),
        "avg_us":          round(elapsed_ms / n * 1000, 2),
    }


def bench_file_not_cached(n: int = 20) -> dict:
    """Confirm file-attached requests are never cached."""
    cache = InMemoryCache(CacheConfig(ttl=300))
    not_cached = 0
    for i in range(n):
        cache.set("qa-agent", f"analyse this pdf #{i}", f"result {i}", has_files=True)
        _, status, _ = cache.get("qa-agent", f"analyse this pdf #{i}", has_files=True)
        if status == "MISS":
            not_cached += 1
    return {
        "scenario":        "file_not_cached",
        "requests":        n,
        "correctly_missed": not_cached,
        "stores":          cache.stores,
    }


async def bench_rate_limiter(rpm: int = 10, burst: int = 15) -> dict:
    """Send burst requests; track allowed vs rejected."""
    limiter = TokenBucketRateLimiter()
    limiter.set_limit("bench-agent", rpm)

    allowed = rejected = queue_waits = 0
    latencies_ms = []

    for _ in range(burst):
        t0 = time.perf_counter()
        ok, wait_s = await limiter.acquire("bench-agent")
        elapsed_ms = (time.perf_counter() - t0) * 1000
        latencies_ms.append(round(elapsed_ms, 2))
        if ok:
            allowed += 1
            if wait_s > 0:
                queue_waits += 1
        else:
            rejected += 1

    rl_stats = limiter.all_stats().get("bench-agent", {})
    return {
        "scenario":        "rate_limiter",
        "configured_rpm":  rpm,
        "burst_requests":  burst,
        "allowed":         allowed,
        "rejected":        rejected,
        "queue_waits":     queue_waits,
        "latencies_ms":    latencies_ms,
        "avg_latency_ms":  round(sum(latencies_ms) / len(latencies_ms), 2),
        "limiter_stats":   rl_stats,
    }


def bench_adaptive_limits() -> dict:
    """Verify adaptive adjustment on simulated latency samples."""
    limiter = TokenBucketRateLimiter()
    limiter.set_limit("slow", 20)
    limiter.set_limit("fast", 10)

    # Feed latency samples
    for _ in range(10):
        limiter.record_latency("slow", 3.0)   # 3 000 ms avg → reduce
        limiter.record_latency("fast", 0.1)   # 100 ms avg → increase

    before_slow = limiter.get_limit("slow")
    before_fast = limiter.get_limit("fast")
    limiter._adapt_all()
    after_slow = limiter.get_limit("slow")
    after_fast = limiter.get_limit("fast")

    return {
        "scenario":       "adaptive_limits",
        "slow_agent": {
            "before_rpm": before_slow,
            "after_rpm":  after_slow,
            "direction":  "reduced" if after_slow < before_slow else "unchanged",
        },
        "fast_agent": {
            "before_rpm": before_fast,
            "after_rpm":  after_fast,
            "direction":  "increased" if after_fast > before_fast else "unchanged",
        },
    }


async def bench_stats_and_prometheus() -> dict:
    """Snapshot RuntimeStats and validate Prometheus output."""
    cache   = InMemoryCache(CacheConfig(ttl=300))
    limiter = TokenBucketRateLimiter()
    stats   = RuntimeStats()

    agents = ["summarizer-agent", "qa-agent", "sentiment-agent"]
    for i, agent in enumerate(agents):
        q = f"test query {i}"
        cache.set(agent, q, f"cached response {i}")
        cache.get(agent, q)         # hit
        cache.get(agent, "miss-q")  # miss
        limiter.set_limit(agent, 8 + i)
        await limiter.acquire(agent)   # populate _call_stats so queue_depth appears
        latency = 0.05 * (i + 1)
        stats.record(agent, latency, 0.0, "MISS",  error=False)
        stats.record(agent, 0.0,     0.0, "HIT",   error=False)

    snap      = stats.snapshot(cache, limiter)
    prom_text = stats.prometheus_text(cache, limiter)
    required  = [
        "gateway_cache_hits_total",
        "gateway_cache_misses_total",
        "gateway_cache_hit_ratio",
        "gateway_queue_depth",
        "gateway_adaptive_limit_current",
        "gateway_agent_latency_seconds",
    ]
    metrics_present = {m: m in prom_text for m in required}

    return {
        "scenario":         "stats_and_prometheus",
        "snapshot":         snap,
        "prometheus_lines": len(prom_text.strip().splitlines()),
        "metrics_present":  metrics_present,
        "all_metrics_ok":   all(metrics_present.values()),
    }


# ── Main ──────────────────────────────────────────────────────────────────────

async def main() -> None:
    print("=== Nasiko Resilient Layer Benchmark ===\n")

    results = {
        "timestamp":      time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        "python_version": sys.version,
        "scenarios":      [],
    }

    # 1. Exact cache
    print("1. Exact cache hit/miss...")
    r = bench_cache_exact(n_unique=30, n_repeat=10)
    results["scenarios"].append(r)
    print(f"   hit_ratio={r['hit_ratio']:.1%}  avg_read={r['avg_read_us']} µs\n")

    # 2. Cache-Control: no-cache bypass
    print("2. Cache-Control: no-cache bypass...")
    r = bench_cache_no_cache_header(n=100)
    results["scenarios"].append(r)
    print(f"   bypass_ratio={r['bypass_ratio']:.1%}  avg={r['avg_us']} µs\n")

    # 3. File requests not cached
    print("3. File requests never cached...")
    r = bench_file_not_cached(n=50)
    results["scenarios"].append(r)
    print(f"   correctly_missed={r['correctly_missed']}/{r['requests']}\n")

    # 4. Rate limiter burst
    print("4. Rate limiter burst test (10 RPM, 15 burst)...")
    r = await bench_rate_limiter(rpm=10, burst=15)
    results["scenarios"].append(r)
    print(f"   allowed={r['allowed']}  rejected={r['rejected']}  queued={r['queue_waits']}\n")

    # 5. Adaptive limits
    print("5. Adaptive rate-limit adjustment...")
    r = bench_adaptive_limits()
    results["scenarios"].append(r)
    print(f"   slow: {r['slow_agent']['before_rpm']} -> {r['slow_agent']['after_rpm']} RPM ({r['slow_agent']['direction']})")
    print(f"   fast: {r['fast_agent']['before_rpm']} -> {r['fast_agent']['after_rpm']} RPM ({r['fast_agent']['direction']})\n")

    # 6. Stats + Prometheus
    print("6. Runtime stats & Prometheus metrics...")
    r = await bench_stats_and_prometheus()
    results["scenarios"].append(r)
    print(f"   prometheus_lines={r['prometheus_lines']}  all_metrics_ok={r['all_metrics_ok']}\n")

    # Save
    out_path = Path(__file__).parent / "latest-benchmark.json"
    out_path.write_text(json.dumps(results, indent=2))
    print(f"Results saved to: {out_path}")
    print("\n=== Benchmark complete ===")


if __name__ == "__main__":
    asyncio.run(main())

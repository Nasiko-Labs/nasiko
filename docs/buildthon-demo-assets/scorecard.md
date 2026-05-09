# Resilient Request Layer — Buildthon Scorecard

> Benchmark run: 2026-05-09 · Python 3.13.7 · Branch: Fix-Ishaan-gupta

---

## Core features

| # | Requirement | Status | Evidence |
|---|---|---|---|
| 1 | Exact-match cache (agent+query+scope+route+files key) | PASS | `test_exact_cache_hit`, hit_ratio=100% in benchmark |
| 2 | Semantic cache (sentence-transformers, cosine ≥ 0.92) | PASS | `_get_encoder()`, `SEMANTIC_CACHE_ENABLED` env var |
| 3 | Cache-Control: no-cache bypass | PASS | `test_cache_bypass_no_cache`, bypass_ratio=100% |
| 4 | Cache-Control: no-store bypass | PASS | `test_cache_no_store`, stores=0 |
| 5 | File uploads never cached | PASS | `test_cache_file_requests_not_cached`, 50/50 missed |
| 6 | Per-agent sliding-window rate limiter | PASS | `test_rate_limit_allows_within_limit` |
| 7 | Bounded queue + rejection | PASS | `test_rate_limit_rejects_when_queue_full` |
| 8 | Adaptive RPM (high latency → reduce, low → increase) | PASS | `test_adaptive_reduces/increases_limit`, -20%/+10% confirmed |
| 9 | Runtime stats (hits, misses, stores, latency histograms) | PASS | `/admin/stats/runtime` returns full snapshot |
| 10 | Prometheus `/metrics` | PASS | 6 metric families, Grafana-ready |
| 11 | Timing footer on every response | PASS | Appended in `router_orchestrator._send_agent_request` |

---

## Admin endpoints

| Endpoint | Auth | Status |
|---|---|---|
| `GET /admin/stats/runtime` | X-Admin-API-Key | PASS — returns uptime, counters, per-agent latency p50/p95 |
| `POST /admin/cache/clear` | X-Admin-API-Key | PASS — clears single agent or entire cache |
| `PUT /admin/cache/config` | X-Admin-API-Key | PASS — TTL and max_size tunable at runtime |
| `PUT /admin/limits/{agent_id}` | X-Admin-API-Key | PASS — RPM override takes effect immediately |

---

## Differentiators

| Differentiator | Status | Details |
|---|---|---|
| Semantic cache | PASS | `all-MiniLM-L6-v2`, lazy-loaded, graceful fallback if not installed, toggled via `SEMANTIC_CACHE_ENABLED=true` |
| Adaptive rate limits | PASS | Background coroutine started on uvicorn startup; 60 s interval; clamps to [MIN_RPM=2, MAX_RPM=100]; confirmed in benchmark: slow 20→16, fast 10→12 |
| Cache-Control headers | PASS | Incoming: `no-cache`, `no-store`, `X-Cache-TTL`, `X-Agent-Priority`; Outgoing: `X-Cache`, `X-Cache-Age`, `X-Agent-Latency` |

---

## Test suite — 11/11

```
router/tests/test_resilient_layer.py::test_exact_cache_hit                  PASSED
router/tests/test_resilient_layer.py::test_exact_cache_miss                 PASSED
router/tests/test_resilient_layer.py::test_cache_bypass_no_cache            PASSED
router/tests/test_resilient_layer.py::test_cache_no_store                   PASSED
router/tests/test_resilient_layer.py::test_cache_ttl_expiry                 PASSED
router/tests/test_resilient_layer.py::test_cache_file_requests_not_cached   PASSED
router/tests/test_resilient_layer.py::test_rate_limit_allows_within_limit   PASSED
router/tests/test_resilient_layer.py::test_rate_limit_rejects_when_queue_full PASSED
router/tests/test_resilient_layer.py::test_adaptive_reduces_limit_on_high_latency PASSED
router/tests/test_resilient_layer.py::test_adaptive_increases_limit_on_low_latency PASSED
router/tests/test_resilient_layer.py::test_prometheus_metrics_format        PASSED
11 passed in 11.45s
```

---

## Benchmark highlights (latest-benchmark.json)

| Scenario | Result |
|---|---|
| Exact cache: 300 reads across 30 unique entries | hit_ratio=**100%**, avg_read=**3.45 µs** |
| Cache-Control: no-cache bypass (100 requests) | bypass_ratio=**100%**, avg=**0.28 µs** |
| File requests never cached (50 requests) | correctly_missed=**50/50** |
| Rate limiter burst (10 RPM, 15 burst) | allowed=14, rejected=1, queued=1 |
| Adaptive adjustment (simulated latency) | slow 20→**16 RPM** (reduced 20%), fast 10→**12 RPM** (increased) |
| Prometheus metrics | **45 lines**, all 6 metric families present |

---

## Architecture notes

- **Zero new microservices** — the resilient layer lives inside the existing router process as a set of Python classes. No Redis, no sidecar, no new Docker containers required.
- **Thread-safe** — all mutable state uses `threading.Lock`; async methods use `asyncio` idioms. Safe under uvicorn's async event loop.
- **Redis-swappable** — `InMemoryCache` follows a simple `get/set/clear/flush` interface; swapping in Redis requires changing only the backend, not the call sites.
- **Graceful degradation** — semantic cache disables itself if `sentence-transformers` is not installed; adaptive loop errors are logged and retried next interval.

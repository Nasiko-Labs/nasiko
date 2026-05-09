# Nasiko Buildthon Demo — Resilient Request Layer

> Judge runbook: everything you need to reproduce, test, and score this submission.

---

## What was built

A **resilient request layer embedded directly inside the Nasiko router service** (`agent-gateway/router/src/core/resilient_executor.py`). Every agent call is now wrapped with:

| Feature | Implementation |
|---|---|
| Exact-match response cache | SHA-256 key: `agent_id + query + user_scope + route + has_files` |
| Semantic cache (opt-in) | `sentence-transformers/all-MiniLM-L6-v2`, cosine ≥ 0.92 |
| Per-agent adaptive rate limiting | Sliding-window token bucket; adjusts ±10-20% every 60 s based on observed latency |
| Bounded queue | Up to 50 pending requests per agent; hard 429 after 30 s timeout |
| Runtime stats | Hit/miss/store counters, per-agent latency histograms (p50, p95) |
| Cache-Control headers | `no-cache`, `no-store`, `X-Cache-TTL`, `X-Agent-Priority: high` |
| Response headers | `X-Cache: HIT/MISS/SEMANTIC-HIT`, `X-Cache-Age`, `X-Agent-Latency` |
| Prometheus `/metrics` | Standard text format, Grafana-ready |
| Timing footer | Appended to every chat response — visible inline in the Nasiko UI |
| Translator stabilization | 15 s OpenAI timeout + fast path for `translate X to Y` patterns |

All new code is on branch **`Fix-Ishaan-gupta`**.

---

## Quick start

```bash
# 1. Clone and switch to the branch
git clone https://github.com/Nasiko-Labs/nasiko
cd nasiko
git checkout Fix-Ishaan-gupta

# 2. Start the full stack (Kong, auth, agents, router)
docker compose -f agent-gateway/docker-compose.yml up -d

# 3. Verify the resilient layer is live
curl http://localhost:8081/router/health
# {"status":"ok"}

curl http://localhost:8081/metrics | head -10
# gateway_cache_hits_total 0 ...
```

---

## Endpoints

### Public

| Endpoint | Method | Description |
|---|---|---|
| `POST /router` | POST | Main routing endpoint — accepts `Cache-Control`, `X-Cache-TTL`, `X-Agent-Priority` headers |
| `GET /metrics` | GET | Prometheus-format metrics |
| `GET /router/health` | GET | Liveness probe |

### Admin (requires `X-Admin-API-Key: local-admin-key`)

| Endpoint | Method | Description |
|---|---|---|
| `GET /admin/stats/runtime` | GET | Full snapshot: cache, rate limits, latency histograms |
| `POST /admin/cache/clear?agent_id=X` | POST | Flush one agent's cache (or all) |
| `PUT /admin/cache/config?ttl=120` | PUT | Tune TTL / max_size at runtime |
| `PUT /admin/limits/{agent_id}?rpm=20` | PUT | Override per-agent RPM |

---

## Demo walkthrough (step-by-step)

### 1 — Send a query (cache MISS, first call)

```bash
TOKEN=$(curl -s -X POST http://localhost:8082/auth/login \
  -H "Content-Type: application/json" \
  -d '{"username":"admin","password":"admin"}' | python -c "import json,sys; print(json.load(sys.stdin)['token'])")

curl -s -X POST http://localhost:8081/router \
  -H "Authorization: Bearer $TOKEN" \
  -F "session_id=demo-session-1" \
  -F "query=Summarise the key benefits of microservices"
```

Look for the timing footer at the bottom of the streamed response:
```
---
*Request layer: agent call + cache store in 1234 ms (hit ratio 0%).*
```

### 2 — Send the exact same query (cache HIT)

```bash
curl -s -X POST http://localhost:8081/router \
  -H "Authorization: Bearer $TOKEN" \
  -F "session_id=demo-session-2" \
  -F "query=Summarise the key benefits of microservices"
```

Footer now shows:
```
---
*Request layer: cache hit in 1 ms (hit ratio 50%).*
```

### 3 — Force bypass with Cache-Control: no-cache

```bash
curl -s -X POST http://localhost:8081/router \
  -H "Authorization: Bearer $TOKEN" \
  -H "Cache-Control: no-cache" \
  -F "session_id=demo-session-3" \
  -F "query=Summarise the key benefits of microservices"
```

Footer shows `MISS` even though the answer is cached.

### 4 — Admin: view runtime stats

```bash
curl -s -H "X-Admin-API-Key: local-admin-key" \
  http://localhost:8081/admin/stats/runtime | python -m json.tool
```

### 5 — Admin: clear cache for one agent

```bash
curl -s -X POST -H "X-Admin-API-Key: local-admin-key" \
  "http://localhost:8081/admin/cache/clear?agent_id=summarizer-agent"
```

### 6 — Adjust rate limit live

```bash
curl -s -X PUT -H "X-Admin-API-Key: local-admin-key" \
  "http://localhost:8081/admin/limits/summarizer-agent?rpm=20"
```

### 7 — Run the full test suite

```bash
cd agent-gateway
uv run python -m pytest router/tests/test_resilient_layer.py -v
# 11 passed
```

### 8 — Run the benchmark

```bash
cd agent-gateway
uv run python ../docs/buildthon-demo-assets/benchmark.py
# Results saved to docs/buildthon-demo-assets/latest-benchmark.json
```

---

## Files changed

| File | Change |
|---|---|
| `agent-gateway/router/src/core/resilient_executor.py` | **New** — full resilient layer implementation |
| `agent-gateway/router/src/core/__init__.py` | Export new classes |
| `agent-gateway/router/src/main.py` | Admin endpoints, startup hook, header parsing |
| `agent-gateway/router/src/services/router_orchestrator.py` | Wire executor into agent call path |
| `agent-gateway/router/tests/test_resilient_layer.py` | **New** — 11 pytest tests |
| `agents/a2a-translator/src/openai_agent_executor.py` | 15 s timeout + fast path + timing footer |
| `docs/buildthon-demo-assets/benchmark.py` | **New** — benchmark script |
| `docs/buildthon-demo-assets/latest-benchmark.json` | **New** — benchmark results |

---

## Environment variables (all optional)

```bash
CACHE_TTL_SECONDS=300          # Response cache TTL
CACHE_MAX_SIZE=1000            # Max cached entries
AGENT_DEFAULT_RPM=10           # Default requests/min per agent
QUEUE_MAX_DEPTH=50             # Max queued requests per agent
QUEUE_TIMEOUT_SECONDS=30       # Queue wait timeout
ADMIN_API_KEY=local-admin-key  # Admin endpoint protection
SEMANTIC_CACHE_ENABLED=false   # Enable sentence-transformers cache
SEMANTIC_CACHE_THRESHOLD=0.92  # Cosine similarity threshold
MIN_RPM=2                      # Adaptive limiter floor
MAX_RPM=100                    # Adaptive limiter ceiling
ADAPTIVE_INTERVAL_SECONDS=60   # Adaptive adjustment interval
```

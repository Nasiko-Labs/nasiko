# Resilient Agent Request Layer

> **Nasiko Buildthon** — A unified request management layer combining intelligent caching + adaptive rate limiting for AI agent fleets.

---

## Architecture

```
Client → Gateway (port 8000)
           ├── Cache Check (Redis)      → HIT → instant response
           ├── Rate Limit (Token Bucket) → OK → forward to agent
           │                             → OVER → queue
           │                             → QUEUE FULL → 429
           └── Agent Fleet
                 ├── agent-a  (port 8001) — fast, 50–150ms
                 ├── agent-b  (port 8002) — medium, 300–700ms
                 └── agent-slow (port 8003) — heavy, 1.5–4s

Dashboard → port 3000 (real-time SSE monitoring)
```

---

## Quick Start

### 1. Clone & Run

```bash
cd resilient-agent-layer
docker compose up --build
```

Services:
| Service | URL |
|---|---|
| Gateway | http://localhost:8000 |
| Dashboard | http://localhost:3000 |
| Agent A | http://localhost:8001 |
| Agent B | http://localhost:8002 |
| Slow Agent | http://localhost:8003 |
| Redis | localhost:6379 |

### 2. Send a Request

```bash
curl -X POST http://localhost:8000/invoke \
  -H "Content-Type: application/json" \
  -d '{"agent_id": "agent-a", "payload": {"query": "hello world"}}'
```

Response:
```json
{
  "agent_id": "agent-a",
  "source": "agent",       // 'cache' | 'agent' | 'queue'
  "latency_ms": 87.3,
  "data": { ... }
}
```

Send the **same request again** → `"source": "cache"` with ~1ms latency.

---

## API Reference

### `POST /invoke`
```json
{
  "agent_id": "agent-a",
  "payload": { "query": "..." },
  "bypass_cache": false,
  "priority": 0
}
```

### `GET /health`

### Admin Endpoints
| Method | Path | Description |
|---|---|---|
| GET | `/admin/stats` | Global stats |
| GET | `/admin/stats/:id` | Per-agent stats |
| DELETE | `/admin/cache` | Flush all cache |
| DELETE | `/admin/cache/:id` | Flush agent cache |
| GET | `/admin/rate-limits` | View rate limits |
| PUT | `/admin/rate-limits/:id` | Update rate limits |
| GET | `/admin/queues` | Queue depths |
| GET | `/admin/stream` | Live SSE stream |

---

## Configuration

Edit `agents.json` to register agents and set per-agent limits:

```json
{
  "agent-a": {
    "base_url": "http://agent-a:8001",
    "cache_ttl": 60,
    "rate_limit_rps": 10,
    "burst": 20,
    "queue_max": 50,
    "queue_timeout_ms": 5000
  }
}
```

---

## Running Tests

```bash
# Unit tests
pip install -r requirements.txt
pip install pytest pytest-asyncio
pytest tests/test_cache.py tests/test_rate_limiter.py -v

# Load test
pip install locust
locust -f tests/load_test.py --headless -u 50 -r 5 --run-time 60s --host http://localhost:8000
```

---

## Demo Scenarios

### 1. Cache Demo
```bash
# Send same request 10 times — watch latency drop from ~100ms to ~1ms
for i in {1..10}; do
  curl -s -X POST http://localhost:8000/invoke \
    -H "Content-Type: application/json" \
    -d '{"agent_id":"agent-a","payload":{"query":"What is AI?"}}' \
    | python3 -c "import sys,json; d=json.load(sys.stdin); print(f'source={d[\"source\"]} latency={d[\"latency_ms\"]}ms')"
done
```

### 2. Rate Limit + Queue Demo
```bash
# Blast 30 concurrent requests to slow agent (rate limit = 2 rps)
for i in {1..30}; do
  curl -s -X POST http://localhost:8000/invoke \
    -H "Content-Type: application/json" \
    -d '{"agent_id":"agent-slow","payload":{"query":"analyze-'$i'"}}' &
done; wait
```

### 3. Admin: Update Rate Limit
```bash
curl -X PUT http://localhost:8000/admin/rate-limits/agent-a \
  -H "Content-Type: application/json" \
  -d '{"rps": 50, "burst": 100}'
```

### 4. Flush Cache
```bash
curl -X DELETE http://localhost:8000/admin/cache
```

---

## Key Metrics

| Metric | Target | How to Verify |
|---|---|---|
| Cache hit rate (repeated) | > 85% | Dashboard / `/admin/stats` |
| Latency reduction (cached) | > 80% | Compare first vs subsequent responses |
| Queue absorption under load | > 95% | Load test 429 rate |
| Admin API response | < 50ms | `/admin/stats` timing |

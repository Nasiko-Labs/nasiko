# RARL — Resilient Agent Request Layer
## Complete Cursor-Friendly Implementation Plan (3-Hour Hackathon Build)

> **How to use this document with Cursor:**
> 1. Save this file in your project root as `BUILD_PLAN.md`.
> 2. In Cursor, open the Composer (`Cmd/Ctrl + I`).
> 3. For each Phase below, **copy that phase's "Cursor Prompt" block verbatim** into Composer along with the file paths it touches. Cursor will generate the code.
> 4. **Do not run multiple phases in one prompt.** One phase = one prompt = one review = one commit.
> 5. After each phase, run the "Verify" step before moving on. **Never skip Verify.**

---

## Project Identity

| Item | Value |
|---|---|
| Project name | `rarl` (Resilient Agent Request Layer) |
| Service port | `8010` |
| Sits between | Kong Gateway (`:9100`) and Nasiko agent containers |
| Backing store | Redis (already running in Nasiko stack) |
| Language | Python 3.12 |
| Framework | FastAPI + asyncio |

---

## Final Architecture (one-page mental model)

```
                   ┌──────────────────────────────────────────────┐
Client ─► Kong ─►  │                  RARL :8010                  │ ─► Agent Container
 :9100             │                                              │      (translator, etc.)
                   │  1. Hash request → cache key                 │
                   │  2. Cache lookup (Redis)         ─► HIT      │
                   │  3. Single-flight check          ─► COALESCE │
                   │  4. Per-agent token bucket + queue           │
                   │  5. Forward to agent (httpx)                 │
                   │  6. Store response in cache                  │
                   │  7. Emit metrics → /admin/stream (SSE)       │
                   └──────────────────────────────────────────────┘
                                     │
                                     ▼
                              /dashboard (HTML + Chart.js)
                              /admin/* (config, stats, explain)
```

---

# PHASE 0 — Pre-Flight (10 minutes, BEFORE the 3-hour clock starts)

These steps must be done before you start the timer. They are environment setup, not build work.

## 0.1 Verify Nasiko is running

```bash
cd nasiko
docker compose -f docker-compose.local.yml --env-file .nasiko-local.env ps
curl http://localhost:8000/api/v1/healthcheck
curl http://localhost:9100/health
```

All three should succeed. If not, fix Nasiko first — you can't demo a layer if the platform underneath it is broken.

## 0.2 Identify the Redis container

```bash
docker compose -f docker-compose.local.yml ps | grep -i redis
```

Note the container name (likely `nasiko-redis-1` or similar) and the network name:

```bash
docker network ls | grep nasiko
```

Note the network name (likely `nasiko_default`). **Write both down — you'll need them in Phase 1.**

## 0.3 Create the project directory

```bash
cd ..                          # back out of nasiko/
mkdir rarl && cd rarl
git init
```

## 0.4 Open the folder in Cursor

```bash
cursor .
```

## 0.5 Create the empty file scaffolding (so Cursor sees the structure)

Run this in the Cursor terminal:

```bash
mkdir -p app/templates tests
touch app/__init__.py app/main.py app/config.py app/proxy.py \
      app/cache.py app/coalescer.py app/ratelimit.py app/queue_lane.py \
      app/metrics.py app/admin.py app/dashboard.py \
      app/templates/dashboard.html \
      tests/demo_load.py \
      Dockerfile docker-compose.override.yml requirements.txt \
      .env.example .gitignore README.md BUILD_PLAN.md
```

## 0.6 Save this build plan into the repo

Copy the contents of this file into `BUILD_PLAN.md`. **This is critical** — Cursor's Composer reads open files for context, and having the spec in the repo means Cursor stays aligned across prompts.

## 0.7 Create `.gitignore`

```
__pycache__/
*.pyc
.env
.venv/
.pytest_cache/
*.log
```

## 0.8 Set Cursor rules (paste into `.cursorrules` in the project root)

```
You are helping build RARL, a FastAPI sidecar that sits between Kong Gateway
and AI agent containers. Read BUILD_PLAN.md for full context.

Hard rules:
- Python 3.12, FastAPI, asyncio, redis-py 5.x async client, httpx async.
- One file per concern. Do NOT merge files unless I ask.
- Every async I/O call must be awaited; no blocking calls in request path.
- Add type hints on every function.
- Keep functions under 30 lines where possible; extract helpers.
- Do NOT add new dependencies without me asking.
- When unsure, prefer the simplest working version — this is a 3-hour hackathon build.
- Comment any non-obvious line; otherwise no comments.
- Never write tests unless explicitly asked (we do manual verification).
```

✅ **Pre-flight done. Start the 3-hour timer now.**

---

# PHASE 1 — Skeleton & Wiring (00:00 → 00:30)

**Goal:** A FastAPI service on port 8010 that proxies any request to a configured agent and returns its response. No cache, no rate limit yet. End state: `curl` round-trips through RARL to a real Nasiko agent.

## 1.1 Write `requirements.txt`

**Cursor Prompt:**
```
Open requirements.txt and add exactly these pinned dependencies, one per line, no extras:

fastapi==0.115.0
uvicorn[standard]==0.30.6
httpx==0.27.2
redis==5.0.8
sse-starlette==2.1.3
pydantic==2.9.2
pydantic-settings==2.5.2
python-multipart==0.0.9
jinja2==3.1.4
```

## 1.2 Write `app/config.py`

**Cursor Prompt:**
```
Create app/config.py with a pydantic-settings BaseSettings class called Settings.

Fields (all with sensible defaults):
- redis_url: str = "redis://nasiko-redis-1:6379/2"  (use DB 2 to avoid collision)
- agent_base_urls: dict[str, str] = field default empty dict; loaded from env JSON string AGENT_BASE_URLS
- default_ttl_seconds: int = 300
- default_rps: float = 10.0
- default_burst: int = 20
- default_max_inflight: int = 4
- default_max_queue: int = 100
- target_p95_latency: float = 1.0
- log_level: str = "INFO"

Provide a `get_settings()` function with @lru_cache.
Read .env file. Reject unknown env vars (extra="ignore").
```

## 1.3 Write `app/main.py`

**Cursor Prompt:**
```
Create app/main.py with:
1. FastAPI app instance with title="RARL" and version="0.1.0".
2. A lifespan context manager that:
   - Connects to Redis (redis.asyncio.from_url) and stores client on app.state.redis
   - Initializes an empty dict app.state.lanes (will hold AgentLane objects later)
   - Initializes app.state.metrics = {} (placeholder)
   - On shutdown: close the Redis client.
3. A GET /health endpoint that returns {"status": "ok", "service": "rarl"}.
4. A catch-all proxy route: @app.api_route("/agents/{agent_id}/{path:path}", methods=["GET","POST","PUT","DELETE","PATCH"]) that calls a handler in app.proxy.handle_request(agent_id, path, request).
5. Mount nothing else yet.

Use the get_settings() function from app.config.
Run with uvicorn when invoked as __main__ on host 0.0.0.0 port 8010.
```

## 1.4 Write `app/proxy.py` (skeleton — no cache/RL yet)

**Cursor Prompt:**
```
Create app/proxy.py with one function:

async def handle_request(agent_id: str, path: str, request: Request) -> Response:
    """
    Phase 1 skeleton: forward everything to the upstream agent and return as-is.
    Cache and rate-limiting will be added in later phases.
    """

Implementation:
1. Look up upstream URL: settings.agent_base_urls[agent_id]. If missing, return 404.
2. Read request body (await request.body()), method, headers (drop "host" and "content-length").
3. Use a module-level httpx.AsyncClient with timeout=30s (create lazily).
4. Forward the request: client.request(method, f"{base}/{path}", headers=..., content=body, params=request.query_params).
5. Return a Response with the upstream's status_code, content, and headers (drop "content-encoding" and "transfer-encoding").
6. Add response header "X-RARL-Phase: 1-proxy".

Use FastAPI's Request and starlette.responses.Response. Add proper type hints.
```

## 1.5 Write `Dockerfile`

**Cursor Prompt:**
```
Create a minimal Dockerfile:
- Base: python:3.12-slim
- Workdir /app
- Copy requirements.txt, pip install --no-cache-dir -r requirements.txt
- Copy app/ to /app/app/
- Expose 8010
- CMD: uvicorn app.main:app --host 0.0.0.0 --port 8010 --log-level info
```

## 1.6 Write `docker-compose.override.yml`

**Cursor Prompt:**
```
Create docker-compose.override.yml that adds an "rarl" service:
- build: .
- container_name: nasiko-rarl
- ports: "8010:8010"
- environment:
    REDIS_URL: redis://redis:6379/2     # adjust hostname based on Nasiko's actual redis service name
    AGENT_BASE_URLS: '{"translator":"http://translator-agent:8000"}'  # placeholder; user will edit
    LOG_LEVEL: INFO
- networks: [nasiko_default]            # external: true
- depends_on: [redis]
- restart: unless-stopped

At the bottom, declare networks:
  nasiko_default:
    external: true

This override is meant to be placed in the nasiko/ directory and run alongside docker-compose.local.yml.
```

⚠️ **Manual step (you, not Cursor):** Edit the `AGENT_BASE_URLS` and the redis hostname to match what you wrote down in step 0.2. Look in Nasiko's main `docker-compose.local.yml` for the actual agent service name (likely `translator-agent` or similar).

## 1.7 Build and run

```bash
cd ../nasiko    # so we're alongside the original compose file
cp ../rarl/docker-compose.override.yml .
docker compose -f docker-compose.local.yml -f docker-compose.override.yml --env-file .nasiko-local.env up -d --build rarl
docker logs -f nasiko-rarl
```

## 1.8 Verify Phase 1

```bash
# Should return {"status":"ok","service":"rarl"}
curl http://localhost:8010/health

# Should proxy through to the real translator agent (deploy it first per Nasiko quickstart)
curl -i http://localhost:8010/agents/translator/health
# Expect: 200, with response header "X-RARL-Phase: 1-proxy"
```

✅ **Phase 1 complete when both curls succeed.** Commit: `git add -A && git commit -m "phase 1: skeleton proxy"`

---

# PHASE 2 — Cache + Single-Flight Coalescing (00:30 → 01:00)

**Goal:** Repeated identical requests served from Redis. Simultaneous identical in-flight requests collapse to one upstream call. End state: 100 concurrent identical requests = 1 agent call, 99 cache/coalesce hits.

## 2.1 Write `app/cache.py`

**Cursor Prompt:**
```
Create app/cache.py with:

import hashlib
import json
from typing import Any
import redis.asyncio as aioredis

CACHEABLE_METHODS = {"GET", "POST"}
SKIP_HEADERS = {"x-request-id", "x-trace-id", "authorization", "cookie", "user-agent"}

def make_cache_key(agent_id: str, method: str, path: str, query: str, body: bytes, headers: dict) -> str:
    """SHA-256 of canonicalized request. Returns 'cache:{agent_id}:{hexdigest}'."""
    # Canonical body: if JSON, parse and re-dump with sort_keys; else use bytes as-is.
    try:
        parsed = json.loads(body) if body else None
        canonical_body = json.dumps(parsed, sort_keys=True, separators=(",", ":")) if parsed is not None else ""
    except json.JSONDecodeError:
        canonical_body = body.decode("utf-8", errors="replace")

    relevant_headers = sorted(
        f"{k.lower()}:{v}" for k, v in headers.items()
        if k.lower() not in SKIP_HEADERS and not k.lower().startswith("x-rarl")
    )

    payload = f"{method.upper()}|{path}|{query}|{canonical_body}|{'|'.join(relevant_headers)}"
    digest = hashlib.sha256(payload.encode("utf-8")).hexdigest()
    return f"cache:{agent_id}:{digest}"


class RedisCache:
    def __init__(self, redis: aioredis.Redis):
        self.redis = redis
        self.hits = 0
        self.misses = 0

    async def get(self, key: str) -> dict | None:
        raw = await self.redis.get(key)
        if raw is None:
            self.misses += 1
            return None
        self.hits += 1
        return json.loads(raw)

    async def set(self, key: str, value: dict, ttl: int) -> None:
        await self.redis.set(key, json.dumps(value), ex=ttl)

    async def purge(self, agent_id: str | None = None) -> int:
        pattern = f"cache:{agent_id}:*" if agent_id else "cache:*"
        count = 0
        async for k in self.redis.scan_iter(match=pattern, count=200):
            await self.redis.delete(k)
            count += 1
        return count

    @property
    def hit_rate(self) -> float:
        total = self.hits + self.misses
        return self.hits / total if total else 0.0
```

## 2.2 Write `app/coalescer.py`

**Cursor Prompt:**
```
Create app/coalescer.py exactly as follows (paste, then format):

import asyncio
from typing import Any, Awaitable, Callable

class SingleFlight:
    """Collapses concurrent identical requests into a single upstream call."""

    def __init__(self):
        self._inflight: dict[str, asyncio.Future] = {}
        self._lock = asyncio.Lock()
        self.coalesced_count = 0

    async def do(self, key: str, fn: Callable[[], Awaitable[Any]]) -> tuple[Any, bool]:
        """Returns (result, was_leader). was_leader=False means this call was coalesced."""
        async with self._lock:
            existing = self._inflight.get(key)
            if existing is not None:
                self.coalesced_count += 1
                fut = existing
                leader = False
            else:
                fut = asyncio.get_running_loop().create_future()
                self._inflight[key] = fut
                leader = True

        if not leader:
            return await fut, False

        try:
            result = await fn()
            if not fut.done():
                fut.set_result(result)
            return result, True
        except Exception as e:
            if not fut.done():
                fut.set_exception(e)
            raise
        finally:
            async with self._lock:
                self._inflight.pop(key, None)
```

## 2.3 Update `app/main.py` to wire cache + coalescer into app.state

**Cursor Prompt:**
```
Modify app/main.py lifespan:

1. Import RedisCache from app.cache and SingleFlight from app.coalescer.
2. After connecting to Redis, set:
   app.state.cache = RedisCache(app.state.redis)
   app.state.singleflight = SingleFlight()
3. Leave everything else alone.

Do NOT modify app.proxy in this prompt.
```

## 2.4 Update `app/proxy.py` to use cache + single-flight

**Cursor Prompt:**
```
Replace app/proxy.py's handle_request with the cache-aware version.

Logic:
1. Look up upstream URL; 404 if missing.
2. Read body, method, query string, headers.
3. If method in CACHEABLE_METHODS (import from app.cache):
   a. Build cache_key via make_cache_key.
   b. cached = await request.app.state.cache.get(cache_key)
   c. If cached: return Response with cached["body"], cached["status"], cached["headers"], plus X-Cache: HIT and X-RARL-Phase: 2-cache.
4. Define an inner async def fetch_upstream(): forward via httpx (same as Phase 1) and return a dict {"status","body","headers"} where body is base64-encoded if non-utf-8 otherwise raw text. Use status_code; copy response headers but drop content-encoding/transfer-encoding/content-length.
5. If cacheable:
   result, was_leader = await request.app.state.singleflight.do(cache_key, fetch_upstream)
   If was_leader and 200 <= result["status"] < 300:
       await cache.set(cache_key, result, settings.default_ttl_seconds)
   Return Response with X-Cache: MISS (if leader) or COALESCED (if not), X-RARL-Phase: 2-cache.
6. If not cacheable, just call fetch_upstream() once and return.

Use httpx.AsyncClient module-level singleton. Be defensive about binary bodies (use base64 for storage, decode for return).
Add helper functions; keep handle_request under 60 lines.
```

## 2.5 Build the load-tester `tests/demo_load.py`

**Cursor Prompt:**
```
Create tests/demo_load.py — a standalone async script using httpx.AsyncClient.

Behavior:
- argparse args: --url (required), --concurrency (default 100), --requests (default 100), --body (optional file path).
- Fire N concurrent identical POST requests using asyncio.gather.
- For each response, capture status, latency_ms, and the X-Cache header.
- At the end, print:
    Total: N
    Status codes: {200: x, 503: y, ...}
    X-Cache breakdown: {HIT: a, MISS: b, COALESCED: c}
    Latency p50 / p95 / p99 / max in ms

Use only stdlib + httpx. No external dependencies.
```

## 2.6 Verify Phase 2

```bash
docker compose -f docker-compose.local.yml -f docker-compose.override.yml --env-file .nasiko-local.env up -d --build rarl

# Single request — should be MISS the first time, HIT the second
curl -i -X POST http://localhost:8010/agents/translator/translate \
  -H 'Content-Type: application/json' \
  -d '{"text":"hello world","target":"fr"}'
# Look for: X-Cache: MISS

curl -i -X POST http://localhost:8010/agents/translator/translate \
  -H 'Content-Type: application/json' \
  -d '{"text":"hello world","target":"fr"}'
# Look for: X-Cache: HIT (and noticeably faster)

# Coalesce test (the money shot)
echo '{"text":"coalesce me","target":"de"}' > /tmp/body.json
python tests/demo_load.py \
  --url http://localhost:8010/agents/translator/translate \
  --concurrency 100 --requests 100 --body /tmp/body.json
# Expect: ~1 MISS, ~99 COALESCED, all 200s
```

✅ **Phase 2 complete when the coalesce test shows 1 MISS + 99 COALESCED.** Commit: `git commit -am "phase 2: cache + single-flight"`

---

# PHASE 3 — Per-Agent Rate Limiting + Bounded Queue + ETA (01:00 → 01:30)

**Goal:** Each agent has its own token bucket. Excess requests queue (not reject), get an ETA back. Queue overflow returns 503. End state: 80 requests at 5 rps to a slow agent finish with zero failures and accurate ETAs.

## 3.1 Write `app/ratelimit.py`

**Cursor Prompt:**
```
Create app/ratelimit.py with a TokenBucket class:

import time

class TokenBucket:
    def __init__(self, rate_per_sec: float, burst: int):
        self.rate = float(rate_per_sec)
        self.capacity = int(burst)
        self.tokens = float(burst)
        self.updated = time.monotonic()

    def _refill(self) -> None:
        now = time.monotonic()
        elapsed = now - self.updated
        self.tokens = min(self.capacity, self.tokens + elapsed * self.rate)
        self.updated = now

    def try_acquire(self, n: int = 1) -> bool:
        self._refill()
        if self.tokens >= n:
            self.tokens -= n
            return True
        return False

    def time_until_available(self, n: int = 1) -> float:
        self._refill()
        if self.tokens >= n:
            return 0.0
        return (n - self.tokens) / self.rate
```

## 3.2 Write `app/queue_lane.py`

**Cursor Prompt:**
```
Create app/queue_lane.py defining AgentLane.

Behavior: each AgentLane owns its own bucket, asyncio.PriorityQueue, semaphore for in-flight cap, and rolling latency window. A background worker drains the queue, respecting the bucket and the in-flight semaphore.

Code:

import asyncio
import collections
import itertools
import time
from typing import Any, Awaitable, Callable
from app.ratelimit import TokenBucket


class QueueOverflow(Exception):
    pass


class AgentLane:
    _seq = itertools.count()

    def __init__(self, agent_id: str, rps: float, burst: int,
                 max_inflight: int, max_queue: int = 100):
        self.agent_id = agent_id
        self.bucket = TokenBucket(rps, burst)
        self.queue: asyncio.PriorityQueue = asyncio.PriorityQueue(maxsize=max_queue)
        self.max_inflight = max_inflight
        self.semaphore = asyncio.Semaphore(max_inflight)
        self.latencies: collections.deque[float] = collections.deque(maxlen=50)
        self.ema = 0.5  # seconds, seed
        self.served = 0
        self.queued_total = 0
        self.rejected = 0
        self._worker_task = asyncio.create_task(self._worker())

    @property
    def queue_depth(self) -> int:
        return self.queue.qsize()

    @property
    def p95(self) -> float:
        if not self.latencies:
            return 0.0
        s = sorted(self.latencies)
        return s[max(0, int(0.95 * len(s)) - 1)]

    def _record_latency(self, dur: float) -> None:
        self.latencies.append(dur)
        self.ema = 0.2 * dur + 0.8 * self.ema

    async def submit(self, priority: int, factory: Callable[[], Awaitable[Any]]) -> tuple[Any, dict]:
        """
        Returns (result, meta) where meta = {"queue_position", "eta_seconds", "wait_seconds"}.
        Raises QueueOverflow if the queue is full.
        """
        # Fast path: bucket has tokens AND queue is empty AND we have inflight capacity
        if self.bucket.try_acquire() and self.queue.empty() and not self.semaphore.locked():
            t0 = time.monotonic()
            async with self.semaphore:
                started = time.monotonic()
                try:
                    result = await factory()
                    return result, {"queue_position": 0, "eta_seconds": 0.0,
                                    "wait_seconds": time.monotonic() - t0}
                finally:
                    self._record_latency(time.monotonic() - started)
                    self.served += 1

        # Slow path: enqueue
        if self.queue.full():
            self.rejected += 1
            raise QueueOverflow(f"queue full for agent {self.agent_id}")

        position = self.queue.qsize()
        eta = (position + 1) * self.ema / max(1, self.max_inflight)
        fut: asyncio.Future = asyncio.get_running_loop().create_future()
        enqueued_at = time.monotonic()
        await self.queue.put((priority, next(AgentLane._seq), fut, factory, enqueued_at))
        self.queued_total += 1

        result = await fut
        return result, {
            "queue_position": position,
            "eta_seconds": eta,
            "wait_seconds": time.monotonic() - enqueued_at,
        }

    async def _worker(self) -> None:
        while True:
            try:
                priority, _, fut, factory, _ = await self.queue.get()
            except asyncio.CancelledError:
                break

            try:
                # Respect the bucket
                while True:
                    if self.bucket.try_acquire():
                        break
                    await asyncio.sleep(self.bucket.time_until_available() + 0.01)

                async with self.semaphore:
                    started = time.monotonic()
                    try:
                        result = await factory()
                        if not fut.done():
                            fut.set_result(result)
                    except Exception as e:
                        if not fut.done():
                            fut.set_exception(e)
                    finally:
                        self._record_latency(time.monotonic() - started)
                        self.served += 1
            finally:
                self.queue.task_done()
```

## 3.3 Update `app/main.py` lifespan to register lanes lazily

**Cursor Prompt:**
```
Add to app/main.py:

1. Import AgentLane from app.queue_lane.
2. In lifespan, after creating cache and singleflight, set app.state.lanes = {} (typed as dict[str, AgentLane]).
3. Add a helper function get_or_create_lane(app, agent_id) that:
   - If agent_id in app.state.lanes, return it.
   - Else, look up rps/burst/max_inflight/max_queue from settings.default_*.
   - Create AgentLane, store in dict, return.

Export get_or_create_lane so proxy.py can import it.
```

## 3.4 Update `app/proxy.py` to route through the lane

**Cursor Prompt:**
```
Modify app/proxy.py handle_request:

After the cache MISS path, BEFORE calling fetch_upstream directly, route through the lane:

from app.main import get_or_create_lane
from app.queue_lane import QueueOverflow

priority = int(request.headers.get("X-Priority", "5"))
lane = get_or_create_lane(request.app, agent_id)

try:
    if cacheable:
        # single-flight wraps the lane.submit
        async def upstream_via_lane():
            result, meta = await lane.submit(priority, fetch_upstream)
            return {"result": result, "meta": meta}
        wrapped, was_leader = await request.app.state.singleflight.do(cache_key, upstream_via_lane)
        result = wrapped["result"]
        meta = wrapped["meta"]
    else:
        result, meta = await lane.submit(priority, fetch_upstream)
        was_leader = True

except QueueOverflow:
    return Response(
        content=b'{"error":"queue_full","retry_after_seconds":5}',
        status_code=503,
        media_type="application/json",
        headers={"Retry-After": "5", "X-RARL-Phase": "3-queue"},
    )

# Add headers from meta
extra_headers = {
    "X-Queue-Position": str(meta["queue_position"]),
    "X-Queue-ETA-Seconds": f"{meta['eta_seconds']:.3f}",
    "X-Queue-Wait-Seconds": f"{meta['wait_seconds']:.3f}",
    "X-RARL-Phase": "3-queue",
    "X-Cache": "HIT" if was_from_cache else ("COALESCED" if not was_leader else "MISS"),
}

(rebuild the Response with these headers merged in)
```

⚠️ **Manual review:** This is the hairiest integration. After Cursor generates this, **read every line.** Common mistakes Cursor makes here:
- Forgetting to merge `extra_headers` into the final Response
- Double-wrapping the singleflight (it should wrap the *lane.submit*, not the inner fetch)
- Not handling the case where `cached` is found (skip lane entirely on cache HIT)

Fix any of those before testing.

## 3.5 Verify Phase 3

```bash
# Rebuild
docker compose -f docker-compose.local.yml -f docker-compose.override.yml --env-file .nasiko-local.env up -d --build rarl

# Override the rate to something tiny to make queueing observable.
# Either edit the override.yml env vars or use the admin API once Phase 4 lands.
# For now, set RPS via env: DEFAULT_RPS=2 DEFAULT_BURST=2 DEFAULT_MAX_INFLIGHT=1

# Spike test against an UNCACHEABLE endpoint (vary the body)
python tests/demo_load.py \
  --url http://localhost:8010/agents/translator/translate \
  --concurrency 30 --requests 30 \
  # use a script that sends DIFFERENT bodies; modify demo_load to support --vary

# Expect: zero 503s if requests <= max_queue, X-Queue-ETA values present, total time ~ requests/rps
```

✅ **Phase 3 complete when 30 concurrent unique requests at rps=2 all return 200 with sane ETAs.** Commit: `git commit -am "phase 3: per-agent token bucket + queue + eta"`

---

# PHASE 4 — Admin API + Metrics (01:30 → 02:00)

**Goal:** Operational endpoints for cache management, runtime config changes, and live stats. End state: judge can `curl /admin/agents` and see live state, `PUT /admin/agents/translator/config` to retune limits without restart.

## 4.1 Write `app/metrics.py`

**Cursor Prompt:**
```
Create app/metrics.py with a Metrics class that holds:
- ring buffers (collections.deque maxlen=120) for: qps_in, qps_upstream, p95_latency_client, p95_latency_upstream, total_cache_hit_rate
- per-agent counters via dict[str, dict]
- methods to record: record_request(agent_id, was_cache_hit, was_coalesced, client_latency, upstream_latency_or_none)
- a snapshot() method that returns a dict suitable for JSON serialization

Internally, sample once per second via an asyncio task (started by main.py lifespan). On each tick, compute QPS, append to deques, reset counters.
```

## 4.2 Write `app/admin.py`

**Cursor Prompt:**
```
Create app/admin.py defining an APIRouter with prefix /admin and these endpoints:

GET  /agents                          # list all known lanes with current config and live stats
GET  /agents/{agent_id}               # detail for one agent (includes queue depth, p95, ema, served, rejected)
PUT  /agents/{agent_id}/config        # body: {"rps":?, "burst":?, "ttl":?, "max_inflight":?}; updates live without restart
POST /cache/purge                     # query param: agent (optional); returns {"purged": N}
GET  /cache/stats                     # {"hits","misses","hit_rate","coalesced","keys_count"}
GET  /explain                         # query param: cache_key OR last=1 returns the most recent decision
                                      # Returns {"cache":"HIT|MISS|COALESCED|NOT_CACHEABLE", "ttl_remaining":?,
                                      #          "queue_position":?, "eta":?, "rate_limited":bool, "agent_id":?}

Implementation notes:
- Updating rps/burst should mutate the existing TokenBucket: set lane.bucket.rate, lane.bucket.capacity, and reset tokens to min(current, new_capacity).
- For /explain, store the last 50 request decisions in a collections.deque on app.state (key by request id from response header X-Request-Id; generate one if missing).
- Use Pydantic models for request bodies; use response_model where helpful.

Mount this router in app/main.py via app.include_router(admin.router).
```

## 4.3 Add request-id and decision logging to `app/proxy.py`

**Cursor Prompt:**
```
Modify app/proxy.py to:
1. Generate a request_id (uuid4 hex first 12 chars) at top of handle_request.
2. Track a "decision" dict throughout: {"request_id","agent_id","cache","queue_position","eta","timestamp"}.
3. After the response is built, append the decision to app.state.recent_decisions (a deque maxlen=50, initialized in main.py lifespan).
4. Add response header "X-Request-Id".

Also call request.app.state.metrics.record_request(...) at the end of every request.
```

## 4.4 Verify Phase 4

```bash
docker compose -f docker-compose.local.yml -f docker-compose.override.yml --env-file .nasiko-local.env up -d --build rarl

# After firing some traffic via demo_load.py:
curl -s http://localhost:8010/admin/agents | jq
curl -s http://localhost:8010/admin/cache/stats | jq
curl -s 'http://localhost:8010/admin/explain?last=1' | jq

# Live retune
curl -X PUT http://localhost:8010/admin/agents/translator/config \
  -H 'Content-Type: application/json' \
  -d '{"rps": 50, "burst": 100}'

# Verify the change reflected
curl -s http://localhost:8010/admin/agents/translator | jq

# Cache purge
curl -X POST 'http://localhost:8010/admin/cache/purge?agent=translator'
```

✅ **Phase 4 complete when all admin endpoints return sensible JSON.** Commit: `git commit -am "phase 4: admin api + metrics"`

---

# PHASE 5 — Real-Time Dashboard (02:00 → 02:30)

**Goal:** Single-page HTML dashboard at `/dashboard` with live charts via Server-Sent Events. End state: judge sees QPS, hit rate, queue depth, latency updating every 500 ms.

## 5.1 Write `app/dashboard.py`

**Cursor Prompt:**
```
Create app/dashboard.py with an APIRouter:

GET /dashboard         # serves the HTML page (Jinja2 template at app/templates/dashboard.html)
GET /admin/stream      # Server-Sent Events endpoint, emits a JSON snapshot every 500 ms

For SSE use sse_starlette.EventSourceResponse. The async generator should:
- yield event_data = {"event": "snapshot", "data": json.dumps(metrics_snapshot)} every 0.5s
- read snapshot from request.app.state.metrics.snapshot()
- include: timestamp, total_requests, hit_rate, coalesced_count, qps_in, qps_upstream,
           p95_client_ms, p95_upstream_ms, lanes: [{agent_id, queue_depth, rps, served, rejected, p95}]

Mount the router in app/main.py.
Use Jinja2Templates pointing at "app/templates".
```

## 5.2 Write `app/templates/dashboard.html`

**Cursor Prompt:**
```
Create app/templates/dashboard.html — a single self-contained HTML page.

Requirements:
- <head>: import Chart.js from CDN (https://cdn.jsdelivr.net/npm/chart.js).
- Dark theme, monospace, hacker-aesthetic. Title: "RARL — Resilient Agent Request Layer".
- Top row: 4 big-number tiles
    1. Total Requests
    2. Cache Hit Rate (%)
    3. Coalesced Calls
    4. Active Agents
- Middle row: 2 line charts side by side
    Left: "QPS In vs QPS to Agents" (two series, last 60 samples)
    Right: "p95 Latency: Client View vs Agent View" (ms)
- Bottom row: 1 stacked bar chart per agent showing queue depth, with a small table beside it listing each agent's current rps, burst, served, rejected, p95.
- A prominent red button labeled "🚀 Spike Test (200 req)" that POSTs to /admin/spike-test (we'll add this endpoint in phase 6).
- JavaScript:
    const es = new EventSource("/admin/stream");
    es.addEventListener("snapshot", (e) => { const data = JSON.parse(e.data); update(data); });
- Charts auto-update on every snapshot. Keep last 60 samples per chart.
- No external CSS framework. Inline <style> only.
- Fonts: system-ui or 'Courier New' monospace.

Make it look polished — judges grade visual presentation.
```

## 5.3 Verify Phase 5

```bash
docker compose -f docker-compose.local.yml -f docker-compose.override.yml --env-file .nasiko-local.env up -d --build rarl
```

Open `http://localhost:8010/dashboard` in your browser. In another terminal, run `python tests/demo_load.py` against the gateway and watch the charts move.

✅ **Phase 5 complete when the dashboard updates live as load is applied.** Commit: `git commit -am "phase 5: live dashboard"`

---

# PHASE 6 — Standout Add-Ons + Demo Polish (02:30 → 03:00)

**Goal:** Ship the differentiators that make judges remember you. In strict order of impact-per-minute:

## 6.1 (10 min) Spike-Test Button Endpoint

**Cursor Prompt:**
```
Add to app/admin.py:

POST /admin/spike-test
Body: {"agent_id": "translator", "concurrency": 200, "requests": 200, "vary": false, "endpoint":"/translate","payload":{"text":"hello","target":"fr"}}

Implementation:
- Use httpx.AsyncClient; fire N concurrent requests AGAINST our own /agents/{agent_id}/{endpoint} (i.e. http://localhost:8010/agents/...).
- If vary=true, append a unique counter to payload["text"] so each request is distinct (forces queue path, not cache).
- Return {"status_codes":{...}, "x_cache_breakdown":{...}, "p50_ms":?, "p95_ms":?, "p99_ms":?, "max_ms":?, "duration_s":?}.
- Set a hard cap: concurrency<=500, requests<=1000.
```

Update `dashboard.html` so the red button calls this endpoint with `vary=false` (cache/coalesce demo) and a second smaller button calls it with `vary=true` (queue demo).

## 6.2 (10 min) `/explain` polish — make it a "wow" endpoint

**Cursor Prompt:**
```
Enhance GET /admin/explain in app/admin.py:

Without args, return the last 10 decisions formatted as a list.
With ?request_id=abc, return that specific decision plus:
- timestamp_iso
- a human-readable reason: "Served from Redis cache (TTL 247s remaining)" or
                          "Coalesced into in-flight request abc12 — saved 1 agent call" or
                          "Queued at position 3, ETA 1.6s, served in 1.4s" or
                          "Forwarded directly — bucket had tokens, queue empty"
- the cache key
- the agent's current rps/burst at time of decision

This endpoint is the differentiator. Make the human-readable reason vivid.
```

Show this in the demo: after running spike-test, hit `/admin/explain` and read the reasons aloud.

## 6.3 (10 min) Adaptive Tuner (only if you have time)

**Cursor Prompt:**
```
Add to app/main.py lifespan a background task adaptive_tuner():

async def adaptive_tuner(app: FastAPI, target_p95: float = 1.0):
    while True:
        await asyncio.sleep(10)
        for lane in app.state.lanes.values():
            if len(lane.latencies) < 10:
                continue
            observed_p95 = lane.p95
            ratio = max(0.5, min(1.5, target_p95 / max(observed_p95, 0.05)))
            new_rate = max(1.0, lane.bucket.rate * ratio)
            lane.bucket.rate = new_rate
            lane.bucket.capacity = max(2, int(new_rate * 2))

Start it as a task in lifespan. Add a flag in settings: adaptive_enabled (default False).
Add an admin endpoint POST /admin/adaptive {"enabled": true} to toggle live.

In the dashboard, add a small toggle switch labeled "Adaptive Mode" wired to that endpoint.
```

## 6.4 README + demo script

**Cursor Prompt:**
```
Generate a README.md for the project covering:

1. One-paragraph problem statement (the original from Nasiko Buildthon).
2. Architecture diagram (ASCII, like the one in BUILD_PLAN.md).
3. Quick start: cp docker-compose.override.yml into nasiko/, docker compose up.
4. Endpoints table: every /agents/* /admin/* /dashboard.
5. The 4 demo "money shots" (numbered, with exact curl commands).
6. Honest "Known limitations" section: single-instance only, in-process single-flight, no auth on admin.
7. Tech stack and credits.

Keep it under 300 lines. Use markdown tables.
```

## 6.5 Final Verification — the full demo dry-run (5 min)

Run the demo end-to-end exactly as you'll show the judges:

1. Open `http://localhost:8010/dashboard` — show the live charts at idle.
2. Run a single request manually, show `X-Cache: MISS` then re-run, show `X-Cache: HIT`.
3. Click "Spike Test" (vary=false). Watch QPS-in spike to 200, QPS-out stay at 1, Coalesced counter jump to 199.
4. Click "Spike Test" (vary=true) at low rate. Watch the queue-depth chart climb and drain. Zero failures.
5. `curl /admin/explain?last=1` — read out loud: "Coalesced into in-flight request a3f9e — saved 1 agent call."
6. `curl PUT /admin/agents/translator/config -d '{"rps":1}'` then run vary=true again — show queue depth growing more, ETAs accurately predicted.

✅ **All 4 success criteria visibly demonstrated.** Final commit: `git commit -am "phase 6: standout features + demo polish"`

---

# Cut-Lines (if you fall behind)

| Behind by | Cut these (in order) |
|---|---|
| 15 min at 02:30 | Adaptive tuner (skip 6.3 entirely) |
| 30 min at 02:00 | Adaptive tuner + the second SSE chart (keep just QPS + queue) |
| 45 min at 01:30 | Adaptive tuner + dashboard (replace with a single auto-refresh `<meta http-equiv="refresh" content="1">` page that just shows `/admin/agents` JSON) |
| 60 min at 01:00 | Drop priority queue (use plain asyncio.Queue), drop /explain reasoning (just return cache status) |

You will still satisfy 100% of the original problem-statement requirements at every cut-line above. **Never cut: cache, single-flight, per-agent token bucket, queue, /admin endpoints, basic dashboard.** Those four are the rubric.

---

# Cursor-Specific Tactics

These will save you 30+ minutes of friction:

1. **Use Composer (`Cmd/Ctrl+I`), not inline chat (`Cmd/Ctrl+K`)** — Composer can edit multiple files in one shot. Inline is for small tweaks.
2. **Always start a new Composer session per phase.** Old sessions accumulate context noise and Cursor starts hallucinating prior decisions. New phase = new chat.
3. **Pin BUILD_PLAN.md and `.cursorrules` to every Composer session** (drag them into the context panel). This anchors Cursor's behavior.
4. **When Cursor generates wrong code, don't argue with it — undo and re-prompt with one extra constraint.** E.g., if it produces a sync function in async context: undo, re-prompt with "must be async, no blocking calls."
5. **After every phase, run `git diff` and skim every line.** Cursor occasionally introduces silent changes to unrelated files. Catch them now, not at 02:55.
6. **For Phase 3 (the trickiest), generate one file at a time.** Don't ask Composer to edit `ratelimit.py`, `queue_lane.py`, `main.py`, and `proxy.py` in one prompt. The merge logic in `proxy.py` is where Cursor most often gets confused.
7. **Use Cursor's `@Codebase` reference** when you need it to understand cross-file dependencies — e.g., "Update @app/main.py to wire the SingleFlight from @app/coalescer.py into app.state."
8. **Never let Cursor write the demo script.** Write `demo_load.py`'s argparse and output yourself in 5 min — you'll iterate it 10× during demo prep, and AI-generated test scripts often have subtle off-by-ones in the timing math that bite you live.

---

# Final 60-Second Pre-Demo Checklist

- [ ] Nasiko stack up; `nasiko-rarl` container green
- [ ] Translator agent deployed and `/agents/translator/health` returns 200 through RARL
- [ ] One cached entry pre-warmed (run a curl before the demo so the very first HIT is instant)
- [ ] Dashboard open in browser, idle and showing zeros cleanly
- [ ] Two terminal panes ready: one for `curl` shots, one for `tail -f` of the rarl logs
- [ ] `tests/demo_load.py` smoke-tested in the last 10 min
- [ ] You can name (without notes) which success criterion each money shot demonstrates
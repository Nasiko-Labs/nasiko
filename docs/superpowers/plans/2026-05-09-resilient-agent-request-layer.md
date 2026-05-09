# Resilient Agent Request Layer Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a dedicated Request Manager service that centralizes agent response caching, single-flight duplicate suppression, per-agent rate limiting, bounded queueing, circuit breaking, runtime controls, and KPI demo visibility for all `/agents/*` traffic.

**Architecture:** Kong remains the public gateway, but dynamic `/agents/{agent}` routes are rewired to a new Request Manager service instead of directly to agent containers. `agent-gateway/registry/registry.py` remains the discovery owner and publishes internal agent targets into Redis; the Request Manager resolves those targets and proxies directly to internal agent URLs so it never loops back through Kong. The existing router contract stays unchanged: routed traffic still flows `Client -> Kong -> Router -> Kong /agents/{agent} -> Request Manager -> Agent`.

**Tech Stack:** Python 3.11, FastAPI, Uvicorn, HTTPX, Redis asyncio client, Docker Compose, Kong Admin API, pytest, pytest-asyncio.

---

## Source Spec

Implement this plan against:

- `docs/superpowers/specs/2026-05-09-resilient-agent-request-layer-design.md`

The approved decisions from that spec are binding:

- Request Manager is the only new execution-control layer.
- Router client contract does not change in MVP.
- Registry publishes internal agent target records to Redis.
- Kong dynamic `/agents/*` routes point to Request Manager.
- Request Manager proxies to internal agent hosts, not public Kong `/agents/*` URLs.
- Cache only safe, text-only A2A JSON-RPC `message/send` upstream JSON responses.
- Cache hits bypass rate limits and queueing.
- Cache misses use single-flight, per-agent concurrency, token bucket RPS, bounded FIFO queue, and circuit breaker.
- Operational endpoints and a live dashboard are part of MVP.

## File Structure

Create these files:

- `agent-gateway/request-manager/Dockerfile`: container image for the Request Manager.
- `agent-gateway/request-manager/requirements.txt`: runtime and test dependencies.
- `agent-gateway/request-manager/request_manager/__init__.py`: package marker.
- `agent-gateway/request-manager/request_manager/settings.py`: environment-driven settings.
- `agent-gateway/request-manager/request_manager/models.py`: shared Pydantic models.
- `agent-gateway/request-manager/request_manager/redis_keys.py`: Redis key helpers.
- `agent-gateway/request-manager/request_manager/cache.py`: cacheability classifier, cache key builder, Redis cache store.
- `agent-gateway/request-manager/request_manager/singleflight.py`: Redis-backed single-flight lock/wait helper.
- `agent-gateway/request-manager/request_manager/limiter.py`: Redis-backed token bucket, concurrency, queue, and degraded local limiter.
- `agent-gateway/request-manager/request_manager/circuit_breaker.py`: per-agent rolling failure circuit breaker.
- `agent-gateway/request-manager/request_manager/metrics.py`: Redis-backed counters, latency samples, and stat snapshots.
- `agent-gateway/request-manager/request_manager/target_resolver.py`: Redis agent target lookup.
- `agent-gateway/request-manager/request_manager/proxy.py`: request classification, cache/limit/proxy orchestration.
- `agent-gateway/request-manager/request_manager/dashboard.py`: simple HTML dashboard.
- `agent-gateway/request-manager/request_manager/main.py`: FastAPI routes.
- `agent-gateway/request-manager/tests/conftest.py`: test fixtures.
- `agent-gateway/request-manager/tests/test_cache_policy.py`: cache key and cacheability tests.
- `agent-gateway/request-manager/tests/test_singleflight.py`: duplicate miss collapse tests.
- `agent-gateway/request-manager/tests/test_limiter.py`: concurrency, queue, and overflow tests.
- `agent-gateway/request-manager/tests/test_circuit_breaker.py`: closed/open/half-open tests.
- `agent-gateway/request-manager/tests/test_proxy_flow.py`: proxy behavior and headers tests.
- `agent-gateway/registry/target_publisher.py`: registry-owned Redis target publisher.
- `agent-gateway/registry/tests/test_target_publisher.py`: target record tests.
- `scripts/request-layer/demo_cache_latency.py`: KPI demo for faster repeated responses.
- `scripts/request-layer/demo_singleflight.py`: KPI demo for reduced duplicate processing.
- `scripts/request-layer/demo_overload.py`: KPI demo for stable overload handling.

Modify these files:

- `agent-gateway/registry/requirements.txt`: add Redis and pytest dependency entries.
- `agent-gateway/registry/registry.py`: publish targets to Redis, point dynamic routes to Request Manager, preserve static routes.
- `docker-compose.local.yml`: add `nasiko-request-manager`, wire registry env vars and dependencies.

Do not modify these files for MVP:

- `agent-gateway/router/src/core/agent_client.py`
- `agent-gateway/router/src/main.py`

The router path is intentionally preserved.

## Redis Key Contract

Use these exact Redis keys:

- Target index: `request-manager:targets`
- Target hash: `request-manager:targets:{agent_id}`
- Cache entry: `request-manager:cache:{cache_key}`
- Single-flight lock: `request-manager:singleflight:{cache_key}`
- Single-flight ready marker: `request-manager:singleflight:ready:{cache_key}`
- Per-agent limits override: `request-manager:limits:{agent_id}`
- Per-agent active counter: `request-manager:active:{agent_id}`
- Global active counter: `request-manager:active:global`
- Per-agent queue list: `request-manager:queue:{agent_id}`
- Per-agent token bucket hash: `request-manager:bucket:{agent_id}`
- Per-agent circuit hash: `request-manager:circuit:{agent_id}`
- Per-agent outcome list: `request-manager:outcomes:{agent_id}`
- Global metrics hash: `request-manager:metrics:global`
- Per-agent metrics hash: `request-manager:metrics:{agent_id}`
- Per-agent latency samples: `request-manager:latency:{agent_id}`
- Per-agent queue wait samples: `request-manager:queue-wait:{agent_id}`

## Request Manager Defaults

Use these exact default values unless an environment variable or Redis per-agent override replaces them:

- Cache TTL seconds: `600`
- Max concurrency per agent: `2`
- Sustained RPS per agent: `5.0`
- Burst capacity per agent: `10`
- Max queue depth per agent: `20`
- Max queue wait milliseconds: `10000`
- Upstream agent timeout seconds: `45.0`
- Global active request cap: `50`
- Circuit rolling window: `20`
- Circuit minimum failures: `5`
- Circuit failure ratio: `0.5`
- Circuit open seconds: `30`
- Single-flight wait milliseconds: `10000`
- Redis operation timeout seconds: `1.0`

## Response Headers

Every Request Manager response should include these headers when applicable:

- `X-Request-Layer-Agent`: resolved agent id.
- `X-Request-Layer-Cache`: `HIT`, `MISS`, or `BYPASS`.
- `X-Request-Layer-Queue-Wait-Ms`: integer queue wait.
- `X-Request-Layer-Limit-State`: `normal`, `degraded`, or `circuit-open`.

## Task 1: Scaffold The Request Manager Package

**Files:**

- Create: `agent-gateway/request-manager/requirements.txt`
- Create: `agent-gateway/request-manager/Dockerfile`
- Create: `agent-gateway/request-manager/request_manager/__init__.py`
- Create: `agent-gateway/request-manager/request_manager/settings.py`
- Create: `agent-gateway/request-manager/request_manager/models.py`
- Create: `agent-gateway/request-manager/request_manager/redis_keys.py`
- Create: `agent-gateway/request-manager/request_manager/main.py`
- Create: `agent-gateway/request-manager/tests/conftest.py`

- [ ] **Step 1: Create the dependency file**

Create `agent-gateway/request-manager/requirements.txt` with this content:

```text
fastapi==0.116.1
uvicorn[standard]==0.35.0
httpx==0.28.1
redis==5.0.8
pydantic==2.11.9
pydantic-settings==2.10.1
pytest==8.3.5
pytest-asyncio==0.23.8
```

- [ ] **Step 2: Create the Dockerfile**

Create `agent-gateway/request-manager/Dockerfile` with this content:

```dockerfile
FROM python:3.11-slim

WORKDIR /app

RUN apt-get update && apt-get install -y curl && rm -rf /var/lib/apt/lists/*

COPY requirements.txt .
RUN pip install --no-cache-dir -r requirements.txt

COPY request_manager ./request_manager

ENV PYTHONPATH=/app
ENV PYTHONUNBUFFERED=1

HEALTHCHECK --interval=30s --timeout=10s --start-period=5s --retries=3 \
    CMD python -c "from urllib.request import urlopen; urlopen('http://localhost:8090/health')" || exit 1

CMD ["uvicorn", "request_manager.main:app", "--host", "0.0.0.0", "--port", "8090"]
```

- [ ] **Step 3: Create the package marker**

Create `agent-gateway/request-manager/request_manager/__init__.py` with this content:

```python
"""Nasiko Request Manager service."""
```

- [ ] **Step 4: Create settings**

Create `agent-gateway/request-manager/request_manager/settings.py` with this content:

```python
from functools import lru_cache

from pydantic import Field
from pydantic_settings import BaseSettings, SettingsConfigDict


class RequestManagerSettings(BaseSettings):
    model_config = SettingsConfigDict(
        env_prefix="REQUEST_MANAGER_",
        extra="ignore",
        populate_by_name=True,
    )

    redis_url: str = Field(default="redis://redis:6379", validation_alias="REDIS_URL")
    service_name: str = "nasiko-request-manager"
    cache_ttl_seconds: int = 600
    max_concurrency_per_agent: int = 2
    sustained_rps_per_agent: float = 5.0
    burst_capacity_per_agent: int = 10
    max_queue_depth_per_agent: int = 20
    max_queue_wait_ms: int = 10_000
    upstream_timeout_seconds: float = 45.0
    global_active_cap: int = 50
    circuit_window_size: int = 20
    circuit_min_failures: int = 5
    circuit_failure_ratio: float = 0.5
    circuit_open_seconds: int = 30
    singleflight_wait_ms: int = 10_000
    redis_timeout_seconds: float = 1.0
    admin_token: str | None = None


@lru_cache
def get_settings() -> RequestManagerSettings:
    return RequestManagerSettings()
```

- [ ] **Step 5: Create shared models**

Create `agent-gateway/request-manager/request_manager/models.py` with this content:

```python
from __future__ import annotations

from enum import Enum
from typing import Any

from pydantic import BaseModel, Field, HttpUrl


class AgentTarget(BaseModel):
    agent_id: str
    public_path: str
    upstream_url: str
    target_revision: str
    source: str
    namespace: str
    updated_at: float


class AgentLimits(BaseModel):
    cache_ttl_seconds: int = Field(ge=0)
    max_concurrency: int = Field(ge=1)
    sustained_rps: float = Field(gt=0)
    burst_capacity: int = Field(ge=1)
    max_queue_depth: int = Field(ge=0)
    max_queue_wait_ms: int = Field(ge=0)
    cache_enabled: bool = True


class CacheState(str, Enum):
    hit = "HIT"
    miss = "MISS"
    bypass = "BYPASS"


class LimitState(str, Enum):
    normal = "normal"
    degraded = "degraded"
    circuit_open = "circuit-open"


class CacheDecision(BaseModel):
    cacheable: bool
    reason: str
    cache_key: str | None = None
    fingerprint: dict[str, Any] | None = None


class CachedResponse(BaseModel):
    status_code: int
    media_type: str | None = "application/json"
    body: bytes
    headers: dict[str, str] = Field(default_factory=dict)


class AcquireResult(BaseModel):
    acquired: bool
    queued: bool = False
    queue_wait_ms: int = 0
    retry_after_seconds: int = 1
    reason: str = "ok"
    degraded: bool = False


class CircuitDecision(BaseModel):
    allowed: bool
    state: str
    retry_after_seconds: int = 0


class AgentStats(BaseModel):
    agent_id: str
    active_requests: int
    queued_requests: int
    cache_hits: int
    cache_misses: int
    cache_bypasses: int
    singleflight_waiters: int
    upstream_requests: int
    upstream_errors: int
    queue_timeouts: int
    circuit_state: str
    p50_latency_ms: float
    p95_latency_ms: float
    p95_queue_wait_ms: float
    limits: AgentLimits


class GlobalStats(BaseModel):
    status: str
    redis_available: bool
    active_requests: int
    cache_hits: int
    cache_misses: int
    cache_bypasses: int
    upstream_requests: int
    upstream_errors: int
    queue_timeouts: int
    agents: list[AgentStats]
```

- [ ] **Step 6: Create Redis key helpers**

Create `agent-gateway/request-manager/request_manager/redis_keys.py` with this content:

```python
def targets_index() -> str:
    return "request-manager:targets"


def target(agent_id: str) -> str:
    return f"request-manager:targets:{agent_id}"


def cache_entry(cache_key: str) -> str:
    return f"request-manager:cache:{cache_key}"


def singleflight_lock(cache_key: str) -> str:
    return f"request-manager:singleflight:{cache_key}"


def singleflight_ready(cache_key: str) -> str:
    return f"request-manager:singleflight:ready:{cache_key}"


def limits(agent_id: str) -> str:
    return f"request-manager:limits:{agent_id}"


def active(agent_id: str) -> str:
    return f"request-manager:active:{agent_id}"


def active_global() -> str:
    return "request-manager:active:global"


def queue(agent_id: str) -> str:
    return f"request-manager:queue:{agent_id}"


def bucket(agent_id: str) -> str:
    return f"request-manager:bucket:{agent_id}"


def circuit(agent_id: str) -> str:
    return f"request-manager:circuit:{agent_id}"


def outcomes(agent_id: str) -> str:
    return f"request-manager:outcomes:{agent_id}"


def metrics_global() -> str:
    return "request-manager:metrics:global"


def metrics_agent(agent_id: str) -> str:
    return f"request-manager:metrics:{agent_id}"


def latency(agent_id: str) -> str:
    return f"request-manager:latency:{agent_id}"


def queue_wait(agent_id: str) -> str:
    return f"request-manager:queue-wait:{agent_id}"
```

- [ ] **Step 7: Create a minimal FastAPI app**

Create `agent-gateway/request-manager/request_manager/main.py` with this initial content:

```python
from fastapi import FastAPI

from request_manager.settings import get_settings

settings = get_settings()

app = FastAPI(
    title="Nasiko Request Manager",
    version="0.1.0",
    description="Traffic-control layer for Nasiko agent requests.",
)


@app.get("/health")
async def health() -> dict[str, object]:
    return {
        "status": "starting",
        "service": settings.service_name,
        "redis_available": False,
        "circuits": {},
    }
```

- [ ] **Step 8: Create pytest fixtures**

Create `agent-gateway/request-manager/tests/conftest.py` with this content:

```python
from __future__ import annotations

import asyncio
from collections import defaultdict, deque
from typing import Any

import pytest


class FakeRedis:
    def __init__(self) -> None:
        self.values: dict[str, Any] = {}
        self.hashes: dict[str, dict[str, str]] = defaultdict(dict)
        self.sets: dict[str, set[str]] = defaultdict(set)
        self.lists: dict[str, deque[str]] = defaultdict(deque)
        self.expiry: dict[str, int] = {}

    async def ping(self) -> bool:
        return True

    async def get(self, key: str) -> Any:
        return self.values.get(key)

    async def set(self, key: str, value: Any, ex: int | None = None, px: int | None = None, nx: bool = False) -> bool:
        if nx and key in self.values:
            return False
        self.values[key] = value
        if ex is not None:
            self.expiry[key] = ex * 1000
        if px is not None:
            self.expiry[key] = px
        return True

    async def delete(self, *keys: str) -> int:
        removed = 0
        for key in keys:
            if key in self.values:
                removed += 1
                self.values.pop(key, None)
            self.hashes.pop(key, None)
            self.sets.pop(key, None)
            self.lists.pop(key, None)
        return removed

    async def hset(self, key: str, mapping: dict[str, Any]) -> int:
        for field, value in mapping.items():
            self.hashes[key][field] = str(value)
        return len(mapping)

    async def hgetall(self, key: str) -> dict[str, str]:
        return dict(self.hashes.get(key, {}))

    async def hincrby(self, key: str, field: str, amount: int = 1) -> int:
        current = int(self.hashes[key].get(field, "0"))
        updated = current + amount
        self.hashes[key][field] = str(updated)
        return updated

    async def sadd(self, key: str, *values: str) -> int:
        before = len(self.sets[key])
        self.sets[key].update(values)
        return len(self.sets[key]) - before

    async def smembers(self, key: str) -> set[str]:
        return set(self.sets.get(key, set()))

    async def srem(self, key: str, *values: str) -> int:
        removed = 0
        for value in values:
            if value in self.sets[key]:
                self.sets[key].remove(value)
                removed += 1
        return removed

    async def incr(self, key: str) -> int:
        current = int(self.values.get(key, "0")) + 1
        self.values[key] = str(current)
        return current

    async def decr(self, key: str) -> int:
        current = int(self.values.get(key, "0")) - 1
        self.values[key] = str(max(current, 0))
        return int(self.values[key])

    async def rpush(self, key: str, value: str) -> int:
        self.lists[key].append(value)
        return len(self.lists[key])

    async def lpop(self, key: str) -> str | None:
        if not self.lists[key]:
            return None
        return self.lists[key].popleft()

    async def lindex(self, key: str, index: int) -> str | None:
        try:
            return list(self.lists[key])[index]
        except IndexError:
            return None

    async def llen(self, key: str) -> int:
        return len(self.lists[key])

    async def lrem(self, key: str, count: int, value: str) -> int:
        original = list(self.lists[key])
        kept = deque(item for item in original if item != value)
        removed = len(original) - len(kept)
        self.lists[key] = kept
        return removed

    async def expire(self, key: str, seconds: int) -> bool:
        self.expiry[key] = seconds * 1000
        return True


@pytest.fixture
def fake_redis() -> FakeRedis:
    return FakeRedis()


@pytest.fixture
def event_loop():
    loop = asyncio.new_event_loop()
    yield loop
    loop.close()
```

- [ ] **Step 9: Run the initial health check test manually**

Run:

```bash
cd agent-gateway/request-manager
python -m pytest -q
```

Expected:

```text
no tests ran
```

- [ ] **Step 10: Commit the scaffold**

Run:

```bash
git add agent-gateway/request-manager
git commit -m "feat: scaffold request manager service"
```

Expected:

```text
Commit succeeds and prints the new commit hash.
```

## Task 2: Add Registry Target Publishing

**Files:**

- Create: `agent-gateway/registry/target_publisher.py`
- Create: `agent-gateway/registry/tests/test_target_publisher.py`
- Modify: `agent-gateway/registry/requirements.txt`

- [ ] **Step 1: Add registry dependencies**

Append these lines to `agent-gateway/registry/requirements.txt`:

```text
redis==5.0.8
pytest==8.3.5
```

- [ ] **Step 2: Write target publisher tests**

Create `agent-gateway/registry/tests/test_target_publisher.py` with this content:

```python
import time

from target_publisher import AgentTargetRecord, build_target_record


def test_build_target_record_uses_internal_url_and_revision():
    record = build_target_record(
        agent_id="agent-a2a-demo",
        host="agent-a2a-demo",
        port=5000,
        public_path="/agents/agent-a2a-demo",
        namespace="docker-agents",
        source="docker",
        target_revision="container-123",
        now=123.4,
    )

    assert record.agent_id == "agent-a2a-demo"
    assert record.public_path == "/agents/agent-a2a-demo"
    assert record.upstream_url == "http://agent-a2a-demo:5000"
    assert record.target_revision == "container-123"
    assert record.source == "docker"
    assert record.updated_at == 123.4


def test_agent_target_record_serializes_for_redis_hash():
    record = AgentTargetRecord(
        agent_id="agent-a2a-demo",
        public_path="/agents/agent-a2a-demo",
        upstream_url="http://agent-a2a-demo:5000",
        target_revision="container-123",
        source="docker",
        namespace="docker-agents",
        updated_at=time.time(),
    )

    payload = record.to_redis_hash()

    assert payload["agent_id"] == "agent-a2a-demo"
    assert payload["upstream_url"] == "http://agent-a2a-demo:5000"
    assert "updated_at" in payload
```

- [ ] **Step 3: Run the tests to verify they fail**

Run:

```bash
python -m pytest agent-gateway/registry/tests/test_target_publisher.py -q
```

Expected:

```text
ModuleNotFoundError: No module named 'target_publisher'
```

- [ ] **Step 4: Implement target publisher**

Create `agent-gateway/registry/target_publisher.py` with this content:

```python
from __future__ import annotations

import logging
import time
from dataclasses import dataclass
from typing import Iterable

import redis

logger = logging.getLogger(__name__)

TARGET_INDEX_KEY = "request-manager:targets"
TARGET_KEY_PREFIX = "request-manager:targets:"


@dataclass(frozen=True)
class AgentTargetRecord:
    agent_id: str
    public_path: str
    upstream_url: str
    target_revision: str
    source: str
    namespace: str
    updated_at: float

    def to_redis_hash(self) -> dict[str, str]:
        return {
            "agent_id": self.agent_id,
            "public_path": self.public_path,
            "upstream_url": self.upstream_url,
            "target_revision": self.target_revision,
            "source": self.source,
            "namespace": self.namespace,
            "updated_at": str(self.updated_at),
        }


def build_target_record(
    agent_id: str,
    host: str,
    port: int,
    public_path: str,
    namespace: str,
    source: str,
    target_revision: str,
    now: float | None = None,
) -> AgentTargetRecord:
    return AgentTargetRecord(
        agent_id=agent_id,
        public_path=public_path,
        upstream_url=f"http://{host}:{port}",
        target_revision=target_revision,
        source=source,
        namespace=namespace,
        updated_at=time.time() if now is None else now,
    )


class RedisTargetPublisher:
    def __init__(self, redis_url: str, socket_timeout: float = 1.0) -> None:
        self.client = redis.Redis.from_url(
            redis_url,
            decode_responses=True,
            socket_timeout=socket_timeout,
            socket_connect_timeout=socket_timeout,
        )

    def publish(self, records: Iterable[AgentTargetRecord]) -> None:
        records = list(records)
        pipeline = self.client.pipeline()
        active_ids = {record.agent_id for record in records}

        for record in records:
            pipeline.hset(
                f"{TARGET_KEY_PREFIX}{record.agent_id}",
                mapping=record.to_redis_hash(),
            )
            pipeline.sadd(TARGET_INDEX_KEY, record.agent_id)

        existing_ids = self.client.smembers(TARGET_INDEX_KEY)
        stale_ids = set(existing_ids) - active_ids
        for stale_id in stale_ids:
            pipeline.delete(f"{TARGET_KEY_PREFIX}{stale_id}")
            pipeline.srem(TARGET_INDEX_KEY, stale_id)

        pipeline.execute()
        logger.info("Published %s request-manager target records", len(records))
```

- [ ] **Step 5: Run target publisher tests**

Run:

```bash
python -m pytest agent-gateway/registry/tests/test_target_publisher.py -q
```

Expected:

```text
2 passed
```

- [ ] **Step 6: Commit target publishing**

Run:

```bash
git add agent-gateway/registry/requirements.txt agent-gateway/registry/target_publisher.py agent-gateway/registry/tests/test_target_publisher.py
git commit -m "feat: publish agent targets for request manager"
```

Expected:

```text
Commit succeeds and prints the new commit hash.
```

## Task 3: Rewire Registry Dynamic Kong Routes To Request Manager

**Files:**

- Modify: `agent-gateway/registry/registry.py`

- [ ] **Step 1: Update imports and configuration**

In `agent-gateway/registry/registry.py`, add this import near the existing imports:

```python
from target_publisher import RedisTargetPublisher, build_target_record
```

Add these constants near the existing configuration constants:

```python
REDIS_URL = os.getenv("REDIS_URL", "redis://redis:6379")
REQUEST_MANAGER_SERVICE_NAME = os.getenv(
    "KONG_REQUEST_MANAGER_SERVICE_NAME",
    "agent-request-manager",
)
REQUEST_MANAGER_HOST = os.getenv("KONG_REQUEST_MANAGER_HOST", "nasiko-request-manager")
REQUEST_MANAGER_PORT = int(os.getenv("KONG_REQUEST_MANAGER_PORT", "8090"))
REQUEST_MANAGER_ROUTE_PREFIX = os.getenv("KONG_REQUEST_MANAGER_ROUTE_PREFIX", "/agents")
```

Add this global near `docker_client = None`:

```python
target_publisher = None
```

- [ ] **Step 2: Extend `ServiceInfo` with target revision and source**

Replace the existing `ServiceInfo` class with:

```python
class ServiceInfo(BaseModel):
    name: str
    host: str
    port: int
    path: str = "/"
    methods: List[str] = ["GET", "POST", "PUT", "DELETE", "PATCH"]
    namespace: str
    source: str
    target_revision: str
```

- [ ] **Step 3: Set Kubernetes target revisions**

In `get_k8s_services()`, update the `ServiceInfo constructor` call to include:

```python
source="kubernetes",
target_revision=svc.metadata.resource_version or svc.metadata.uid or service_name,
```

- [ ] **Step 4: Set Docker target revisions and skip Request Manager**

In the Docker infrastructure skip list, add:

```python
"nasiko-request-manager",
```

In `get_docker_services()`, update the `ServiceInfo constructor` call to include:

```python
source="docker",
target_revision=container.id,
```

- [ ] **Step 5: Add publisher accessor**

Add this function below `get_docker_client()`:

```python
def get_target_publisher() -> RedisTargetPublisher | None:
    """Initialize Redis publisher for Request Manager target records."""
    global target_publisher
    if target_publisher is None:
        try:
            target_publisher = RedisTargetPublisher(REDIS_URL)
            logger.info("Request Manager target publisher initialized")
        except Exception as e:
            logger.error(f"Failed to initialize target publisher: {e}")
            return None
    return target_publisher
```

- [ ] **Step 6: Add Request Manager Kong service registration**

Add this function above `register_service_in_kong()`:

```python
def ensure_request_manager_service() -> bool:
    """Ensure the shared Kong service for all dynamic agent routes exists."""
    service_data = {
        "name": REQUEST_MANAGER_SERVICE_NAME,
        "url": f"http://{REQUEST_MANAGER_HOST}:{REQUEST_MANAGER_PORT}",
        "connect_timeout": 60000,
        "write_timeout": 300000,
        "read_timeout": 300000,
        "retries": 0,
        "protocol": "http",
    }

    try:
        response = requests.get(f"{KONG_ADMIN_URL}/services/{REQUEST_MANAGER_SERVICE_NAME}", timeout=10)
        if response.status_code == 200:
            response = requests.patch(
                f"{KONG_ADMIN_URL}/services/{REQUEST_MANAGER_SERVICE_NAME}",
                json=service_data,
                timeout=10,
            )
            logger.info("Updated Request Manager Kong service")
        else:
            response = requests.post(f"{KONG_ADMIN_URL}/services", json=service_data, timeout=10)
            logger.info("Created Request Manager Kong service")
        return response.status_code in [200, 201]
    except Exception as e:
        logger.error(f"Failed to ensure Request Manager Kong service: {e}")
        return False
```

- [ ] **Step 7: Rewrite dynamic route registration**

Replace `register_service_in_kong(service: ServiceInfo)` with:

```python
def register_service_in_kong(service: ServiceInfo) -> bool:
    """Register an agent route in Kong that points to the Request Manager."""
    try:
        if not ensure_request_manager_service():
            return False

        route_data = {
            "name": f"{service.name}-route",
            "paths": [service.path],
            "methods": service.methods,
            "strip_path": False,
            "preserve_host": False,
            "service": {"name": REQUEST_MANAGER_SERVICE_NAME},
        }

        response = requests.get(f"{KONG_ADMIN_URL}/routes/{service.name}-route", timeout=10)
        if response.status_code == 200:
            response = requests.patch(
                f"{KONG_ADMIN_URL}/routes/{service.name}-route",
                json=route_data,
                timeout=10,
            )
            logger.info(f"Updated dynamic Request Manager route: {service.name}-route")
        else:
            response = requests.post(
                f"{KONG_ADMIN_URL}/services/{REQUEST_MANAGER_SERVICE_NAME}/routes",
                json=route_data,
                timeout=10,
            )
            logger.info(f"Created dynamic Request Manager route: {service.name}-route")

        if response.status_code not in [200, 201]:
            logger.error(f"Failed to register Request Manager route for {service.name}: {response.text}")
            return False

        logger.info(f"Registered {service.path} through Request Manager")
        return True
    except Exception as e:
        logger.error(f"Error registering Request Manager route for {service.name}: {e}")
        return False
```

- [ ] **Step 8: Preserve Request Manager service during cleanup**

In `cleanup_stale_services()`, add `REQUEST_MANAGER_SERVICE_NAME` to `static_proxy_services`:

```python
REQUEST_MANAGER_SERVICE_NAME,
```

Inside the same `for kong_service in kong_services:` loop, after static proxy services are skipped and before the `if service_name not in current_service_names:` check, add this block so old direct agent services are removed after their routes move to Request Manager:

```python
            if service_name.startswith("agent-") and service_name != REQUEST_MANAGER_SERVICE_NAME:
                try:
                    routes_response = requests.get(
                        f"{KONG_ADMIN_URL}/services/{service_name}/routes",
                        timeout=10,
                    )
                    if routes_response.status_code == 200:
                        for route in routes_response.json().get("data", []):
                            requests.delete(f"{KONG_ADMIN_URL}/routes/{route['id']}", timeout=10)
                    delete_response = requests.delete(
                        f"{KONG_ADMIN_URL}/services/{service_name}",
                        timeout=10,
                    )
                    if delete_response.status_code == 204:
                        logger.info(f"Removed legacy direct agent service: {service_name}")
                except Exception as e:
                    logger.error(f"Error removing legacy direct agent service {service_name}: {e}")
                continue
```

Then add this function below `cleanup_stale_services()`:

```python
def cleanup_stale_agent_routes(current_service_names: Set[str]) -> None:
    """Remove dynamic agent routes that no longer have discovered targets."""
    try:
        response = requests.get(f"{KONG_ADMIN_URL}/routes", timeout=10)
        if response.status_code != 200:
            logger.error(f"Failed to get Kong routes: {response.text}")
            return

        valid_route_names = {f"{name}-route" for name in current_service_names}
        for route in response.json().get("data", []):
            route_name = route.get("name", "")
            if not route_name.endswith("-route"):
                continue
            if not route_name.startswith("agent-"):
                continue
            if route_name in valid_route_names:
                continue
            delete_response = requests.delete(f"{KONG_ADMIN_URL}/routes/{route['id']}", timeout=10)
            if delete_response.status_code == 204:
                logger.info(f"Deleted stale agent route: {route_name}")
    except Exception as e:
        logger.error(f"Error cleaning up stale agent routes: {e}")
```

- [ ] **Step 9: Publish targets in the sync loop**

In `apply_middlewares_to_route()`, extend the CORS plugin `exposed_headers` list with:

```python
                    "X-Request-Layer-Agent",
                    "X-Request-Layer-Cache",
                    "X-Request-Layer-Queue-Wait-Ms",
                    "X-Request-Layer-Limit-State",
```

- [ ] **Step 10: Publish targets in the sync loop**

Inside `sync_services()`, after services are discovered and before dynamic route registration, add:

```python
publisher = get_target_publisher()
if publisher:
    target_records = [
        build_target_record(
            agent_id=service.name,
            host=service.host,
            port=service.port,
            public_path=service.path,
            namespace=service.namespace,
            source=service.source,
            target_revision=service.target_revision,
        )
        for service in services
    ]
    try:
        publisher.publish(target_records)
    except Exception as e:
        logger.error(f"Failed to publish Request Manager targets: {e}")
```

After `cleanup_stale_services(successful_registrations)`, add:

```python
cleanup_stale_agent_routes(successful_registrations)
```

- [ ] **Step 11: Update manual sync to publish targets and apply middleware**

In `trigger_sync()`, after discovery and before the registration loop, add:

```python
publisher = get_target_publisher()
if publisher:
    target_records = [
        build_target_record(
            agent_id=service.name,
            host=service.host,
            port=service.port,
            public_path=service.path,
            namespace=service.namespace,
            source=service.source,
            target_revision=service.target_revision,
        )
        for service in services
    ]
    try:
        publisher.publish(target_records)
    except Exception as e:
        logger.error(f"Failed to publish Request Manager targets: {e}")
```

Inside the registration loop, after a successful `register_service_in_kong(service)`, add:

```python
route_name = f"{service.name}-route"
apply_middlewares_to_route(route_name, ["cors", "nasiko-auth", "chat-logger"])
```

- [ ] **Step 12: Run registry tests**

Run:

```bash
python -m pytest agent-gateway/registry/tests/test_target_publisher.py -q
```

Expected:

```text
2 passed
```

- [ ] **Step 13: Commit registry route rewiring**

Run:

```bash
git add agent-gateway/registry/registry.py
git commit -m "feat: route agent traffic through request manager"
```

Expected:

```text
Commit succeeds and prints the new commit hash.
```

## Task 4: Implement Target Resolver And Runtime Limits

**Files:**

- Create: `agent-gateway/request-manager/request_manager/target_resolver.py`
- Modify: `agent-gateway/request-manager/request_manager/models.py`
- Create: `agent-gateway/request-manager/tests/test_target_resolver.py`

- [ ] **Step 1: Write target resolver tests**

Create `agent-gateway/request-manager/tests/test_target_resolver.py` with this content:

```python
import pytest

from request_manager.models import AgentTarget
from request_manager.target_resolver import TargetResolver


@pytest.mark.asyncio
async def test_resolves_agent_target_from_redis(fake_redis):
    await fake_redis.hset(
        "request-manager:targets:agent-a2a-demo",
        mapping={
            "agent_id": "agent-a2a-demo",
            "public_path": "/agents/agent-a2a-demo",
            "upstream_url": "http://agent-a2a-demo:5000",
            "target_revision": "rev-1",
            "source": "docker",
            "namespace": "docker-agents",
            "updated_at": "123.4",
        },
    )

    resolver = TargetResolver(fake_redis)
    target = await resolver.resolve("agent-a2a-demo")

    assert isinstance(target, AgentTarget)
    assert target.upstream_url == "http://agent-a2a-demo:5000"
    assert target.target_revision == "rev-1"


@pytest.mark.asyncio
async def test_returns_none_when_agent_target_is_missing(fake_redis):
    resolver = TargetResolver(fake_redis)

    assert await resolver.resolve("missing-agent") is None
```

- [ ] **Step 2: Run the tests to verify they fail**

Run:

```bash
cd agent-gateway/request-manager
python -m pytest tests/test_target_resolver.py -q
```

Expected:

```text
ModuleNotFoundError: No module named 'request_manager.target_resolver'
```

- [ ] **Step 3: Implement target resolver**

Create `agent-gateway/request-manager/request_manager/target_resolver.py` with this content:

```python
from __future__ import annotations

from request_manager import redis_keys
from request_manager.models import AgentLimits, AgentTarget
from request_manager.settings import RequestManagerSettings


class TargetResolver:
    def __init__(self, redis_client) -> None:
        self.redis = redis_client
        self.memory: dict[str, AgentTarget] = {}

    async def resolve(self, agent_id: str) -> AgentTarget | None:
        try:
            payload = await self.redis.hgetall(redis_keys.target(agent_id))
        except Exception:
            return self.memory.get(agent_id)
        if not payload:
            return self.memory.get(agent_id)
        target = AgentTarget(
            agent_id=payload["agent_id"],
            public_path=payload["public_path"],
            upstream_url=payload["upstream_url"].rstrip("/"),
            target_revision=payload["target_revision"],
            source=payload["source"],
            namespace=payload["namespace"],
            updated_at=float(payload["updated_at"]),
        )
        self.memory[agent_id] = target
        return target


class LimitResolver:
    def __init__(self, redis_client, settings: RequestManagerSettings) -> None:
        self.redis = redis_client
        self.settings = settings

    async def resolve(self, agent_id: str) -> AgentLimits:
        defaults = AgentLimits(
            cache_ttl_seconds=self.settings.cache_ttl_seconds,
            max_concurrency=self.settings.max_concurrency_per_agent,
            sustained_rps=self.settings.sustained_rps_per_agent,
            burst_capacity=self.settings.burst_capacity_per_agent,
            max_queue_depth=self.settings.max_queue_depth_per_agent,
            max_queue_wait_ms=self.settings.max_queue_wait_ms,
            cache_enabled=True,
        )
        try:
            override = await self.redis.hgetall(redis_keys.limits(agent_id))
        except Exception:
            return defaults
        if not override:
            return defaults

        data = defaults.model_dump()
        for field in data:
            if field not in override:
                continue
            if isinstance(data[field], bool):
                data[field] = override[field].lower() in {"1", "true", "yes", "on"}
            elif isinstance(data[field], int):
                data[field] = int(float(override[field]))
            elif isinstance(data[field], float):
                data[field] = float(override[field])
            else:
                data[field] = override[field]
        return AgentLimits(**data)

    async def update(self, agent_id: str, limits: AgentLimits) -> AgentLimits:
        await self.redis.hset(redis_keys.limits(agent_id), mapping=limits.model_dump())
        return limits
```

- [ ] **Step 4: Add limit resolver tests**

Append this content to `agent-gateway/request-manager/tests/test_target_resolver.py`:

```python
from request_manager.settings import RequestManagerSettings
from request_manager.target_resolver import LimitResolver


@pytest.mark.asyncio
async def test_limit_resolver_merges_redis_overrides(fake_redis):
    await fake_redis.hset(
        "request-manager:limits:agent-a2a-demo",
        mapping={"max_concurrency": "7", "cache_enabled": "false"},
    )
    settings = RequestManagerSettings(redis_url="redis://redis:6379")
    resolver = LimitResolver(fake_redis, settings)

    limits = await resolver.resolve("agent-a2a-demo")

    assert limits.max_concurrency == 7
    assert limits.cache_enabled is False
    assert limits.cache_ttl_seconds == 600
```

- [ ] **Step 5: Run target resolver tests**

Run:

```bash
cd agent-gateway/request-manager
python -m pytest tests/test_target_resolver.py -q
```

Expected:

```text
3 passed
```

- [ ] **Step 6: Commit resolver work**

Run:

```bash
git add agent-gateway/request-manager/request_manager/target_resolver.py agent-gateway/request-manager/tests/test_target_resolver.py
git commit -m "feat: resolve agent targets and limits"
```

Expected:

```text
Commit succeeds and prints the new commit hash.
```

## Task 5: Implement Cache Policy, Key Builder, And Cache Store

**Files:**

- Create: `agent-gateway/request-manager/request_manager/cache.py`
- Create: `agent-gateway/request-manager/tests/test_cache_policy.py`

- [ ] **Step 1: Write cache policy tests**

Create `agent-gateway/request-manager/tests/test_cache_policy.py` with this content:

```python
import json

import pytest

from request_manager.cache import CachePolicy, RedisResponseCache, normalize_text
from request_manager.models import AgentLimits, AgentTarget, CachedResponse


def target() -> AgentTarget:
    return AgentTarget(
        agent_id="agent-a2a-demo",
        public_path="/agents/agent-a2a-demo",
        upstream_url="http://agent-a2a-demo:5000",
        target_revision="rev-1",
        source="docker",
        namespace="docker-agents",
        updated_at=123.4,
    )


def limits(cache_enabled: bool = True) -> AgentLimits:
    return AgentLimits(
        cache_ttl_seconds=600,
        max_concurrency=2,
        sustained_rps=5,
        burst_capacity=10,
        max_queue_depth=20,
        max_queue_wait_ms=10000,
        cache_enabled=cache_enabled,
    )


def test_normalize_text_is_conservative():
    assert normalize_text("  Hello   WORLD  ") == "hello world"


def test_cacheable_message_send_builds_stable_key():
    body = {
        "jsonrpc": "2.0",
        "id": "abc",
        "method": "message/send",
        "params": {"message": {"parts": [{"kind": "text", "text": " Hello world "}]}}
    }

    decision = CachePolicy().decide(
        agent_id="agent-a2a-demo",
        headers={"x-subject-id": "user-1"},
        json_body=body,
        target=target(),
        limits=limits(),
    )

    assert decision.cacheable is True
    assert decision.cache_key is not None
    assert decision.fingerprint["scope"] == "user-1"
    assert decision.fingerprint["target_revision"] == "rev-1"
    assert decision.fingerprint["texts"] == ["hello world"]


def test_cache_key_ignores_jsonrpc_id():
    body_a = {
        "jsonrpc": "2.0",
        "id": "abc",
        "method": "message/send",
        "params": {"message": {"parts": [{"kind": "text", "text": "Hello"}]}}
    }
    body_b = {
        "jsonrpc": "2.0",
        "id": "xyz",
        "method": "message/send",
        "params": {"message": {"parts": [{"kind": "text", "text": "hello"}]}}
    }
    policy = CachePolicy()

    key_a = policy.decide("agent-a2a-demo", {}, body_a, target(), limits()).cache_key
    key_b = policy.decide("agent-a2a-demo", {}, body_b, target(), limits()).cache_key

    assert key_a == key_b


def test_cache_bypasses_no_cache_header():
    body = {
        "jsonrpc": "2.0",
        "method": "message/send",
        "params": {"message": {"parts": [{"kind": "text", "text": "Hello"}]}}
    }

    decision = CachePolicy().decide(
        agent_id="agent-a2a-demo",
        headers={"cache-control": "no-cache"},
        json_body=body,
        target=target(),
        limits=limits(),
    )

    assert decision.cacheable is False
    assert decision.reason == "cache-control-no-cache"


def test_cache_bypasses_non_text_parts():
    body = {
        "jsonrpc": "2.0",
        "method": "message/send",
        "params": {"message": {"parts": [{"kind": "file", "text": "x"}]}}
    }

    decision = CachePolicy().decide("agent-a2a-demo", {}, body, target(), limits())

    assert decision.cacheable is False
    assert decision.reason == "non-text-part"


@pytest.mark.asyncio
async def test_redis_response_cache_round_trips_bytes(fake_redis):
    cache = RedisResponseCache(fake_redis)
    response = CachedResponse(status_code=200, body=b'{"ok": true}', headers={"content-type": "application/json"})

    await cache.set("abc", response, ttl_seconds=600)
    cached = await cache.get("abc")

    assert cached.status_code == 200
    assert cached.body == b'{"ok": true}'
    assert cached.headers["content-type"] == "application/json"
```

- [ ] **Step 2: Run the tests to verify they fail**

Run:

```bash
cd agent-gateway/request-manager
python -m pytest tests/test_cache_policy.py -q
```

Expected:

```text
ModuleNotFoundError: No module named 'request_manager.cache'
```

- [ ] **Step 3: Implement cache module**

Create `agent-gateway/request-manager/request_manager/cache.py` with this content:

```python
from __future__ import annotations

import base64
import hashlib
import json
import re
from typing import Any

from request_manager import redis_keys
from request_manager.models import AgentLimits, AgentTarget, CacheDecision, CachedResponse


def normalize_text(text: str) -> str:
    return re.sub(r"\s+", " ", text.strip().lower())


class CachePolicy:
    def decide(
        self,
        agent_id: str,
        headers: dict[str, str],
        json_body: dict[str, Any],
        target: AgentTarget,
        limits: AgentLimits,
    ) -> CacheDecision:
        normalized_headers = {key.lower(): value for key, value in headers.items()}
        if not limits.cache_enabled:
            return CacheDecision(cacheable=False, reason="agent-cache-disabled")
        if "no-cache" in normalized_headers.get("cache-control", "").lower():
            return CacheDecision(cacheable=False, reason="cache-control-no-cache")
        if json_body.get("method") != "message/send":
            return CacheDecision(cacheable=False, reason="unsupported-method")

        parts = (
            json_body.get("params", {})
            .get("message", {})
            .get("parts", [])
        )
        if not parts:
            return CacheDecision(cacheable=False, reason="missing-message-parts")

        texts: list[str] = []
        for part in parts:
            if part.get("kind") != "text":
                return CacheDecision(cacheable=False, reason="non-text-part")
            text = part.get("text")
            if not isinstance(text, str):
                return CacheDecision(cacheable=False, reason="missing-text")
            texts.append(normalize_text(text))

        scope = normalized_headers.get("x-subject-id") or "anonymous"
        fingerprint = {
            "agent_id": agent_id,
            "method": "message/send",
            "texts": texts,
            "scope": scope,
            "target_revision": target.target_revision,
        }
        encoded = json.dumps(fingerprint, sort_keys=True, separators=(",", ":")).encode("utf-8")
        cache_key = hashlib.sha256(encoded).hexdigest()
        return CacheDecision(
            cacheable=True,
            reason="cacheable",
            cache_key=cache_key,
            fingerprint=fingerprint,
        )


class RedisResponseCache:
    def __init__(self, redis_client) -> None:
        self.redis = redis_client

    async def get(self, cache_key: str) -> CachedResponse | None:
        try:
            raw = await self.redis.get(redis_keys.cache_entry(cache_key))
        except Exception:
            return None
        if not raw:
            return None
        payload = json.loads(raw)
        return CachedResponse(
            status_code=int(payload["status_code"]),
            media_type=payload.get("media_type"),
            body=base64.b64decode(payload["body_b64"].encode("ascii")),
            headers=payload.get("headers", {}),
        )

    async def set(self, cache_key: str, response: CachedResponse, ttl_seconds: int) -> None:
        payload = {
            "status_code": response.status_code,
            "media_type": response.media_type,
            "headers": response.headers,
            "body_b64": base64.b64encode(response.body).decode("ascii"),
        }
        try:
            await self.redis.set(
                redis_keys.cache_entry(cache_key),
                json.dumps(payload, sort_keys=True),
                ex=ttl_seconds,
            )
        except Exception:
            return

    async def clear(self, agent_id: str | None = None) -> int:
        if agent_id is not None:
            return 0
        if hasattr(self.redis, "scan_iter"):
            keys = []
            async for key in self.redis.scan_iter(match="request-manager:cache:*"):
                keys.append(key)
            if not keys:
                return 0
            return await self.redis.delete(*keys)
        keys = [
            key for key in list(getattr(self.redis, "values", {}).keys())
            if key.startswith("request-manager:cache:")
        ]
        if not keys:
            return 0
        return await self.redis.delete(*keys)
```

- [ ] **Step 4: Run cache tests**

Run:

```bash
cd agent-gateway/request-manager
python -m pytest tests/test_cache_policy.py -q
```

Expected:

```text
6 passed
```

- [ ] **Step 5: Commit cache policy**

Run:

```bash
git add agent-gateway/request-manager/request_manager/cache.py agent-gateway/request-manager/tests/test_cache_policy.py
git commit -m "feat: add safe agent response cache policy"
```

Expected:

```text
Commit succeeds and prints the new commit hash.
```

## Task 6: Implement Single-Flight Dedupe

**Files:**

- Create: `agent-gateway/request-manager/request_manager/singleflight.py`
- Create: `agent-gateway/request-manager/tests/test_singleflight.py`

- [ ] **Step 1: Write single-flight tests**

Create `agent-gateway/request-manager/tests/test_singleflight.py` with this content:

```python
import pytest

from request_manager.singleflight import SingleFlight


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
```

- [ ] **Step 2: Run tests to verify they fail**

Run:

```bash
cd agent-gateway/request-manager
python -m pytest tests/test_singleflight.py -q
```

Expected:

```text
ModuleNotFoundError: No module named 'request_manager.singleflight'
```

- [ ] **Step 3: Implement single-flight**

Create `agent-gateway/request-manager/request_manager/singleflight.py` with this content:

```python
from __future__ import annotations

import asyncio
import time
import uuid
from dataclasses import dataclass

from request_manager import redis_keys


@dataclass(frozen=True)
class SingleFlightClaim:
    cache_key: str
    token: str
    owner: bool


class SingleFlight:
    def __init__(self, redis_client, wait_ms: int) -> None:
        self.redis = redis_client
        self.wait_ms = wait_ms

    async def claim(self, cache_key: str) -> SingleFlightClaim:
        token = str(uuid.uuid4())
        try:
            acquired = await self.redis.set(
                redis_keys.singleflight_lock(cache_key),
                token,
                px=max(self.wait_ms, 1000),
                nx=True,
            )
        except Exception:
            acquired = True
        return SingleFlightClaim(cache_key=cache_key, token=token, owner=bool(acquired))

    async def wait_until_ready(self, cache_key: str) -> bool:
        deadline = time.monotonic() + (self.wait_ms / 1000)
        while time.monotonic() < deadline:
            try:
                if await self.redis.get(redis_keys.singleflight_ready(cache_key)):
                    return True
                if not await self.redis.get(redis_keys.singleflight_lock(cache_key)):
                    return True
            except Exception:
                return True
            await asyncio.sleep(0.05)
        return False

    async def release(self, claim: SingleFlightClaim) -> None:
        if not claim.owner:
            return
        try:
            await self.redis.set(redis_keys.singleflight_ready(claim.cache_key), "1", px=1000)
            await self.redis.delete(redis_keys.singleflight_lock(claim.cache_key))
        except Exception:
            return
```

- [ ] **Step 4: Run single-flight tests**

Run:

```bash
cd agent-gateway/request-manager
python -m pytest tests/test_singleflight.py -q
```

Expected:

```text
3 passed
```

- [ ] **Step 5: Commit single-flight**

Run:

```bash
git add agent-gateway/request-manager/request_manager/singleflight.py agent-gateway/request-manager/tests/test_singleflight.py
git commit -m "feat: dedupe concurrent cache misses"
```

Expected:

```text
Commit succeeds and prints the new commit hash.
```

## Task 7: Implement Rate Limiter And Bounded FIFO Queue

**Files:**

- Create: `agent-gateway/request-manager/request_manager/limiter.py`
- Create: `agent-gateway/request-manager/tests/test_limiter.py`

- [ ] **Step 1: Write limiter tests**

Create `agent-gateway/request-manager/tests/test_limiter.py` with this content:

```python
import pytest

from request_manager.limiter import RequestLimiter
from request_manager.models import AgentLimits


def limits(max_concurrency: int = 1, max_queue_depth: int = 1, max_queue_wait_ms: int = 50) -> AgentLimits:
    return AgentLimits(
        cache_ttl_seconds=600,
        max_concurrency=max_concurrency,
        sustained_rps=100,
        burst_capacity=100,
        max_queue_depth=max_queue_depth,
        max_queue_wait_ms=max_queue_wait_ms,
        cache_enabled=True,
    )


@pytest.mark.asyncio
async def test_acquires_when_capacity_available(fake_redis):
    limiter = RequestLimiter(fake_redis, global_active_cap=50)

    result = await limiter.acquire("agent-a2a-demo", limits(), request_id="req-1")

    assert result.acquired is True
    assert result.queued is False


@pytest.mark.asyncio
async def test_releases_capacity(fake_redis):
    limiter = RequestLimiter(fake_redis, global_active_cap=50)
    await limiter.acquire("agent-a2a-demo", limits(), request_id="req-1")

    await limiter.release("agent-a2a-demo")

    assert int(await fake_redis.get("request-manager:active:agent-a2a-demo")) == 0
    assert int(await fake_redis.get("request-manager:active:global")) == 0


@pytest.mark.asyncio
async def test_returns_overflow_when_queue_full(fake_redis):
    limiter = RequestLimiter(fake_redis, global_active_cap=50)
    await limiter.acquire("agent-a2a-demo", limits(), request_id="req-1")
    await fake_redis.rpush("request-manager:queue:agent-a2a-demo", "already-queued")

    result = await limiter.acquire("agent-a2a-demo", limits(), request_id="req-2")

    assert result.acquired is False
    assert result.reason == "queue-full"
```

- [ ] **Step 2: Run tests to verify they fail**

Run:

```bash
cd agent-gateway/request-manager
python -m pytest tests/test_limiter.py -q
```

Expected:

```text
ModuleNotFoundError: No module named 'request_manager.limiter'
```

- [ ] **Step 3: Implement limiter**

Create `agent-gateway/request-manager/request_manager/limiter.py` with this content:

```python
from __future__ import annotations

import asyncio
import time

from request_manager import redis_keys
from request_manager.models import AcquireResult, AgentLimits


class LocalFallbackLimiter:
    def __init__(self, global_active_cap: int) -> None:
        self.global_semaphore = asyncio.Semaphore(global_active_cap)
        self.agent_semaphores: dict[str, asyncio.Semaphore] = {}

    def _agent_semaphore(self, agent_id: str, limits: AgentLimits) -> asyncio.Semaphore:
        if agent_id not in self.agent_semaphores:
            self.agent_semaphores[agent_id] = asyncio.Semaphore(max(1, min(limits.max_concurrency, 1)))
        return self.agent_semaphores[agent_id]

    async def acquire(self, agent_id: str, limits: AgentLimits, request_id: str) -> AcquireResult:
        agent_semaphore = self._agent_semaphore(agent_id, limits)
        start = time.monotonic()
        timeout = limits.max_queue_wait_ms / 1000
        global_acquired = False
        agent_acquired = False
        try:
            await asyncio.wait_for(self.global_semaphore.acquire(), timeout=timeout)
            global_acquired = True
            await asyncio.wait_for(agent_semaphore.acquire(), timeout=timeout)
            agent_acquired = True
            wait_ms = int((time.monotonic() - start) * 1000)
            return AcquireResult(acquired=True, queued=wait_ms > 0, queue_wait_ms=wait_ms, degraded=True)
        except asyncio.TimeoutError:
            if agent_acquired:
                agent_semaphore.release()
            if global_acquired:
                self.global_semaphore.release()
            return AcquireResult(acquired=False, queued=True, reason="degraded-local-timeout", degraded=True)

    async def release(self, agent_id: str) -> None:
        agent_semaphore = self.agent_semaphores.get(agent_id)
        if agent_semaphore is not None:
            agent_semaphore.release()
        self.global_semaphore.release()


class RequestLimiter:
    def __init__(self, redis_client, global_active_cap: int) -> None:
        self.redis = redis_client
        self.global_active_cap = global_active_cap
        self.local = LocalFallbackLimiter(global_active_cap)

    async def acquire(self, agent_id: str, limits: AgentLimits, request_id: str) -> AcquireResult:
        try:
            return await self._distributed_acquire(agent_id, limits, request_id)
        except Exception:
            return await self.local.acquire(agent_id, limits, request_id)

    async def _distributed_acquire(self, agent_id: str, limits: AgentLimits, request_id: str) -> AcquireResult:
        if await self._try_acquire_capacity(agent_id, limits):
            return AcquireResult(acquired=True)

        queue_length = await self.redis.llen(redis_keys.queue(agent_id))
        if queue_length >= limits.max_queue_depth:
            return AcquireResult(acquired=False, reason="queue-full", retry_after_seconds=1)

        await self.redis.rpush(redis_keys.queue(agent_id), request_id)
        start = time.monotonic()
        deadline = start + (limits.max_queue_wait_ms / 1000)

        try:
            while time.monotonic() < deadline:
                head = await self.redis.lindex(redis_keys.queue(agent_id), 0)
                if head == request_id and await self._try_acquire_capacity(agent_id, limits):
                    await self.redis.lpop(redis_keys.queue(agent_id))
                    wait_ms = int((time.monotonic() - start) * 1000)
                    return AcquireResult(acquired=True, queued=True, queue_wait_ms=wait_ms)
                await asyncio.sleep(0.05)
        finally:
            await self.redis.lrem(redis_keys.queue(agent_id), 0, request_id)

        return AcquireResult(acquired=False, queued=True, reason="queue-timeout", retry_after_seconds=1)

    async def release(self, agent_id: str, degraded: bool = False) -> None:
        if degraded:
            await self.local.release(agent_id)
            return
        await self._safe_decr(redis_keys.active(agent_id))
        await self._safe_decr(redis_keys.active_global())

    async def _try_acquire_capacity(self, agent_id: str, limits: AgentLimits) -> bool:
        agent_active = int(await self.redis.get(redis_keys.active(agent_id)) or "0")
        global_active = int(await self.redis.get(redis_keys.active_global()) or "0")
        if agent_active >= limits.max_concurrency:
            return False
        if global_active >= self.global_active_cap:
            return False
        if not await self._consume_token(agent_id, limits):
            return False
        await self.redis.incr(redis_keys.active(agent_id))
        await self.redis.incr(redis_keys.active_global())
        return True

    async def _consume_token(self, agent_id: str, limits: AgentLimits) -> bool:
        key = redis_keys.bucket(agent_id)
        bucket = await self.redis.hgetall(key)
        now = time.monotonic()
        tokens = float(bucket.get("tokens", limits.burst_capacity))
        updated_at = float(bucket.get("updated_at", now))
        elapsed = max(now - updated_at, 0)
        tokens = min(limits.burst_capacity, tokens + elapsed * limits.sustained_rps)
        if tokens < 1:
            await self.redis.hset(key, mapping={"tokens": tokens, "updated_at": now})
            await self.redis.expire(key, 3600)
            return False
        await self.redis.hset(key, mapping={"tokens": tokens - 1, "updated_at": now})
        await self.redis.expire(key, 3600)
        return True

    async def _safe_decr(self, key: str) -> None:
        current = int(await self.redis.get(key) or "0")
        if current <= 0:
            await self.redis.set(key, "0")
            return
        await self.redis.decr(key)
```

- [ ] **Step 4: Run limiter tests**

Run:

```bash
cd agent-gateway/request-manager
python -m pytest tests/test_limiter.py -q
```

Expected:

```text
3 passed
```

- [ ] **Step 5: Commit limiter work**

Run:

```bash
git add agent-gateway/request-manager/request_manager/limiter.py agent-gateway/request-manager/tests/test_limiter.py
git commit -m "feat: add per-agent limiter and queue"
```

Expected:

```text
Commit succeeds and prints the new commit hash.
```

## Task 8: Implement Circuit Breaker

**Files:**

- Create: `agent-gateway/request-manager/request_manager/circuit_breaker.py`
- Create: `agent-gateway/request-manager/tests/test_circuit_breaker.py`

- [ ] **Step 1: Write circuit breaker tests**

Create `agent-gateway/request-manager/tests/test_circuit_breaker.py` with this content:

```python
import pytest

from request_manager.circuit_breaker import CircuitBreaker


@pytest.mark.asyncio
async def test_allows_when_closed(fake_redis):
    breaker = CircuitBreaker(fake_redis, window_size=20, min_failures=5, failure_ratio=0.5, open_seconds=30)

    decision = await breaker.before_request("agent-a2a-demo")

    assert decision.allowed is True
    assert decision.state == "closed"


@pytest.mark.asyncio
async def test_opens_after_failure_threshold(fake_redis):
    breaker = CircuitBreaker(fake_redis, window_size=20, min_failures=5, failure_ratio=0.5, open_seconds=30)

    for _ in range(5):
        await breaker.record_result("agent-a2a-demo", success=False)
    decision = await breaker.before_request("agent-a2a-demo")

    assert decision.allowed is False
    assert decision.state == "open"
    assert decision.retry_after_seconds > 0


@pytest.mark.asyncio
async def test_success_closes_half_open_circuit(fake_redis):
    breaker = CircuitBreaker(fake_redis, window_size=20, min_failures=5, failure_ratio=0.5, open_seconds=0)
    for _ in range(5):
        await breaker.record_result("agent-a2a-demo", success=False)
    half_open = await breaker.before_request("agent-a2a-demo")

    await breaker.record_result("agent-a2a-demo", success=True)
    closed = await breaker.before_request("agent-a2a-demo")

    assert half_open.allowed is True
    assert half_open.state == "half-open"
    assert closed.allowed is True
```

- [ ] **Step 2: Run tests to verify they fail**

Run:

```bash
cd agent-gateway/request-manager
python -m pytest tests/test_circuit_breaker.py -q
```

Expected:

```text
ModuleNotFoundError: No module named 'request_manager.circuit_breaker'
```

- [ ] **Step 3: Implement circuit breaker**

Create `agent-gateway/request-manager/request_manager/circuit_breaker.py` with this content:

```python
from __future__ import annotations

import time

from request_manager import redis_keys
from request_manager.models import CircuitDecision


class CircuitBreaker:
    def __init__(
        self,
        redis_client,
        window_size: int,
        min_failures: int,
        failure_ratio: float,
        open_seconds: int,
    ) -> None:
        self.redis = redis_client
        self.window_size = window_size
        self.min_failures = min_failures
        self.failure_ratio = failure_ratio
        self.open_seconds = open_seconds

    async def before_request(self, agent_id: str) -> CircuitDecision:
        try:
            state = await self.redis.hgetall(redis_keys.circuit(agent_id))
        except Exception:
            return CircuitDecision(allowed=True, state="degraded")
        open_until = float(state.get("open_until", "0") or "0")
        now = time.time()
        if open_until > now:
            return CircuitDecision(
                allowed=False,
                state="open",
                retry_after_seconds=max(1, int(open_until - now)),
            )
        if state.get("state") == "open":
            await self.redis.hset(redis_keys.circuit(agent_id), mapping={"state": "half-open", "open_until": "0"})
            return CircuitDecision(allowed=True, state="half-open")
        return CircuitDecision(allowed=True, state=state.get("state", "closed"))

    async def record_result(self, agent_id: str, success: bool) -> None:
        try:
            await self._record_result(agent_id, success)
        except Exception:
            return

    async def _record_result(self, agent_id: str, success: bool) -> None:
        if success:
            state = await self.redis.hgetall(redis_keys.circuit(agent_id))
            if state.get("state") == "half-open":
                await self.redis.hset(redis_keys.circuit(agent_id), mapping={"state": "closed", "open_until": "0"})
            await self._append_outcome(agent_id, "1")
            return

        await self._append_outcome(agent_id, "0")
        outcomes = list(getattr(self.redis, "lists", {}).get(redis_keys.outcomes(agent_id), []))
        failures = outcomes.count("0")
        if len(outcomes) >= self.min_failures and failures >= self.min_failures:
            ratio = failures / len(outcomes)
            if ratio >= self.failure_ratio:
                await self.redis.hset(
                    redis_keys.circuit(agent_id),
                    mapping={
                        "state": "open",
                        "open_until": str(time.time() + self.open_seconds),
                    },
                )

    async def _append_outcome(self, agent_id: str, outcome: str) -> None:
        key = redis_keys.outcomes(agent_id)
        await self.redis.rpush(key, outcome)
        values = getattr(self.redis, "lists", {}).get(key)
        while values is not None and len(values) > self.window_size:
            values.popleft()
```

- [ ] **Step 4: Run circuit breaker tests**

Run:

```bash
cd agent-gateway/request-manager
python -m pytest tests/test_circuit_breaker.py -q
```

Expected:

```text
3 passed
```

- [ ] **Step 5: Commit circuit breaker**

Run:

```bash
git add agent-gateway/request-manager/request_manager/circuit_breaker.py agent-gateway/request-manager/tests/test_circuit_breaker.py
git commit -m "feat: protect agents with circuit breaker"
```

Expected:

```text
Commit succeeds and prints the new commit hash.
```

## Task 9: Implement Metrics

**Files:**

- Create: `agent-gateway/request-manager/request_manager/metrics.py`

- [ ] **Step 1: Create metrics recorder**

Create `agent-gateway/request-manager/request_manager/metrics.py` with this content:

```python
from __future__ import annotations

from statistics import median

from request_manager import redis_keys
from request_manager.models import AgentLimits, AgentStats, GlobalStats


class MetricsRecorder:
    def __init__(self, redis_client) -> None:
        self.redis = redis_client

    async def increment(self, agent_id: str, field: str, amount: int = 1) -> None:
        try:
            await self.redis.hincrby(redis_keys.metrics_agent(agent_id), field, amount)
            await self.redis.hincrby(redis_keys.metrics_global(), field, amount)
        except Exception:
            return

    async def record_latency(self, agent_id: str, latency_ms: int) -> None:
        key = redis_keys.latency(agent_id)
        try:
            await self.redis.rpush(key, str(latency_ms))
            values = getattr(self.redis, "lists", {}).get(key)
            while values is not None and len(values) > 200:
                values.popleft()
        except Exception:
            return

    async def record_queue_wait(self, agent_id: str, wait_ms: int) -> None:
        key = redis_keys.queue_wait(agent_id)
        try:
            await self.redis.rpush(key, str(wait_ms))
            values = getattr(self.redis, "lists", {}).get(key)
            while values is not None and len(values) > 200:
                values.popleft()
        except Exception:
            return

    async def agent_stats(self, agent_id: str, limits: AgentLimits, circuit_state: str) -> AgentStats:
        try:
            metrics = await self.redis.hgetall(redis_keys.metrics_agent(agent_id))
            active_requests = int(await self.redis.get(redis_keys.active(agent_id)) or "0")
            queued_requests = await self.redis.llen(redis_keys.queue(agent_id))
            latency_samples = [float(value) for value in getattr(self.redis, "lists", {}).get(redis_keys.latency(agent_id), [])]
            queue_samples = [float(value) for value in getattr(self.redis, "lists", {}).get(redis_keys.queue_wait(agent_id), [])]
        except Exception:
            metrics = {}
            active_requests = 0
            queued_requests = 0
            latency_samples = []
            queue_samples = []
        return AgentStats(
            agent_id=agent_id,
            active_requests=active_requests,
            queued_requests=queued_requests,
            cache_hits=int(metrics.get("cache_hits", "0")),
            cache_misses=int(metrics.get("cache_misses", "0")),
            cache_bypasses=int(metrics.get("cache_bypasses", "0")),
            singleflight_waiters=int(metrics.get("singleflight_waiters", "0")),
            upstream_requests=int(metrics.get("upstream_requests", "0")),
            upstream_errors=int(metrics.get("upstream_errors", "0")),
            queue_timeouts=int(metrics.get("queue_timeouts", "0")),
            circuit_state=circuit_state,
            p50_latency_ms=percentile(latency_samples, 50),
            p95_latency_ms=percentile(latency_samples, 95),
            p95_queue_wait_ms=percentile(queue_samples, 95),
            limits=limits,
        )

    async def global_stats(self, redis_available: bool, agents: list[AgentStats]) -> GlobalStats:
        try:
            metrics = await self.redis.hgetall(redis_keys.metrics_global())
            active_requests = int(await self.redis.get(redis_keys.active_global()) or "0")
        except Exception:
            metrics = {}
            active_requests = 0
        return GlobalStats(
            status="healthy" if redis_available else "degraded",
            redis_available=redis_available,
            active_requests=active_requests,
            cache_hits=int(metrics.get("cache_hits", "0")),
            cache_misses=int(metrics.get("cache_misses", "0")),
            cache_bypasses=int(metrics.get("cache_bypasses", "0")),
            upstream_requests=int(metrics.get("upstream_requests", "0")),
            upstream_errors=int(metrics.get("upstream_errors", "0")),
            queue_timeouts=int(metrics.get("queue_timeouts", "0")),
            agents=agents,
        )


def percentile(values: list[float], pct: int) -> float:
    if not values:
        return 0.0
    if pct == 50:
        return float(median(values))
    ordered = sorted(values)
    index = int(round((pct / 100) * (len(ordered) - 1)))
    return float(ordered[index])
```

- [ ] **Step 2: Run full Request Manager unit suite**

Run:

```bash
cd agent-gateway/request-manager
python -m pytest tests -q
```

Expected:

```text
21 passed
```

- [ ] **Step 3: Commit metrics**

Run:

```bash
git add agent-gateway/request-manager/request_manager/metrics.py
git commit -m "feat: record request layer metrics"
```

Expected:

```text
Commit succeeds and prints the new commit hash.
```

## Task 10: Implement Proxy Orchestration

**Files:**

- Create: `agent-gateway/request-manager/request_manager/proxy.py`
- Create: `agent-gateway/request-manager/tests/test_proxy_flow.py`
- Modify: `agent-gateway/request-manager/request_manager/main.py`

- [ ] **Step 1: Write proxy flow tests**

Create `agent-gateway/request-manager/tests/test_proxy_flow.py` with this content:

```python
import json

import pytest
from fastapi import Request

from request_manager.models import AgentTarget
from request_manager.proxy import build_upstream_url, copy_response_headers, extract_agent_id


def test_extract_agent_id_from_agents_path():
    assert extract_agent_id("/agents/agent-a2a-demo") == ("agent-a2a-demo", "")
    assert extract_agent_id("/agents/agent-a2a-demo/message") == ("agent-a2a-demo", "/message")


def test_build_upstream_url_strips_public_agents_prefix():
    target = AgentTarget(
        agent_id="agent-a2a-demo",
        public_path="/agents/agent-a2a-demo",
        upstream_url="http://agent-a2a-demo:5000",
        target_revision="rev-1",
        source="docker",
        namespace="docker-agents",
        updated_at=123.4,
    )

    assert build_upstream_url(target, "") == "http://agent-a2a-demo:5000/"
    assert build_upstream_url(target, "/message") == "http://agent-a2a-demo:5000/message"


def test_copy_response_headers_removes_hop_by_hop_headers():
    copied = copy_response_headers({
        "content-type": "application/json",
        "connection": "keep-alive",
        "transfer-encoding": "chunked",
    })

    assert copied == {"content-type": "application/json"}
```

- [ ] **Step 2: Run tests to verify they fail**

Run:

```bash
cd agent-gateway/request-manager
python -m pytest tests/test_proxy_flow.py -q
```

Expected:

```text
ModuleNotFoundError: No module named 'request_manager.proxy'
```

- [ ] **Step 3: Implement proxy helpers and handler**

Create `agent-gateway/request-manager/request_manager/proxy.py` with this content:

```python
from __future__ import annotations

import time
import uuid
from typing import Any

import httpx
from fastapi import Request
from fastapi.responses import JSONResponse, Response

from request_manager.cache import CachePolicy, RedisResponseCache
from request_manager.circuit_breaker import CircuitBreaker
from request_manager.limiter import RequestLimiter
from request_manager.metrics import MetricsRecorder
from request_manager.models import CachedResponse, CacheState, LimitState
from request_manager.singleflight import SingleFlight
from request_manager.target_resolver import LimitResolver, TargetResolver

HOP_BY_HOP_HEADERS = {
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailers",
    "transfer-encoding",
    "upgrade",
    "host",
    "content-length",
}


def extract_agent_id(path: str) -> tuple[str, str]:
    if not path.startswith("/agents/"):
        raise ValueError("path must start with /agents/")
    remainder = path[len("/agents/"):]
    if "/" not in remainder:
        return remainder, ""
    agent_id, subpath = remainder.split("/", 1)
    return agent_id, f"/{subpath}"


def build_upstream_url(target, subpath: str) -> str:
    if not subpath:
        subpath = "/"
    return f"{target.upstream_url}{subpath}"


def copy_request_headers(headers: dict[str, str]) -> dict[str, str]:
    return {
        key: value
        for key, value in headers.items()
        if key.lower() not in HOP_BY_HOP_HEADERS
    }


def copy_response_headers(headers: dict[str, str]) -> dict[str, str]:
    return {
        key: value
        for key, value in headers.items()
        if key.lower() not in HOP_BY_HOP_HEADERS
    }


class RequestProxy:
    def __init__(
        self,
        target_resolver: TargetResolver,
        limit_resolver: LimitResolver,
        cache: RedisResponseCache,
        cache_policy: CachePolicy,
        singleflight: SingleFlight,
        limiter: RequestLimiter,
        circuit_breaker: CircuitBreaker,
        metrics: MetricsRecorder,
        http_client: httpx.AsyncClient,
    ) -> None:
        self.target_resolver = target_resolver
        self.limit_resolver = limit_resolver
        self.cache = cache
        self.cache_policy = cache_policy
        self.singleflight = singleflight
        self.limiter = limiter
        self.circuit_breaker = circuit_breaker
        self.metrics = metrics
        self.http_client = http_client

    async def handle(self, request: Request) -> Response:
        try:
            agent_id, subpath = extract_agent_id(request.url.path)
        except ValueError:
            return JSONResponse({"error": "invalid_agent_path"}, status_code=404)

        target = await self.target_resolver.resolve(agent_id)
        if target is None:
            return JSONResponse(
                {"error": "agent_target_not_found", "agent_id": agent_id},
                status_code=404,
                headers={"X-Request-Layer-Agent": agent_id, "X-Request-Layer-Cache": CacheState.bypass.value},
            )

        limits = await self.limit_resolver.resolve(agent_id)
        body = await request.body()
        headers = dict(request.headers)
        json_body: dict[str, Any] | None = None
        if request.method.upper() == "POST" and "application/json" in headers.get("content-type", ""):
            try:
                json_body = await request.json()
            except Exception:
                json_body = None

        cache_decision = (
            self.cache_policy.decide(agent_id, headers, json_body, target, limits)
            if json_body is not None
            else None
        )

        if cache_decision and cache_decision.cacheable and cache_decision.cache_key:
            cached = await self.cache.get(cache_decision.cache_key)
            if cached:
                await self.metrics.increment(agent_id, "cache_hits")
                return self._cached_response(agent_id, cached)

            claim = await self.singleflight.claim(cache_decision.cache_key)
            if not claim.owner:
                await self.metrics.increment(agent_id, "singleflight_waiters")
                ready = await self.singleflight.wait_until_ready(cache_decision.cache_key)
                cached = await self.cache.get(cache_decision.cache_key) if ready else None
                if cached:
                    await self.metrics.increment(agent_id, "cache_hits")
                    return self._cached_response(agent_id, cached)
                return JSONResponse(
                    {"error": "singleflight_timeout", "agent_id": agent_id},
                    status_code=503,
                    headers={
                        "Retry-After": "1",
                        "X-Request-Layer-Agent": agent_id,
                        "X-Request-Layer-Cache": CacheState.miss.value,
                        "X-Request-Layer-Limit-State": LimitState.normal.value,
                    },
                )
        else:
            claim = None
            await self.metrics.increment(agent_id, "cache_bypasses")

        await self.metrics.increment(agent_id, "cache_misses")

        circuit = await self.circuit_breaker.before_request(agent_id)
        if not circuit.allowed:
            if claim:
                await self.singleflight.release(claim)
            return JSONResponse(
                {"error": "circuit_open", "agent_id": agent_id},
                status_code=503,
                headers={
                    "Retry-After": str(circuit.retry_after_seconds),
                    "X-Request-Layer-Agent": agent_id,
                    "X-Request-Layer-Cache": CacheState.miss.value,
                    "X-Request-Layer-Limit-State": LimitState.circuit_open.value,
                },
            )

        request_id = str(uuid.uuid4())
        acquired = await self.limiter.acquire(agent_id, limits, request_id)
        await self.metrics.record_queue_wait(agent_id, acquired.queue_wait_ms)
        limit_state = LimitState.degraded.value if acquired.degraded else LimitState.normal.value
        if not acquired.acquired:
            if acquired.reason == "queue-timeout":
                await self.metrics.increment(agent_id, "queue_timeouts")
            if claim:
                await self.singleflight.release(claim)
            return JSONResponse(
                {"error": acquired.reason, "agent_id": agent_id},
                status_code=429,
                headers={
                    "Retry-After": str(acquired.retry_after_seconds),
                    "X-Request-Layer-Agent": agent_id,
                    "X-Request-Layer-Cache": CacheState.miss.value,
                    "X-Request-Layer-Queue-Wait-Ms": str(acquired.queue_wait_ms),
                    "X-Request-Layer-Limit-State": limit_state,
                },
            )

        started = time.monotonic()
        upstream_success = False
        try:
            await self.metrics.increment(agent_id, "upstream_requests")
            upstream_response = await self.http_client.request(
                request.method,
                build_upstream_url(target, subpath),
                content=body,
                headers=copy_request_headers(headers),
                params=dict(request.query_params),
            )
            upstream_success = 200 <= upstream_response.status_code < 500
            response_headers = copy_response_headers(dict(upstream_response.headers))
            response_headers.update(
                {
                    "X-Request-Layer-Agent": agent_id,
                    "X-Request-Layer-Cache": CacheState.miss.value,
                    "X-Request-Layer-Queue-Wait-Ms": str(acquired.queue_wait_ms),
                    "X-Request-Layer-Limit-State": limit_state,
                }
            )
            if (
                cache_decision
                and cache_decision.cacheable
                and cache_decision.cache_key
                and 200 <= upstream_response.status_code < 300
                and "application/json" in upstream_response.headers.get("content-type", "")
            ):
                await self.cache.set(
                    cache_decision.cache_key,
                    CachedResponse(
                        status_code=upstream_response.status_code,
                        media_type=upstream_response.headers.get("content-type"),
                        body=upstream_response.content,
                        headers=copy_response_headers(dict(upstream_response.headers)),
                    ),
                    ttl_seconds=limits.cache_ttl_seconds,
                )
            return Response(
                content=upstream_response.content,
                status_code=upstream_response.status_code,
                media_type=upstream_response.headers.get("content-type"),
                headers=response_headers,
            )
        except httpx.TimeoutException:
            await self.metrics.increment(agent_id, "upstream_errors")
            return JSONResponse(
                {"error": "upstream_timeout", "agent_id": agent_id},
                status_code=504,
                headers={
                    "X-Request-Layer-Agent": agent_id,
                    "X-Request-Layer-Cache": CacheState.miss.value,
                    "X-Request-Layer-Queue-Wait-Ms": str(acquired.queue_wait_ms),
                    "X-Request-Layer-Limit-State": limit_state,
                },
            )
        except httpx.HTTPError:
            await self.metrics.increment(agent_id, "upstream_errors")
            return JSONResponse(
                {"error": "upstream_error", "agent_id": agent_id},
                status_code=502,
                headers={
                    "X-Request-Layer-Agent": agent_id,
                    "X-Request-Layer-Cache": CacheState.miss.value,
                    "X-Request-Layer-Queue-Wait-Ms": str(acquired.queue_wait_ms),
                    "X-Request-Layer-Limit-State": limit_state,
                },
            )
        finally:
            elapsed_ms = int((time.monotonic() - started) * 1000)
            await self.metrics.record_latency(agent_id, elapsed_ms)
            await self.circuit_breaker.record_result(agent_id, success=upstream_success)
            await self.limiter.release(agent_id, degraded=acquired.degraded)
            if claim:
                await self.singleflight.release(claim)

    def _cached_response(self, agent_id: str, cached: CachedResponse) -> Response:
        headers = copy_response_headers(cached.headers)
        headers.update(
            {
                "X-Request-Layer-Agent": agent_id,
                "X-Request-Layer-Cache": CacheState.hit.value,
                "X-Request-Layer-Queue-Wait-Ms": "0",
                "X-Request-Layer-Limit-State": LimitState.normal.value,
            }
        )
        return Response(
            content=cached.body,
            status_code=cached.status_code,
            media_type=cached.media_type,
            headers=headers,
        )
```

- [ ] **Step 4: Run proxy helper tests**

Run:

```bash
cd agent-gateway/request-manager
python -m pytest tests/test_proxy_flow.py -q
```

Expected:

```text
3 passed
```

- [ ] **Step 5: Wire proxy into FastAPI app**

Replace `agent-gateway/request-manager/request_manager/main.py` with this content:

```python
from __future__ import annotations

import httpx
import redis.asyncio as redis
from fastapi import FastAPI, Request

from request_manager.cache import CachePolicy, RedisResponseCache
from request_manager.circuit_breaker import CircuitBreaker
from request_manager.limiter import RequestLimiter
from request_manager.metrics import MetricsRecorder
from request_manager.proxy import RequestProxy
from request_manager.settings import get_settings
from request_manager.singleflight import SingleFlight
from request_manager.target_resolver import LimitResolver, TargetResolver

settings = get_settings()

app = FastAPI(
    title="Nasiko Request Manager",
    version="0.1.0",
    description="Traffic-control layer for Nasiko agent requests.",
)


@app.on_event("startup")
async def startup() -> None:
    redis_client = redis.from_url(
        settings.redis_url,
        decode_responses=True,
        socket_timeout=settings.redis_timeout_seconds,
        socket_connect_timeout=settings.redis_timeout_seconds,
    )
    http_client = httpx.AsyncClient(timeout=settings.upstream_timeout_seconds)
    app.state.redis = redis_client
    app.state.http_client = http_client
    app.state.metrics = MetricsRecorder(redis_client)
    app.state.limit_resolver = LimitResolver(redis_client, settings)
    app.state.proxy = RequestProxy(
        target_resolver=TargetResolver(redis_client),
        limit_resolver=app.state.limit_resolver,
        cache=RedisResponseCache(redis_client),
        cache_policy=CachePolicy(),
        singleflight=SingleFlight(redis_client, settings.singleflight_wait_ms),
        limiter=RequestLimiter(redis_client, settings.global_active_cap),
        circuit_breaker=CircuitBreaker(
            redis_client,
            window_size=settings.circuit_window_size,
            min_failures=settings.circuit_min_failures,
            failure_ratio=settings.circuit_failure_ratio,
            open_seconds=settings.circuit_open_seconds,
        ),
        metrics=app.state.metrics,
        http_client=http_client,
    )


@app.on_event("shutdown")
async def shutdown() -> None:
    await app.state.http_client.aclose()
    await app.state.redis.aclose()


@app.get("/health")
async def health() -> dict[str, object]:
    redis_available = False
    try:
        redis_available = bool(await app.state.redis.ping())
    except Exception:
        redis_available = False
    return {
        "status": "healthy" if redis_available else "degraded",
        "service": settings.service_name,
        "redis_available": redis_available,
        "circuits": {},
    }

@app.api_route("/agents/{path:path}", methods=["GET", "POST", "PUT", "DELETE", "PATCH", "OPTIONS"])
async def proxy_agents(request: Request):
    return await app.state.proxy.handle(request)
```

- [ ] **Step 6: Commit proxy orchestration**

Run:

```bash
git add agent-gateway/request-manager/request_manager/proxy.py agent-gateway/request-manager/request_manager/main.py agent-gateway/request-manager/tests/test_proxy_flow.py
git commit -m "feat: proxy agent requests through request manager"
```

Expected:

```text
Commit succeeds and prints the new commit hash.
```

## Task 11: Add Control Endpoints And Dashboard

**Files:**

- Create: `agent-gateway/request-manager/request_manager/dashboard.py`
- Modify: `agent-gateway/request-manager/request_manager/main.py`

- [ ] **Step 1: Create dashboard HTML**

Create `agent-gateway/request-manager/request_manager/dashboard.py` with this content:

```python
from fastapi.responses import HTMLResponse


def dashboard_html() -> HTMLResponse:
    return HTMLResponse(
        """
<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8" />
  <meta name="viewport" content="width=device-width, initial-scale=1" />
  <title>Nasiko Request Manager</title>
  <style>
    :root { --ink:#17211c; --muted:#5f6f66; --panel:#fffaf0; --line:#dfd3bd; --accent:#0e7c5f; --warn:#b65324; }
    body { margin:0; font-family: ui-serif, Georgia, serif; color:var(--ink); background:linear-gradient(135deg,#f6eedf,#d9eadf); }
    main { width:min(1180px, calc(100vw - 32px)); margin:32px auto; }
    h1 { font-size:42px; margin:0 0 8px; letter-spacing:-0.03em; }
    p { color:var(--muted); }
    .grid { display:grid; grid-template-columns: repeat(auto-fit, minmax(220px, 1fr)); gap:16px; margin:24px 0; }
    .card { background:rgba(255,250,240,.82); border:1px solid var(--line); border-radius:22px; padding:18px; box-shadow:0 18px 50px rgba(45,35,20,.08); }
    .metric { font-size:34px; font-weight:700; }
    table { width:100%; border-collapse:collapse; background:rgba(255,250,240,.82); border-radius:18px; overflow:hidden; }
    th, td { padding:12px; border-bottom:1px solid var(--line); text-align:left; }
    th { color:var(--muted); font-size:12px; text-transform:uppercase; letter-spacing:.08em; }
    .pill { display:inline-block; border-radius:999px; padding:4px 9px; background:#dff2e7; color:var(--accent); font-size:12px; }
    .bad { background:#ffe1d0; color:var(--warn); }
  </style>
</head>
<body>
<main>
  <h1>Nasiko Request Manager</h1>
  <p>Live cache, queue, rate-limit, and circuit-breaker view for agent traffic.</p>
  <section class="grid" id="cards"></section>
  <table>
    <thead>
      <tr>
        <th>Agent</th><th>Active</th><th>Queued</th><th>Hit Rate</th><th>P95 Latency</th><th>P95 Queue</th><th>Circuit</th>
      </tr>
    </thead>
    <tbody id="agents"></tbody>
  </table>
</main>
<script>
async function refresh() {
  const res = await fetch('/control/stats');
  const data = await res.json();
  const hitTotal = data.cache_hits + data.cache_misses;
  const hitRate = hitTotal ? Math.round((data.cache_hits / hitTotal) * 100) : 0;
  document.getElementById('cards').innerHTML = [
    ['Status', data.status],
    ['Cache Hit Rate', hitRate + '%'],
    ['Active Requests', data.active_requests],
    ['Upstream Errors', data.upstream_errors],
    ['Queue Timeouts', data.queue_timeouts],
  ].map(([k,v]) => `<article class="card"><p>${k}</p><div class="metric">${v}</div></article>`).join('');
  document.getElementById('agents').innerHTML = data.agents.map(agent => {
    const total = agent.cache_hits + agent.cache_misses;
    const rate = total ? Math.round((agent.cache_hits / total) * 100) : 0;
    const cls = agent.circuit_state === 'closed' ? 'pill' : 'pill bad';
    return `<tr><td>${agent.agent_id}</td><td>${agent.active_requests}</td><td>${agent.queued_requests}</td><td>${rate}%</td><td>${agent.p95_latency_ms}ms</td><td>${agent.p95_queue_wait_ms}ms</td><td><span class="${cls}">${agent.circuit_state}</span></td></tr>`;
  }).join('');
}
refresh();
setInterval(refresh, 2000);
</script>
</body>
</html>
        """
    )
```

- [ ] **Step 2: Add control endpoints to main app**

In `agent-gateway/request-manager/request_manager/main.py`, add these imports:

```python
from fastapi import Body, Query
from request_manager import redis_keys
from request_manager.dashboard import dashboard_html
from request_manager.models import AgentLimits
```

Add these endpoints below `proxy_agents()`:

```python
@app.get("/")
async def dashboard():
    return dashboard_html()


async def _agent_ids() -> list[str]:
    try:
        return sorted(await app.state.redis.smembers(redis_keys.targets_index()))
    except Exception:
        return []


async def _agent_circuit_state(agent_id: str) -> str:
    try:
        circuit = await app.state.redis.hgetall(redis_keys.circuit(agent_id))
        return circuit.get("state", "closed")
    except Exception:
        return "degraded"


@app.get("/control/stats")
async def control_stats():
    agents = []
    for agent_id in await _agent_ids():
        limits = await app.state.limit_resolver.resolve(agent_id)
        agents.append(
            await app.state.metrics.agent_stats(
                agent_id,
                limits,
                await _agent_circuit_state(agent_id),
            )
        )
    try:
        redis_available = bool(await app.state.redis.ping())
    except Exception:
        redis_available = False
    return await app.state.metrics.global_stats(redis_available, agents)


@app.get("/control/agents/{agent_id}/stats")
async def agent_stats(agent_id: str):
    limits = await app.state.limit_resolver.resolve(agent_id)
    return await app.state.metrics.agent_stats(agent_id, limits, await _agent_circuit_state(agent_id))


@app.get("/control/limits")
async def control_limits():
    result = {}
    for agent_id in await _agent_ids():
        result[agent_id] = (await app.state.limit_resolver.resolve(agent_id)).model_dump()
    return result


@app.put("/control/limits/{agent_id}")
async def update_limits(agent_id: str, limits: AgentLimits = Body()):
    return await app.state.limit_resolver.update(agent_id, limits)


@app.delete("/control/cache")
async def clear_cache(agent: str | None = Query(default=None)):
    cleared = await app.state.proxy.cache.clear(agent_id=agent)
    return {"cleared": cleared, "agent": agent}
```

- [ ] **Step 3: Run full Request Manager tests**

Run:

```bash
cd agent-gateway/request-manager
python -m pytest tests -q
```

Expected:

```text
21 passed
```

- [ ] **Step 4: Commit control endpoints and dashboard**

Run:

```bash
git add agent-gateway/request-manager/request_manager/dashboard.py agent-gateway/request-manager/request_manager/main.py
git commit -m "feat: expose request manager controls"
```

Expected:

```text
Commit succeeds and prints the new commit hash.
```

## Task 12: Wire Docker Compose

**Files:**

- Modify: `docker-compose.local.yml`

- [ ] **Step 1: Add Request Manager service**

In `docker-compose.local.yml`, add this service in the gateway layer before `kong-service-registry`:

```yaml
  nasiko-request-manager:
    build:
      context: ./agent-gateway/request-manager
      dockerfile: Dockerfile
    container_name: nasiko-request-manager
    restart: unless-stopped
    depends_on:
      redis:
        condition: service_healthy
    environment:
      REDIS_URL: redis://redis:6379
      REQUEST_MANAGER_CACHE_TTL_SECONDS: "600"
      REQUEST_MANAGER_MAX_CONCURRENCY_PER_AGENT: "2"
      REQUEST_MANAGER_SUSTAINED_RPS_PER_AGENT: "5"
      REQUEST_MANAGER_BURST_CAPACITY_PER_AGENT: "10"
      REQUEST_MANAGER_MAX_QUEUE_DEPTH_PER_AGENT: "20"
      REQUEST_MANAGER_MAX_QUEUE_WAIT_MS: "10000"
      REQUEST_MANAGER_UPSTREAM_TIMEOUT_SECONDS: "45"
      REQUEST_MANAGER_GLOBAL_ACTIVE_CAP: "50"
    ports:
      - "8090:8090"
    networks:
      - app-network
      - agents-net
    healthcheck:
      test: ["CMD-SHELL", "python -c \"from urllib.request import urlopen; urlopen('http://localhost:8090/health')\" || exit 1"]
      interval: 30s
      timeout: 10s
      retries: 3
```

- [ ] **Step 2: Wire registry dependencies and environment**

In the existing `kong-service-registry` service:

Add this dependency:

```yaml
      nasiko-request-manager:
        condition: service_healthy
```

Add these environment variables:

```yaml
      REDIS_URL: redis://redis:6379
      KONG_REQUEST_MANAGER_HOST: nasiko-request-manager
      KONG_REQUEST_MANAGER_PORT: "8090"
      KONG_REQUEST_MANAGER_SERVICE_NAME: agent-request-manager
```

- [ ] **Step 3: Build the two changed images**

Run:

```bash
docker --context rancher-desktop compose -f docker-compose.local.yml --env-file .nasiko-local.env build nasiko-request-manager kong-service-registry
```

Expected:

```text
nasiko-request-manager  Built
kong-service-registry   Built
```

- [ ] **Step 4: Start local stack**

Run:

```bash
docker --context rancher-desktop compose -f docker-compose.local.yml --env-file .nasiko-local.env up -d nasiko-request-manager kong-service-registry
```

Expected:

```text
Container nasiko-request-manager  Healthy
Container kong-service-registry   Running
```

- [ ] **Step 5: Verify Request Manager health**

Run:

```bash
curl -sS http://localhost:8090/health
```

Expected JSON shape:

```json
{"status":"healthy","service":"nasiko-request-manager","redis_available":true,"circuits":{}}
```

- [ ] **Step 6: Commit Compose wiring**

Run:

```bash
git add docker-compose.local.yml
git commit -m "feat: run request manager locally"
```

Expected:

```text
Commit succeeds and prints the new commit hash.
```

## Task 13: Add KPI Demo Scripts

**Files:**

- Create: `scripts/request-layer/demo_cache_latency.py`
- Create: `scripts/request-layer/demo_singleflight.py`
- Create: `scripts/request-layer/demo_overload.py`

- [ ] **Step 1: Create cache latency demo**

Create `scripts/request-layer/demo_cache_latency.py` with this content:

```python
#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
import time
from urllib.request import Request, urlopen


def call(url: str, subject: str, text: str) -> tuple[float, str]:
    body = json.dumps({
        "jsonrpc": "2.0",
        "id": str(time.time_ns()),
        "method": "message/send",
        "params": {"message": {"parts": [{"kind": "text", "text": text}]}}
    }).encode("utf-8")
    req = Request(
        url,
        data=body,
        method="POST",
        headers={
            "content-type": "application/json",
            "x-subject-id": subject,
        },
    )
    start = time.perf_counter()
    with urlopen(req, timeout=60) as response:
        response.read()
        cache = response.headers.get("X-Request-Layer-Cache", "missing")
    return (time.perf_counter() - start) * 1000, cache


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--url", required=True, help="Full Kong agent URL, e.g. http://localhost:9100/agents/agent-a2a-demo")
    parser.add_argument("--text", default="Explain resilient request layers in one paragraph.")
    parser.add_argument("--subject", default="demo-user")
    args = parser.parse_args()

    cold_ms, cold_cache = call(args.url, args.subject, args.text)
    hot_ms, hot_cache = call(args.url, args.subject, args.text)
    improvement = 0 if cold_ms == 0 else round((1 - (hot_ms / cold_ms)) * 100, 1)

    print(json.dumps({
        "cold_ms": round(cold_ms, 2),
        "cold_cache": cold_cache,
        "hot_ms": round(hot_ms, 2),
        "hot_cache": hot_cache,
        "latency_reduction_percent": improvement,
        "target_met": hot_ms < 100 or improvement >= 80,
    }, indent=2))


if __name__ == "__main__":
    main()
```

- [ ] **Step 2: Create single-flight demo**

Create `scripts/request-layer/demo_singleflight.py` with this content:

```python
#!/usr/bin/env python3
from __future__ import annotations

import argparse
import concurrent.futures
import json
import time
from urllib.request import Request, urlopen


def call(url: str, index: int, text: str) -> str:
    body = json.dumps({
        "jsonrpc": "2.0",
        "id": f"sf-{index}",
        "method": "message/send",
        "params": {"message": {"parts": [{"kind": "text", "text": text}]}}
    }).encode("utf-8")
    req = Request(
        url,
        data=body,
        method="POST",
        headers={"content-type": "application/json", "x-subject-id": "singleflight-demo"},
    )
    with urlopen(req, timeout=60) as response:
        response.read()
        return response.headers.get("X-Request-Layer-Cache", "missing")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--url", required=True)
    parser.add_argument("--concurrency", type=int, default=20)
    parser.add_argument("--text", default="Return a compact description of Nasiko.")
    args = parser.parse_args()

    start = time.perf_counter()
    with concurrent.futures.ThreadPoolExecutor(max_workers=args.concurrency) as pool:
        results = list(pool.map(lambda i: call(args.url, i, args.text), range(args.concurrency)))
    elapsed_ms = (time.perf_counter() - start) * 1000
    print(json.dumps({
        "requests": args.concurrency,
        "elapsed_ms": round(elapsed_ms, 2),
        "cache_headers": {header: results.count(header) for header in sorted(set(results))},
        "target_met": results.count("HIT") + results.count("MISS") == args.concurrency,
    }, indent=2))


if __name__ == "__main__":
    main()
```

- [ ] **Step 3: Create overload demo**

Create `scripts/request-layer/demo_overload.py` with this content:

```python
#!/usr/bin/env python3
from __future__ import annotations

import argparse
import concurrent.futures
import json
import statistics
import time
from urllib.request import Request, urlopen
from urllib.error import HTTPError


def call(url: str, index: int) -> dict[str, object]:
    body = json.dumps({
        "jsonrpc": "2.0",
        "id": f"load-{index}",
        "method": "message/send",
        "params": {"message": {"parts": [{"kind": "text", "text": f"Load test unique prompt {index}"}]}}
    }).encode("utf-8")
    req = Request(url, data=body, method="POST", headers={"content-type": "application/json", "x-subject-id": "overload-demo"})
    start = time.perf_counter()
    try:
        with urlopen(req, timeout=60) as response:
            response.read()
            status = response.status
            wait = int(response.headers.get("X-Request-Layer-Queue-Wait-Ms", "0"))
    except HTTPError as error:
        status = error.code
        wait = int(error.headers.get("X-Request-Layer-Queue-Wait-Ms", "0"))
    return {"status": status, "wait_ms": wait, "latency_ms": round((time.perf_counter() - start) * 1000, 2)}


def p95(values: list[int]) -> float:
    if not values:
        return 0
    ordered = sorted(values)
    return ordered[int(round(0.95 * (len(ordered) - 1)))]


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--url", required=True)
    parser.add_argument("--requests", type=int, default=40)
    parser.add_argument("--concurrency", type=int, default=20)
    args = parser.parse_args()

    with concurrent.futures.ThreadPoolExecutor(max_workers=args.concurrency) as pool:
        results = list(pool.map(lambda i: call(args.url, i), range(args.requests)))

    statuses = [int(result["status"]) for result in results]
    waits = [int(result["wait_ms"]) for result in results]
    upstream_failures = [status for status in statuses if status >= 500 and status != 503]
    print(json.dumps({
        "requests": args.requests,
        "statuses": {str(status): statuses.count(status) for status in sorted(set(statuses))},
        "queue_p95_wait_ms": p95(waits),
        "upstream_failure_rate_percent": round((len(upstream_failures) / len(statuses)) * 100, 2),
        "target_met": p95(waits) < 5000 and (len(upstream_failures) / len(statuses)) < 0.05,
    }, indent=2))


if __name__ == "__main__":
    main()
```

- [ ] **Step 4: Make scripts executable**

Run:

```bash
chmod +x scripts/request-layer/demo_cache_latency.py scripts/request-layer/demo_singleflight.py scripts/request-layer/demo_overload.py
```

Expected:

```text
```

- [ ] **Step 5: Commit demo scripts**

Run:

```bash
git add scripts/request-layer
git commit -m "test: add request layer KPI demos"
```

Expected:

```text
Commit succeeds and prints the new commit hash.
```

## Task 14: End-To-End Verification

**Files:**

- No new files.

- [ ] **Step 1: Run unit tests**

Run:

```bash
cd agent-gateway/request-manager
python -m pytest tests -q
```

Expected:

```text
21 passed
```

Run:

```bash
python -m pytest agent-gateway/registry/tests/test_target_publisher.py -q
```

Expected:

```text
2 passed
```

- [ ] **Step 2: Rebuild and start the local services**

Run:

```bash
docker --context rancher-desktop compose -f docker-compose.local.yml --env-file .nasiko-local.env up -d --build nasiko-request-manager kong-service-registry
```

Expected:

```text
Container nasiko-request-manager  Healthy
Container kong-service-registry   Running
```

- [ ] **Step 3: Verify Redis target publication**

Run:

```bash
docker --context rancher-desktop compose -f docker-compose.local.yml --env-file .nasiko-local.env exec redis redis-cli --raw SMEMBERS request-manager:targets
```

Expected:

```text
One or more discovered agent ids are printed, each on its own line.
```

Capture one discovered agent id:

```bash
AGENT_ID="$(docker --context rancher-desktop compose -f docker-compose.local.yml --env-file .nasiko-local.env exec redis redis-cli --raw SRANDMEMBER request-manager:targets)"
printf '%s\n' "$AGENT_ID"
```

Expected:

```text
The printed value is a non-empty agent id.
```

Run one agent-specific lookup:

```bash
docker --context rancher-desktop compose -f docker-compose.local.yml --env-file .nasiko-local.env exec redis redis-cli HGETALL "request-manager:targets:${AGENT_ID}"
```

Expected fields:

```text
agent_id
the captured AGENT_ID value
upstream_url
an internal URL in the form `http://agent-name:5000`
target_revision
a non-empty revision value
```

- [ ] **Step 4: Verify Kong route points to Request Manager**

Run:

```bash
curl -sS http://localhost:9101/routes | python -m json.tool
```

Expected:

```text
Dynamic agent routes have "strip_path": false and their service is "agent-request-manager".
```

- [ ] **Step 5: Verify dashboard**

Open:

```text
http://localhost:8090/
```

Expected:

```text
Dashboard loads and displays status, cache hit rate, active requests, upstream errors, queue timeouts, and per-agent rows.
```

- [ ] **Step 6: Run cache KPI demo**

Run with the captured `AGENT_ID`:

```bash
python scripts/request-layer/demo_cache_latency.py --url "http://localhost:9100/agents/${AGENT_ID}"
```

Expected JSON:

```json
{
  "cold_cache": "MISS",
  "hot_cache": "HIT",
  "target_met": true
}
```

- [ ] **Step 7: Run single-flight KPI demo**

Clear cache first:

```bash
curl -sS -X DELETE http://localhost:8090/control/cache
```

Run:

```bash
python scripts/request-layer/demo_singleflight.py --url "http://localhost:9100/agents/${AGENT_ID}" --concurrency 20
```

Expected JSON:

```json
{
  "requests": 20,
  "target_met": true
}
```

Confirm `/control/stats` shows single-flight waiters:

```bash
curl -sS http://localhost:8090/control/stats | python -m json.tool
```

Expected:

```text
singleflight_waiters is greater than 0 for the demo agent.
```

- [ ] **Step 8: Run overload KPI demo**

Set strict demo limits:

```bash
curl -sS -X PUT "http://localhost:8090/control/limits/${AGENT_ID}" \
  -H 'content-type: application/json' \
  -d '{"cache_ttl_seconds":600,"max_concurrency":2,"sustained_rps":5,"burst_capacity":10,"max_queue_depth":20,"max_queue_wait_ms":10000,"cache_enabled":true}'
```

Run:

```bash
python scripts/request-layer/demo_overload.py --url "http://localhost:9100/agents/${AGENT_ID}" --requests 40 --concurrency 20
```

Expected:

```text
The script prints JSON with "target_met": true, "queue_p95_wait_ms" below 5000, and "upstream_failure_rate_percent" below 5.
```

The exact `queue_p95_wait_ms` value may be lower or higher depending on agent latency. The pass condition is `target_met: true`, which requires p95 queue wait below `5000ms` and upstream failure rate below `5%`.

- [ ] **Step 9: Verify existing router contract**

Use the current web UI or router endpoint exactly as before this change.

Expected:

```text
The router still returns its user-facing streaming response, and agent calls now show Request Manager cache/queue metrics behind the scenes.
```

- [ ] **Step 10: Commit verification notes if a docs update was needed**

If verification required changing commands, service names, or demo thresholds in this plan or the design spec, commit only those doc updates:

```bash
git add docs/superpowers/plans/2026-05-09-resilient-agent-request-layer.md docs/superpowers/specs/2026-05-09-resilient-agent-request-layer-design.md
git commit -m "docs: update request layer verification notes"
```

Expected when there are doc edits:

```text
Commit succeeds and prints the new commit hash.
```

Expected when there are no doc edits:

```text
No commit is needed.
```

## Acceptance Checklist

- [ ] `registry.py` publishes internal agent targets to Redis with `target_revision`.
- [ ] Kong dynamic `/agents/{agent}` routes point to `agent-request-manager`.
- [ ] Request Manager resolves targets from Redis and proxies to internal agent URLs.
- [ ] Router files remain unchanged for MVP.
- [ ] Cache only stores safe text-only A2A `message/send` 2xx JSON responses.
- [ ] Cache key includes agent id, method, normalized text, subject scope, and target revision.
- [ ] Repeated requests return `X-Request-Layer-Cache: HIT`.
- [ ] Concurrent identical misses collapse through single-flight.
- [ ] Cache hits do not consume limiter capacity.
- [ ] Cache misses use per-agent token bucket, concurrency cap, and bounded FIFO queue.
- [ ] Queue overflow and queue timeout return controlled responses with `Retry-After`.
- [ ] Circuit breaker returns controlled `503` with `Retry-After` when open.
- [ ] `/health`, `/control/stats`, `/control/agents/{agent}/stats`, `/control/limits`, `PUT /control/limits/{agent}`, and `DELETE /control/cache` work.
- [ ] Dashboard at `http://localhost:8090/` updates every two seconds.
- [ ] KPI demo scripts produce pass/fail JSON for latency, single-flight, and overload.
- [ ] End-to-end demo can show all four problem statement KPIs.

## KPI Mapping

- Faster repeated responses: Task 5 cache policy, Task 10 cache serving, Task 13 `demo_cache_latency.py`.
- Reduced duplicate processing: Task 6 single-flight, Task 10 single-flight wait path, Task 13 `demo_singleflight.py`.
- Stable overload handling: Task 7 limiter/queue, Task 8 circuit breaker, Task 13 `demo_overload.py`.
- Operational visibility: Task 9 metrics, Task 11 control endpoints/dashboard.

## Implementation Notes

- The Request Manager's cached response is the upstream agent HTTP response, not the router's user-facing `StreamingResponse`.
- `strip_path` must be `False` on dynamic Kong routes so Request Manager receives `/agents/{agent}` and can resolve the agent id.
- Request Manager must never call `http://kong-gateway:8000/agents/agent-a2a-demo`; it must use Redis `upstream_url` values such as `http://agent-a2a-demo:5000`.
- The registry is the only Docker/Kubernetes discovery owner in MVP.
- The router-to-Kong extra hop remains by design for MVP because it preserves auth, chat logging, and the existing `AgentClient` contract.

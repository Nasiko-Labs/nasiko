# Resilient Agent Request Layer Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a real resilient request layer in Nasiko's router that caches repeated agent responses, queues excess per-agent traffic, adapts limits from runtime pressure, and exposes admin and metrics endpoints.

**Architecture:** The implementation wraps `RouterOrchestrator._send_agent_request()` with a new `ResilientAgentExecutor`. The executor checks a cache before calling the real `AgentClient`, admits or queues requests with per-agent controls, records runtime stats, and stores successful responses. Admin routes and metrics are mounted in the existing FastAPI app.

**Tech Stack:** Python 3.12, FastAPI, Pydantic, httpx, pytest, asyncio, optional Redis via `redis.asyncio`, Nasiko `agent-gateway/router`.

---

## File Structure

- Create `agent-gateway/router/src/resilience/__init__.py`: exports the public resilience classes.
- Create `agent-gateway/router/src/resilience/models.py`: Pydantic models and dataclasses for cache config, limiter config, execution results, and stats snapshots.
- Create `agent-gateway/router/src/resilience/cache.py`: safe normalized response cache with TTL, Redis optional backend, local fallback, auth-scope isolation, file-upload bypass.
- Create `agent-gateway/router/src/resilience/limiter.py`: adaptive per-agent token bucket and bounded wait decisions.
- Create `agent-gateway/router/src/resilience/stats.py`: runtime counters, latency recording, queue depth, and Prometheus text rendering.
- Create `agent-gateway/router/src/resilience/executor.py`: orchestration wrapper around the existing real agent call.
- Create `agent-gateway/router/src/resilience/admin.py`: FastAPI admin router.
- Modify `agent-gateway/router/src/config/settings.py`: add resilience configuration.
- Modify `agent-gateway/router/src/services/router_orchestrator.py`: instantiate and use `ResilientAgentExecutor`.
- Modify `agent-gateway/router/src/main.py`: include admin routes and replace placeholder `/metrics`.
- Modify `agent-gateway/router/pyproject.toml`: add `redis` dependency.
- Modify `docker-compose.local.yml`: pass Redis URL and admin API key configuration to `nasiko-router`.
- Create `agent-gateway/router/tests/test_resilience_cache.py`.
- Create `agent-gateway/router/tests/test_resilience_limiter.py`.
- Create `agent-gateway/router/tests/test_resilience_executor.py`.
- Create `agent-gateway/router/tests/test_resilience_admin.py`.

## Task 1: Cache Models And Cache Behavior

**Files:**
- Create: `agent-gateway/router/src/resilience/models.py`
- Create: `agent-gateway/router/src/resilience/cache.py`
- Create: `agent-gateway/router/src/resilience/__init__.py`
- Test: `agent-gateway/router/tests/test_resilience_cache.py`

- [ ] **Step 1: Write failing cache tests**

Create tests that verify normalized repeated requests hit cache, auth scopes isolate entries, different agents isolate entries, TTL expiry misses, and file uploads are not cacheable.

- [ ] **Step 2: Run cache tests to verify failure**

Run: `cd agent-gateway/router && poetry run pytest tests/test_resilience_cache.py -v`
Expected: import failures for missing resilience modules.

- [ ] **Step 3: Implement cache models and local cache**

Add `CacheConfig`, `CacheLookup`, `CacheRecord`, `CacheEntry`, `CacheStats`, and `SemanticResponseCache`. Use SHA-256 over namespace, agent id, normalized query, auth scope, and route. Store only successful response text. Use monotonic time for TTL.

- [ ] **Step 4: Run cache tests to verify pass**

Run: `cd agent-gateway/router && poetry run pytest tests/test_resilience_cache.py -v`
Expected: all cache tests pass.

## Task 2: Adaptive Limiter And Queue Decisions

**Files:**
- Create: `agent-gateway/router/src/resilience/limiter.py`
- Modify: `agent-gateway/router/src/resilience/models.py`
- Test: `agent-gateway/router/tests/test_resilience_limiter.py`

- [ ] **Step 1: Write failing limiter tests**

Create tests that verify per-agent independence, burst allowance, token depletion, queue wait when temporary capacity is unavailable, rejection when max queue depth is exceeded, and effective rate tightens after high latency/error pressure.

- [ ] **Step 2: Run limiter tests to verify failure**

Run: `cd agent-gateway/router && poetry run pytest tests/test_resilience_limiter.py -v`
Expected: import failures for missing limiter.

- [ ] **Step 3: Implement limiter**

Add `LimitConfig`, `LimitDecision`, `AgentLimitState`, and `AdaptiveRateLimiter`. Implement token refill from monotonic time, per-agent configs, pressure calculation from latency/error/queue depth, and decision values `allow`, `queue`, and `reject`.

- [ ] **Step 4: Run limiter tests to verify pass**

Run: `cd agent-gateway/router && poetry run pytest tests/test_resilience_limiter.py -v`
Expected: all limiter tests pass.

## Task 3: Runtime Stats And Metrics

**Files:**
- Create: `agent-gateway/router/src/resilience/stats.py`
- Modify: `agent-gateway/router/src/resilience/models.py`
- Test: `agent-gateway/router/tests/test_resilience_admin.py`

- [ ] **Step 1: Write failing stats tests**

Create tests that record hits, misses, queue waits, queue depth, agent latency, errors, and rejections, then assert JSON snapshots and Prometheus text contain the required Buildthon KPI metrics.

- [ ] **Step 2: Run stats tests to verify failure**

Run: `cd agent-gateway/router && poetry run pytest tests/test_resilience_admin.py -v`
Expected: missing `RuntimeStats`.

- [ ] **Step 3: Implement stats**

Add `RuntimeStats` with counters, per-agent gauges, latency sums/counts, queue wait sums/counts, hit ratio calculation, and Prometheus text output.

- [ ] **Step 4: Run stats tests to verify pass**

Run: `cd agent-gateway/router && poetry run pytest tests/test_resilience_admin.py -v`
Expected: stats tests pass.

## Task 4: Resilient Executor

**Files:**
- Create: `agent-gateway/router/src/resilience/executor.py`
- Modify: `agent-gateway/router/src/resilience/__init__.py`
- Test: `agent-gateway/router/tests/test_resilience_executor.py`

- [ ] **Step 1: Write failing executor tests**

Create async tests that use a fake real-agent callable. Verify first request calls the agent and stores response, second identical request returns cached response without calling the agent, file requests bypass cache, queued requests eventually execute, full queues return a bounded failure, and failed agent calls are not cached.

- [ ] **Step 2: Run executor tests to verify failure**

Run: `cd agent-gateway/router && poetry run pytest tests/test_resilience_executor.py -v`
Expected: missing `ResilientAgentExecutor`.

- [ ] **Step 3: Implement executor**

Add `ResilientAgentExecutor.execute()`. It accepts request, files, token, agent id/url, and a coroutine for the real A2A call. It checks cache, asks limiter for admission, waits with bounded async sleeps when queued, records queue wait and latency, stores successful responses, and returns either response text or raises an execution error with status and retry-after data.

- [ ] **Step 4: Run executor tests to verify pass**

Run: `cd agent-gateway/router && poetry run pytest tests/test_resilience_executor.py -v`
Expected: executor tests pass.

## Task 5: Wire Into Nasiko Router

**Files:**
- Modify: `agent-gateway/router/src/config/settings.py`
- Modify: `agent-gateway/router/src/services/router_orchestrator.py`
- Modify: `agent-gateway/router/src/main.py`
- Modify: `agent-gateway/router/pyproject.toml`
- Modify: `docker-compose.local.yml`
- Test: existing router tests plus executor tests.

- [ ] **Step 1: Write failing integration tests where feasible**

Add tests or assertions that `RouterOrchestrator` owns a `ResilientAgentExecutor` and `_send_agent_request()` returns a cached final response after a repeated request.

- [ ] **Step 2: Run integration test to verify failure**

Run: `cd agent-gateway/router && poetry run pytest tests/test_resilience_executor.py tests/test_resilience_admin.py -v`
Expected: integration assertions fail until wiring is complete.

- [ ] **Step 3: Add settings**

Add settings for `RESILIENCE_ENABLED`, `RESILIENCE_CACHE_TTL_SECONDS`, `RESILIENCE_DEFAULT_AGENT_RPS`, `RESILIENCE_MIN_AGENT_RPS`, `RESILIENCE_BURST`, `RESILIENCE_MAX_QUEUE_DEPTH`, `RESILIENCE_MAX_QUEUE_WAIT_SECONDS`, `RESILIENCE_TARGET_LATENCY_SECONDS`, `RESILIENCE_ADMIN_API_KEY`, and `REDIS_URL`.

- [ ] **Step 4: Wire executor**

Instantiate `ResilientAgentExecutor` in `RouterOrchestrator.__init__()`. In `_send_agent_request()`, call the executor with a real callable that invokes `self.agent_client.send_request()` and `extract_response_content()`.

- [ ] **Step 5: Replace metrics placeholder**

Mount admin routes and make `/metrics` return `PlainTextResponse(orchestrator.resilient_executor.metrics_text())`.

- [ ] **Step 6: Update compose**

Pass `REDIS_URL=redis://redis:6379` and resilience settings to `nasiko-router`.

- [ ] **Step 7: Run focused tests**

Run: `cd agent-gateway/router && poetry run pytest tests/test_resilience_cache.py tests/test_resilience_limiter.py tests/test_resilience_executor.py tests/test_resilience_admin.py -v`
Expected: all focused tests pass.

## Task 6: Final Verification

**Files:**
- All files modified above.

- [ ] **Step 1: Run router test suite**

Run: `cd agent-gateway/router && poetry run pytest -v`
Expected: no regressions in the router package.

- [ ] **Step 2: Inspect git diff**

Run: `git diff --stat`
Expected: changes are limited to router resilience code, tests, config, compose, and docs.

- [ ] **Step 3: Commit implementation**

Run:

```bash
git add agent-gateway/router docker-compose.local.yml docs/superpowers/plans/2026-05-09-resilient-agent-request-layer.md
git commit -m "feat: add resilient agent request layer"
```

Expected: implementation commit succeeds.

## Self-Review

Spec coverage:

- Semantic cache: Tasks 1, 4, and 5.
- Per-agent adaptive limits: Task 2.
- Queueing: Tasks 2 and 4.
- Admin controls: Tasks 3 and 5.
- Runtime metrics: Tasks 3 and 5.
- Real Nasiko router integration: Task 5.

Placeholder scan: no placeholder implementation steps remain.

Type consistency: `SemanticResponseCache`, `AdaptiveRateLimiter`, `RuntimeStats`, and `ResilientAgentExecutor` are introduced before integration uses them.


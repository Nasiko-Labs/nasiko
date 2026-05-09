# Resilient Agent Request Layer Design

Date: 2026-05-09
Status: Approved for implementation planning

## Problem Statement

Nasiko needs a unified traffic-control layer between the gateway and agent fleet. The layer must reduce redundant compute for repeated agent requests, prevent one overloaded agent from destabilizing the platform, and expose operational controls for cache, limits, queues, and runtime stats.

The Buildthon success metrics are:

- Faster repeated responses through reduced repeated-response latency.
- Reduced duplicate processing through measurable cache hit rate and single-flight dedupe.
- Stable overload handling through lower failures during spikes and predictable queue times.
- Operational visibility through real-time monitoring dashboards and control endpoints.

## Current Codebase Context

Nasiko already has the right seams for this design:

- Kong is the public gateway on port `9100` and routes `/router`, `/api`, `/auth`, `/app`, and dynamic `/agents/{agent}` traffic.
- `agent-gateway/registry/registry.py` discovers agent containers on `agents-net` and registers dynamic Kong routes for `/agents/{container}`.
- The router service (`agent-gateway/router`) selects an agent, then calls the chosen agent through a Kong `/agents/{agent}` URL.
- Nasiko agents communicate through the A2A JSON-RPC protocol. The standard upstream execution method is `message/send`, which is why cacheability rules key off that method.
- Router-level caches already exist for routing support: agent cards are cached in `AgentRegistry`, and FAISS/vector-store data is cached in `VectorStoreService`.
- Redis is already available in `docker-compose.local.yml` and is used by other platform workflows.
- Existing chat logging is implemented as a Kong plugin, proving gateway extensibility, but the queueing/adaptive logic is better expressed in a dedicated service than in Lua.

## Chosen Architecture

Add a dedicated **Request Manager** service after Kong and before the agent fleet.

```text
Client / Router
  -> Kong Gateway
      -> Request Manager
          -> Redis
          -> Internal Agent Container
```

For routed requests:

```text
Client -> Kong -> Router -> Kong /agents/{agent} -> Request Manager -> Agent
```

For MVP, the router keeps calling the existing Kong `/agents/{agent}` URL instead of calling the Request Manager directly. This preserves the current `AgentClient` contract, gateway middleware behavior, and chat logging path while avoiding router-specific wiring. The extra local hop is acceptable for MVP because agent/LLM execution dominates latency. A later production optimization can add a direct internal `Router -> Request Manager` path after equivalent auth/logging/metrics behavior exists there.

For direct agent requests:

```text
Client -> Kong /agents/{agent} -> Request Manager -> Agent
```

Kong remains the public front door for auth, CORS, and route matching. The Request Manager becomes the execution-control layer for all `/agents/*` traffic. The router decides which agent should handle a request; the Request Manager controls execution once a specific agent is selected.

### Agent Target Discovery

`agent-gateway/registry/registry.py` remains the single owner of Docker/Kubernetes agent discovery. Instead of duplicating Docker socket or Kubernetes watcher logic inside the Request Manager, extend `registry.py` to publish a Redis-backed internal target table whenever it discovers, updates, or removes an agent.

Target records are keyed by public agent id and include:

- Public agent id, for example `agent-a2a-translator`.
- Public path, for example `/agents/agent-a2a-translator`.
- Internal upstream URL, for example `http://agent-a2a-translator:5000` in local Docker or the Kubernetes service DNS URL in cluster mode.
- `target_revision`, derived from container id or image id locally and deployment/service version metadata in Kubernetes.
- Health/discovery source and last-updated timestamp.

Kong dynamic `/agents/{agent}` routes point to the Request Manager service. The Request Manager resolves `{agent}` through the Redis target table, then proxies to the internal upstream URL. This prevents proxy loops because the Request Manager never calls the public Kong `/agents/*` URL.

## Why This Placement

The layer sits after Kong, not before it, because placing it before Kong would duplicate gateway concerns such as public routing and auth. It sits outside the router because router-only caching/rate limiting would miss direct `/agents/*` traffic and would not satisfy the gateway-to-agent traffic-control requirement. A shared Python library inside the router has the same limitation: it can optimize routed calls, but it cannot protect direct agent calls that never enter the router.

This placement gives one chokepoint for all agent execution. It lets direct calls and router-selected calls share the same cache, queue, limit, and metric model.

## Request Flow

Every `/agents/{agent}` request handled by the Request Manager follows this order:

1. Classify the request and extract the target agent.
2. Resolve the internal agent target from the Redis target table.
3. Decide whether the request is cacheable.
4. Check Redis cache before any agent call.
5. Collapse identical in-flight cache misses so concurrent duplicates share one computation.
6. Acquire per-agent capacity or wait in a bounded FIFO queue.
7. Proxy the request to the resolved internal agent target.
8. Cache successful responses when safe.
9. Release capacity and record metrics.
10. Return the agent response or a controlled overload response.

## Cache Design

### Placement

Agent response caching belongs in the Request Manager after routing, because the selected agent is known. This avoids stale or incorrect attribution from caching before routing and lets the same cache serve both router-driven and direct agent calls.

Router-level caches remain routing-support caches only:

- Agent registry/card cache: keep in router.
- Vector store / embeddings cache: keep in router.
- Agent response cache: move to Request Manager.
- Agent-selection cache: defer because session history and changing registries make this risky.

### Cacheable Requests

MVP cacheability is intentionally conservative:

In this design, "agent response" means the upstream HTTP response returned by the agent to the Request Manager, typically a JSON A2A response. It does not mean the router's user-facing `StreamingResponse`, which is a progress stream emitted by the router while it orchestrates the request.

- Cache only HTTP JSON requests using A2A JSON-RPC `message/send`.
- Cache only text-only user messages.
- Cache only successful 2xx responses with valid JSON result payloads.
- Cache only agents/endpoints not explicitly marked non-cacheable.
- Honor `Cache-Control: no-cache` by bypassing lookup and write.

Do not cache:

- Errors, timeouts, 4xx, 5xx, or circuit-breaker responses.
- File uploads or non-text message parts.
- Streaming or partial responses.
- Side-effect-heavy agents such as email-sending or ticket-creation agents unless explicitly declared safe.

### Cache Key

The cache key is a stable hash over:

- Agent id from `/agents/{agent}`.
- JSON-RPC method.
- Normalized text parts.
- Safe route/config metadata that affects response shape.
- User or tenant scope from gateway/auth headers, preferably `X-Subject-ID`; local unauthenticated calls use an explicit `anonymous` scope.
- `target_revision` from the Redis target table.

Normalization for MVP is limited to trimming whitespace, lowercasing, and stable JSON field ordering. Semantic matching is deferred because it increases cost and risks returning incorrect responses.

### TTL, Eviction, And Invalidation

MVP uses Redis TTL eviction:

- Global default TTL, configurable through environment/config.
- Per-agent TTL override through runtime config.
- Cache entries always include `target_revision`.
- Per-agent cache invalidation is triggered on `target_revision` change, cacheability/config change, or manually through control endpoints.

LRU/LFU memory policies, cache warming, and semantic cache are deferred.

### Single-Flight Dedupe

The Request Manager deduplicates concurrent identical cache misses. The first request computes; duplicates wait for the same result up to a bounded wait. This prevents cache stampedes and directly reduces duplicate processing during bursts.

## Rate Limiting Design

Per-agent concurrency is the primary protection because concurrent in-flight work is what consumes agent resources and causes cascading overload. Per-agent token-bucket RPS limiting is the sustained-rate protection and burst-smoothing algorithm for MVP.

Cache hits do not consume agent capacity or rate-limit tokens because they do not reach the agent. Cache misses pass through the per-agent limiter before proxying.

MVP limit dimensions:

- Per-agent concurrency cap.
- Per-agent token bucket with sustained RPS and burst capacity.
- Per-agent bounded queue depth.
- Per-agent max queue wait.
- Global safety cap.
- Runtime per-agent overrides.

Deferred dimensions:

- Per-user or per-tenant fairness.
- Premium priority lanes.
- Fully dynamic auto-tuning.

Redis-backed counters/locks coordinate limits across multiple Request Manager replicas. If Redis is unavailable, the service enters degraded mode: bypass cache, use conservative in-process limits, and expose degraded health instead of silently removing all protection.

## Recommended MVP Defaults

These defaults are intentionally conservative for local demo and can be overridden at runtime:

- Cache TTL: `600` seconds.
- Per-agent max concurrency: `2`.
- Per-agent sustained RPS: `5`.
- Per-agent burst capacity: `10`.
- Per-agent max queue depth: `20`.
- Per-request max queue wait: `10_000` milliseconds.
- Agent upstream timeout: `45` seconds.
- Global safety cap: `50` active upstream requests, enabled by default.
- Circuit breaker open trigger: at least `5` failures and `50%` error/timeout rate over a rolling `20` requests.
- Circuit breaker open duration: `30` seconds before half-open probe.

## Circuit-Breaker Degradation

MVP adaptive behavior is limited to circuit-breaker-based degradation. The Request Manager does not dynamically tune concurrency from p95 latency in MVP.

When an agent crosses the circuit-breaker threshold, the Request Manager opens the circuit for that agent and returns controlled `503` responses with `Retry-After` until the open duration expires. It then allows a small half-open probe. If the probe succeeds, the circuit closes; if it fails, the circuit opens again.

More advanced p95-latency-based concurrency tuning, queue wait adjustment, and automatic limit recovery are production-evolution items.

## Queue Design

MVP queueing is bounded synchronous FIFO:

- If capacity is available, run immediately.
- If capacity is full and queue has room, the HTTP request waits.
- If a slot opens before timeout, the request proceeds and returns the normal agent response.
- If queue is full or wait timeout expires, return `429` or `503` with `Retry-After` and queue metadata.

This preserves the current client contract. Existing `/router` and `/agents/*` callers continue receiving normal HTTP responses without a new job-id/polling flow.

Production evolution adds an async job mode for long-running workflows:

```text
POST request -> 202 Accepted + job_id -> status/progress endpoint -> final result
```

Interactive requests remain synchronous; long-running workflows can move to async when needed.

## Failure Handling

MVP failure behavior:

- Per-agent request timeout so slow agents do not hold capacity forever.
- Circuit breaker opens after repeated timeout/error thresholds.
- No automatic retries by default; naive retries can amplify overload.
- Cache is bypassed for failures.
- Queue overflow returns controlled overload responses, not unbounded waits.
- Redis degraded mode uses conservative in-process protection and reports degraded health.

Circuit breaker responses include enough metadata for operators and clients to understand whether the failure is overload, timeout, or open-circuit protection.

### Timeout Budget

The Request Manager timeout budget must fit beneath the caller's timeout. For MVP, queue wait is capped at `10` seconds and upstream agent execution at `45` seconds, keeping the Request Manager's worst-case wait below roughly `55` seconds plus small proxy overhead. This is intentionally below the existing router `REQUEST_TIMEOUT` default of `60` seconds and Kong's longer service timeouts, so the Request Manager returns controlled outcomes before upstream callers give up.

## Operational Controls

The Request Manager exposes JSON control endpoints and a minimal dashboard.

Required endpoints:

- `GET /health`: service readiness, Redis readiness/degraded mode, and aggregate circuit state.
- `GET /control/stats`: global cache, latency, queue, and error summary.
- `GET /control/agents/{agent}/stats`: per-agent active requests, queued requests, hit rate, p50/p95 latency, errors, timeouts, and circuit state.
- `GET /control/limits`: current global and per-agent limits.
- `PUT /control/limits/{agent}`: update concurrency, queue size, TTL, and wait timeout.
- `DELETE /control/cache`: clear all cache or filter by agent.

Development can use a simple admin token or open local endpoints. Production should require admin JWT/API key authorization and audit control changes.

Useful response headers:

- `X-Request-Layer-Cache`: `HIT`, `MISS`, or `BYPASS`.
- `X-Request-Layer-Agent`: resolved agent id.
- `X-Request-Layer-Queue-Wait-Ms`: time spent waiting for capacity.
- `X-Request-Layer-Limit-State`: `normal`, `degraded`, or `circuit-open`.

## Dashboard

A minimal live dashboard is part of MVP, not stretch, because the problem statement explicitly calls out real-time monitoring dashboards.

Dashboard cards:

- Cache hit rate.
- Cold vs cached latency.
- Estimated latency saved.
- Per-agent queue depth.
- Per-agent active requests.
- 429/503/timeouts/errors.
- Circuit breaker state.
- Current limits and TTL.
- Agent-level breakdown.

Prometheus `/metrics`, Phoenix span attributes, alerting, and richer UI polish are stretch goals.

## Configuration Model

MVP uses a two-layer configuration model:

1. Global defaults from environment/config.
2. Per-agent Redis overrides written by the control API.

Merge order is deterministic: Redis per-agent overrides replace global defaults for only the fields they set; all missing fields fall back to global defaults. Optional future AgentCard hints for cacheability and safe request types may be added later, but ops-controlled config remains authoritative.

## Demo And KPI Verification

The MVP includes repeatable demo scripts so every KPI has evidence.

### Demo Targets

- Cached response latency: under `100ms` locally, or at least `80%` faster than the cold call when machine load makes an absolute target noisy.
- Repeated workflow cache hit rate: above `90%` for identical safe text requests after the first miss.
- Concurrent duplicate processing: one upstream agent call for a burst of identical misses, with the rest served by single-flight/cache.
- Overload failure rate: below `5%` upstream agent failures during a 2x limit burst; controlled `429`/`503` responses are counted separately from upstream failures.
- Queue p95 wait: below `5s` for a configured demo burst that fits within queue capacity.
- Operational visibility: dashboard and `/control/stats` update during the demo without service restart.

### Faster Repeated Responses

Script: `demo_cache_latency`

Flow:

1. Send a text-only request to an agent.
2. Record cold latency and cache miss.
3. Send the same request again.
4. Record cached latency and cache hit.

Expected evidence:

- Second response is materially faster.
- Dashboard shows cache hit count and hit rate.
- Response headers or stats indicate cache hit/miss.

### Reduced Duplicate Processing

Script: `demo_singleflight`

Flow:

1. Send many identical concurrent requests.
2. Ensure only one upstream agent execution occurs for the same key.
3. Serve duplicates from single-flight result and/or cache.

Expected evidence:

- Upstream call count stays near one for identical concurrent misses.
- Dashboard shows single-flight waiters and cache writes.

### Stable Overload Handling

Script: `demo_overload`

Flow:

1. Set low concurrency for one agent.
2. Send burst traffic above that limit.
3. Observe queueing, bounded waits, and controlled overflow.
4. Optionally send traffic to another agent to show cross-agent isolation.

Expected evidence:

- Queue depth rises predictably.
- Some requests wait and complete.
- Overflow returns `429`/`503` only after queue depth or timeout limits.
- Other agents remain unaffected.

### Operational Visibility

Flow:

1. Open dashboard.
2. Hit `/control/stats` and per-agent stats endpoints.
3. Change a limit through control API.
4. Observe runtime effect without restart.

Expected evidence:

- Live cache, queue, latency, failure, and limit metrics.
- Runtime control changes reflected immediately.

## MVP Scope

Included:

- New dedicated Request Manager service.
- Kong registry integration so public `/agents/*` routes reach Request Manager.
- Redis-published internal agent-target resolution from `registry.py` to avoid proxy loops.
- Redis exact response cache.
- Single-flight dedupe.
- Per-agent concurrency and bounded synchronous FIFO queue.
- Circuit-breaker-based degradation.
- Control endpoints.
- Minimal live dashboard.
- Demo scripts for all KPIs.

Excluded from MVP:

- Semantic response cache.
- Async job mode.
- Streaming response caching.
- Cache warming.
- LRU/LFU eviction beyond Redis TTL.
- Per-tenant fairness.
- Priority queues.
- Prometheus/Phoenix deep integration.
- Production audit log and alerting.
- p95-latency-based adaptive concurrency tuning.

## Risks And Mitigations

Risk: Proxy loop between Kong and Request Manager.

Mitigation: `registry.py` publishes internal target URLs to Redis. The Request Manager resolves agents from that table and never calls public `/agents/*` Kong URLs.

Risk: Cache-key collision or over-normalization returns an incorrect response.

Mitigation: Use SHA-256 over stable JSON fields, include agent id, user/tenant scope, and `target_revision`, and keep normalization conservative.

Risk: Incorrect cache hits for side-effect or user-specific agents.

Mitigation: Conservative cacheability, user/tenant scope in key, per-agent opt-out, no caching for errors/uploads/streams.

Risk: Queueing ties up HTTP connections.

Mitigation: Bound max queue depth and max wait; return `Retry-After` on overflow; document async job mode as production evolution.

Risk: Redis outage removes distributed coordination.

Mitigation: Degraded mode with local conservative limits, cache bypass, and degraded health.

Risk: Adaptive behavior becomes hard to explain.

Mitigation: MVP limits adaptive behavior to a circuit breaker with visible metrics and clear closed, open, and half-open states.

## Production Evolution

After MVP, the design can evolve without changing the placement:

- Add async job mode for long-running workflows.
- Add Prometheus metrics and Phoenix span attributes.
- Add per-tenant fairness and priority classes.
- Add semantic cache for explicitly safe agents.
- Add direct internal `Router -> Request Manager` calls after equivalent auth/logging/metrics behavior is available.
- Add p95-latency-based adaptive concurrency tuning and automatic recovery.
- Add audit logs for control actions.
- Add alerting for queue spikes, low hit rate, and circuit-open states.
- Add richer health-aware routing feedback to router or gateway.

## Implementation Planning Inputs

The implementation plan will specify exact file layout, Docker image wiring, Redis key names, Kong registry changes, and test scripts. The design intent is fixed: preserve existing client contracts while centralizing execution traffic control behind Kong.

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

For direct agent requests:

```text
Client -> Kong /agents/{agent} -> Request Manager -> Agent
```

Kong remains the public front door for auth, CORS, and route matching. The Request Manager becomes the execution-control layer for all `/agents/*` traffic. The router decides which agent should handle a request; the Request Manager controls execution once a specific agent is selected.

## Why This Placement

The layer sits after Kong, not before it, because placing it before Kong would duplicate gateway concerns such as public routing and auth. It sits outside the router because router-only caching/rate limiting would miss direct `/agents/*` traffic and would not satisfy the gateway-to-agent traffic-control requirement.

This placement gives one chokepoint for all agent execution. It lets direct calls and router-selected calls share the same cache, queue, limit, and metric model.

## Request Flow

Every `/agents/{agent}` request handled by the Request Manager follows this order:

1. Classify the request and extract the target agent.
2. Decide whether the request is cacheable.
3. Check Redis cache before any agent call.
4. Collapse identical in-flight cache misses so concurrent duplicates share one computation.
5. Acquire per-agent capacity or wait in a bounded FIFO queue.
6. Proxy the request to the internal agent target.
7. Cache successful responses when safe.
8. Release capacity and record metrics.
9. Return the agent response or a controlled overload response.

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
- User or tenant scope when available from gateway/auth headers.
- Agent version or version token when available.

Normalization for MVP is limited to trimming whitespace, lowercasing, and stable JSON field ordering. Semantic matching is deferred because it increases cost and risks returning incorrect responses.

### TTL, Eviction, And Invalidation

MVP uses Redis TTL eviction:

- Global default TTL, configurable through environment/config.
- Per-agent TTL override through runtime config.
- Cache entries include an agent version token when available.
- Per-agent cache invalidation is triggered on agent version/config change or manually through control endpoints.

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
- Optional global safety cap.
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
- Per-request max queue wait: `30_000` milliseconds.
- Agent upstream timeout: `60` seconds.
- Global safety cap: `50` active upstream requests.
- Circuit breaker open trigger: at least `5` failures and `50%` error/timeout rate over a rolling `20` requests.
- Circuit breaker open duration: `30` seconds before half-open probe.

## Adaptive Degradation

The limiter is configurable first, adaptive second. Each agent has base limits, and the Request Manager adjusts behavior based on rolling signals:

- p95 latency above threshold.
- Error or timeout rate above threshold.
- Queue depth near or above threshold.
- Circuit breaker state.

During degradation, the layer may:

- Shorten queue wait.
- Reduce effective concurrency temporarily.
- Return faster overload responses with `Retry-After`.
- Open a circuit for consistently failing agents.

When health recovers for a configured cooldown period, the agent returns toward its base limits.

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

## Operational Controls

The Request Manager exposes JSON control endpoints and a minimal dashboard.

Required endpoints:

- `GET /health`: service and Redis readiness.
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

Use a layered configuration model:

1. Global defaults from environment/config.
2. Per-agent overrides in Redis/config store.
3. Runtime updates through control API.
4. Optional future AgentCard hints for cacheability and safe request types.

Ops-controlled config takes precedence over AgentCard hints. This avoids trusting agent authors to define platform safety limits unilaterally.

## Demo And KPI Verification

The MVP includes repeatable demo scripts so every KPI has evidence.

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
- Internal agent-target resolution to avoid proxy loops.
- Redis exact response cache.
- Single-flight dedupe.
- Per-agent concurrency and bounded synchronous FIFO queue.
- Simple adaptive degradation and circuit breaker.
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

## Risks And Mitigations

Risk: Proxy loop between Kong and Request Manager.

Mitigation: The Request Manager must call internal container host/port targets, not public `/agents/*` Kong URLs.

Risk: Incorrect cache hits for side-effect or user-specific agents.

Mitigation: Conservative cacheability, user/tenant scope in key, per-agent opt-out, no caching for errors/uploads/streams.

Risk: Queueing ties up HTTP connections.

Mitigation: Bound max queue depth and max wait; return `Retry-After` on overflow; document async job mode as production evolution.

Risk: Redis outage removes distributed coordination.

Mitigation: Degraded mode with local conservative limits, cache bypass, and degraded health.

Risk: Adaptive limits become hard to explain.

Mitigation: MVP uses simple threshold-based degradation with visible metrics and clear state transitions.

## Production Evolution

After MVP, the design can evolve without changing the placement:

- Add async job mode for long-running workflows.
- Add Prometheus metrics and Phoenix span attributes.
- Add per-tenant fairness and priority classes.
- Add semantic cache for explicitly safe agents.
- Add audit logs for control actions.
- Add alerting for queue spikes, low hit rate, and circuit-open states.
- Add richer health-aware routing feedback to router or gateway.

## Implementation Planning Inputs

The implementation plan will specify exact file layout, Docker image wiring, Redis key names, Kong registry changes, and test scripts. The design intent is fixed: preserve existing client contracts while centralizing execution traffic control behind Kong.

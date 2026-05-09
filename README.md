# Nasiko: Resilient AI Request Orchestration Middleware

Nasiko is a FastAPI-based infrastructure middleware for resilient AI agent request handling. It demonstrates how an AI system can stay responsive during burst traffic by combining async request processing, intelligent caching, request coalescing, per-agent rate limiting, queue-based overload smoothing, and real-time observability.

The project is intentionally lightweight and hackathon-friendly. It uses in-memory components and Python `asyncio` so reviewers can understand the architecture quickly, run the demo locally, and see the resilience behavior without Redis, Kafka, databases, Docker orchestration, or external monitoring stacks.

## Project Overview

AI agent applications often fail in predictable ways:

- Burst traffic overwhelms individual agents.
- Repeated prompts waste expensive model or tool compute.
- Concurrent identical cache misses trigger duplicate processing.
- Overload is hard to explain without clear operational metrics.
- Traffic spikes can cascade into slower responses, rejected requests, or unstable demos.

Nasiko acts as a traffic orchestration layer in front of AI agents. It protects agent capacity, avoids unnecessary recomputation, buffers overload, and exposes the system behavior through reviewer-friendly APIs.

## Problem Statement

Modern AI systems need more than a simple request/response route. As traffic increases, a naive backend can suffer from:

- Duplicate computation: repeated or simultaneous identical requests are processed multiple times.
- Cache stampedes: concurrent cache misses all call the same expensive agent before the first response is cached.
- Overloaded agents: one popular agent can receive more requests than it can process safely.
- Cascading failures: aggressive rejection or slow processing can degrade the whole API experience.
- Poor traffic visibility: without metrics, judges, maintainers, and operators cannot see why the system is stable or overloaded.

Nasiko focuses on these backend resilience concerns while keeping the implementation small enough to audit in a pull request.

## Solution Architecture

The middleware follows a modular request pipeline:

1. Validate the request and target agent.
2. Check the cache before consuming agent capacity.
3. Use per-cache-key async locks to coalesce duplicate cache misses.
4. Apply per-agent sliding-window rate limits.
5. Process allowed cache misses immediately.
6. Queue overloaded cache misses instead of failing aggressively.
7. Drain queued requests in a background async worker.
8. Store successful results in cache.
9. Update structured observability metrics.

Core design principles:

- Cache first: repeated prompts should be fast and should not consume rate-limit capacity.
- Coalesce duplicate work: simultaneous identical misses should wait for the first result.
- Limit per agent: noisy traffic to one agent should not overload every agent.
- Queue overload: spikes should be smoothed when possible.
- Observe everything: cache, limiter, queue, latency, and agent behavior should be visible.

## Architecture Diagram

```text
Users
  |
  v
Gateway
  |
  v
Resilient Request Layer
  |
  +-- Cache
  |     +-- In-memory response cache
  |     +-- Cache-key async locks
  |
  +-- Rate Limiter
  |     +-- Per-agent sliding windows
  |     +-- Remaining capacity and retry estimates
  |
  +-- Queue System
  |     +-- asyncio.Queue overload buffer
  |     +-- Background worker
  |
  +-- Metrics
        +-- Traffic, cache, queue, performance, and agent stats
  |
  v
AI Agents
  |
  +-- translator
  +-- coder
  +-- search
  +-- math
```

## Request Lifecycle

```text
Request
  |
  v
Validation
  |
  v
Cache Lookup
  |-- hit --> Return cached response
  |
  v
Acquire cache-key lock
  |
  v
Re-check cache
  |-- hit --> Return cached response
  |
  v
Rate Limiter Check
  |-- allowed --> Agent Processing --> Store Cache --> Metrics --> Response
  |
  v
Queue Request
  |
  v
Return queued response
  |
  v
Background Worker
  |
  v
Agent Processing --> Store Cache --> Metrics
```

## Features

- Async request handling with FastAPI.
- Intelligent in-memory cache for repeated prompts.
- Cache stampede prevention with per-key `asyncio.Lock` request coalescing.
- Configurable per-agent rate limiting.
- Queue-based resilience for over-limit requests.
- Background worker for gradual queue draining.
- Real-time metrics for traffic, cache, queue, performance, and agents.
- Health endpoint with warnings, bottlenecks, and recommendations.
- Modular services that can later map to Redis, distributed limiters, durable queues, or external metrics systems.

## Tech Stack

- FastAPI for the HTTP API.
- Python `asyncio` for non-blocking route and worker behavior.
- Pydantic models for request and response validation.
- In-memory cache for demo-friendly response reuse.
- In-memory sliding-window rate limiter.
- `asyncio.Queue` for lightweight overload buffering.
- Modular service files for maintainable backend architecture.

## Project Structure

```text
main.py           FastAPI app, request pipeline, lifecycle, and endpoints
agents.py         Async fake AI agents and agent registry
cache.py          In-memory cache plus request coalescing locks
limiter.py        Per-agent sliding-window rate limiter
queue_manager.py  asyncio.Queue manager and background worker infrastructure
stats.py          Centralized observability, metrics, and health evaluation
models.py         Pydantic API schemas
README.md         Project documentation and demo guide
```

## Setup

Install the minimal runtime dependencies:

```bash
pip install fastapi uvicorn
```

Start the API:

```bash
uvicorn main:app --host 0.0.0.0 --port 8000
```

Open the interactive API docs:

```text
http://localhost:8000/docs
```

## API Documentation

### Core Endpoints

| Method | Endpoint | Purpose |
| --- | --- | --- |
| `POST` | `/request` | Submit a request through cache, limiter, queue, agent processing, and metrics |
| `GET` | `/stats` | Full structured observability snapshot |
| `GET` | `/health` | Health status, warnings, bottlenecks, and recommendations |
| `GET` | `/metrics/summary` | Compact dashboard-friendly metrics summary |
| `GET` | `/limits` | Current per-agent limits, usage, retry windows, and remaining capacity |
| `GET` | `/queue/status` | Queue depth, worker state, processed count, and estimated wait time |
| `POST` | `/cache/clear` | Clear the in-memory response cache |
| `POST` | `/stats/reset` | Reset in-memory metrics without clearing cache, limiter, or queue state |

### Agent and Debug Endpoints

| Method | Endpoint | Purpose |
| --- | --- | --- |
| `GET` | `/agents` | List available fake agents |
| `GET` | `/agents/status` | Per-agent operational metrics |
| `GET` | `/agents/{agent_name}` | Agent metadata and request count |
| `GET` | `/debug/cache` | Cache internals for local review |
| `GET` | `/debug/limiter` | Rate limiter internals for local review |
| `GET` | `/debug/queue` | Queue internals for local review |
| `GET` | `/debug/agents` | Agent registry internals for local review |

## Example Requests and Responses

### 1. Submit an AI Request

```bash
curl -X POST http://localhost:8000/request \
  -H "Content-Type: application/json" \
  -d '{"agent":"coder","query":"Generate a Python loop"}'
```

Example response:

```json
{
  "agent": "coder",
  "response": "[CODER] Generated code:\n```python\nfor i in range(10):\n    print(i)\n```",
  "cached": false,
  "processing_time": 0.703
}
```

### 2. Repeat the Same Request

```bash
curl -X POST http://localhost:8000/request \
  -H "Content-Type: application/json" \
  -d '{"agent":"coder","query":"Generate a Python loop"}'
```

Example cached response:

```json
{
  "agent": "coder",
  "response": "[CODER] Generated code:\n```python\nfor i in range(10):\n    print(i)\n```",
  "cached": true,
  "processing_time": 0.0
}
```

### 3. Trigger Queue-Based Overload

Use unique prompts so the cache does not bypass the rate limiter:

```bash
for i in $(seq 1 8); do
  curl -s -X POST http://localhost:8000/request \
    -H "Content-Type: application/json" \
    -d "{\"agent\":\"coder\",\"query\":\"overload demo request $i\"}" &
done
wait
```

Example queued response:

```json
{
  "status": "queued",
  "request_id": "2bbda6dd-8a30-4c53-9505-cd2d5e6fb2b3",
  "agent": "coder",
  "queue_position": 2,
  "estimated_wait_time": 1.5,
  "queue_size": 2,
  "message": "Request accepted into the resilience queue.",
  "agent_limit_info": {
    "allowed": false,
    "agent": "coder",
    "limit": 3,
    "window_seconds": 10,
    "requests_in_window": 3,
    "remaining_requests": 0,
    "retry_after": 8.9
  }
}
```

### 4. Inspect Metrics

```bash
curl http://localhost:8000/stats
```

Example structure:

```json
{
  "traffic": {
    "total_requests": 12,
    "processed_requests": 4,
    "active_requests": 0,
    "queued_requests": 5,
    "processed_from_queue": 3,
    "rate_limited_requests": 5
  },
  "cache": {
    "cache_hits": 2,
    "cache_misses": 10,
    "cache_hit_ratio": 16.67
  },
  "queue": {
    "current_queue_size": 2,
    "max_queue_size": 100,
    "queue_overflow_count": 0,
    "average_queue_wait_time": 1.25
  },
  "performance": {
    "average_response_time": 0.61,
    "fastest_response_time": 0.0,
    "slowest_response_time": 0.91,
    "total_processing_time": 3.05
  },
  "agents": {
    "coder": {
      "request_count": 8,
      "cache_hits": 1,
      "rate_limited_count": 5,
      "average_processing_time": 0.7
    }
  }
}
```

### 5. Inspect Rate Limits

```bash
curl http://localhost:8000/limits
```

Example response:

```json
{
  "limits": {
    "coder": {
      "limit": 3,
      "window_seconds": 10,
      "requests_in_window": 3,
      "remaining_requests": 0,
      "retry_after": 8.9,
      "allowed": false,
      "agent": "coder"
    }
  }
}
```

### 6. Inspect Queue Status

```bash
curl http://localhost:8000/queue/status
```

Example response:

```json
{
  "queue_size": 2,
  "current_queue_size": 2,
  "max_queue_size": 100,
  "processing_count": 1,
  "worker_status": "running",
  "worker_running": true,
  "processed_queue_count": 3,
  "processed_from_queue": 3,
  "failed_queue_count": 0,
  "queue_overflow_count": 0,
  "estimated_wait_time": 1.5
}
```

### 7. Clear the Cache

```bash
curl -X POST http://localhost:8000/cache/clear
```

Example response:

```json
{
  "status": "success",
  "message": "Cache cleared. Removed 4 entries.",
  "cache_size_before": 4,
  "cache_size_after": 0
}
```

## Demo Flow

This sequence is designed for hackathon judges and pull request reviewers.

1. Start the server.

   ```bash
   uvicorn main:app --host 0.0.0.0 --port 8000
   ```

2. Send the first request.

   The first request is a cache miss. The fake agent simulates processing latency, and `/stats` records a cache miss plus processing time.

3. Send the exact same request again.

   The response returns from cache with `"cached": true`. This demonstrates compute savings and faster repeated requests.

4. Send several identical requests concurrently.

   Request coalescing ensures only the first cache miss performs agent work. Waiting requests reuse the cached response once it is written.

5. Send many unique requests to `coder`.

   The `coder` agent has a deliberately low default limit, so excessive unique traffic is queued instead of aggressively rejected.

6. Watch the queue stabilize.

   ```bash
   curl http://localhost:8000/queue/status
   curl http://localhost:8000/stats
   curl http://localhost:8000/health
   ```

   Queue size, processed queue count, rate-limited requests, cache metrics, active requests, and health recommendations update in real time.

## Copy-Paste Demo Script

Run this sequence after starting the server. It demonstrates the first slow request, the cached repeat, overload queueing, and live metrics.

```bash
# Reset demo-visible state.
curl -s -X POST http://localhost:8000/stats/reset
curl -s -X POST http://localhost:8000/cache/clear

# Demo 1: first request is processed by the fake agent.
curl -s -X POST http://localhost:8000/request \
  -H "Content-Type: application/json" \
  -d '{"agent":"translator","query":"hello"}'

# Demo 2: identical request returns from cache.
curl -s -X POST http://localhost:8000/request \
  -H "Content-Type: application/json" \
  -d '{"agent":"translator","query":"hello"}'

# Demo 3: unique coder requests exceed the per-agent limit and enter the queue.
for i in $(seq 1 6); do
  curl -s -X POST http://localhost:8000/request \
    -H "Content-Type: application/json" \
    -d "{\"agent\":\"coder\",\"query\":\"demo spike $i\"}" &
done
wait

# Demo 4: observe queue, limiter, and metrics behavior.
curl -s http://localhost:8000/queue/status
curl -s http://localhost:8000/limits
curl -s http://localhost:8000/stats
curl -s http://localhost:8000/health
```

## Default Rate Limits

| Agent | Limit |
| --- | --- |
| `translator` | 5 requests per 10 seconds |
| `coder` | 3 requests per 10 seconds |
| `search` | 8 requests per 10 seconds |
| `math` | 10 requests per 10 seconds |

These values make overload behavior easy to demonstrate locally while keeping the implementation simple.

## Observability Model

Metrics are grouped for API consumers and future dashboard integrations:

- Traffic: total requests, processed requests, active requests, queued requests, processed queue work, and rate-limited requests.
- Cache: hits, misses, and hit ratio.
- Queue: current size, max size, overflow count, and average wait estimate.
- Performance: average, fastest, slowest, and total processing time.
- Agents: per-agent request counts, cache hits, limiter pressure, and average processing time.

The `/health` endpoint converts those signals into a simple state:

- `healthy`: low pressure and no major bottlenecks.
- `warning`: queue or active request pressure is building.
- `overloaded`: queue capacity, active request count, or overflow signals indicate high pressure.

## Hackathon Narrative

Nasiko is production-inspired backend engineering in a compact demo:

- It treats AI agents as constrained resources.
- It prevents repeated work before reaching the agent layer.
- It smooths bursts with queue-based resilience.
- It explains system behavior with observability-first APIs.
- It uses clean module boundaries that reviewers can map to real infrastructure later.

The result is not just a fake AI endpoint. It is a traffic management layer for AI workloads.

## Scalability Path

The current implementation is intentionally in-memory. The architecture is modular so production-grade infrastructure can replace each component with minimal route changes:

- Redis cache: share cached responses across multiple API instances.
- Distributed limiter: move per-agent admission control to Redis or another atomic shared store.
- Kafka, RabbitMQ, Redis Streams, or Celery: replace `asyncio.Queue` with a durable queue and independent workers.
- Prometheus and Grafana: export `stats.py` metrics to production dashboards.
- Horizontal scaling: run multiple FastAPI instances behind a gateway once cache, limiter, and queue state are externalized.
- Persistent job status: store queued request status in a database or durable queue backend.
- Real agents: replace fake agents with LLM, tool, workflow, or LangChain-backed implementations.

## Contributor Notes

This repository intentionally avoids advanced infrastructure in the MVP. The goal is to make the resilience architecture visible and easy to review:

- Keep cache, limiter, queue, and stats logic isolated in their modules.
- Keep routes thin and focused on orchestration.
- Prefer async-friendly primitives.
- Avoid adding Redis, Kafka, databases, auth, or dashboards unless the project scope changes.
- Preserve demo clarity: cache hits, queue growth, limiter pressure, and health status should be easy to trigger locally.

## Future Improvements

- Environment-based configuration for limits, queue size, and cache size.
- TTL and LRU eviction for cache entries.
- Request status lookup for queued jobs.
- Retry policy and dead-letter handling for failed queued work.
- Structured logging with correlation IDs.
- Percentile latency metrics.
- Per-agent queue priorities.
- Authentication around debug and reset endpoints.
- External metrics exporter for Prometheus or OpenTelemetry.

## Summary

Nasiko demonstrates a practical resilience layer for AI systems: cache reusable work, coalesce duplicate requests, rate-limit constrained agents, queue overload, and expose operational behavior clearly. It is small enough for a hackathon demo and structured enough for maintainers to review, extend, and evolve.

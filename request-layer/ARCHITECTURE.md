# Architecture & Detailed Explanation

This document explains the internal mechanics of the **Nasiko Request Layer**, detailing how traffic flows from the client to the agent and back.

## 1. The Core Problem

In a multi-agent system, agents (LLMs or specific tools) are often slow or resource-constrained. If a sudden burst of requests arrives:
- Identical queries result in redundant, expensive compute.
- Too many concurrent requests will crash or timeout the agent.
- Simple rejection (429 Too Many Requests) forces the client to build complex retry logic.

The **Request Layer** solves this by acting as a protective proxy.

## 2. Request Flow Lifecycle

When a `POST /api/process` request arrives, it goes through four distinct stages:

### Stage A: Request Fingerprinting & Caching
1. We hash the `agent_name` and the `query` (along with any parameters) into a stable SHA-256 footprint.
2. We check Redis (`get: hash`).
3. **HIT:** If it exists, we return it instantly (0-5ms latency).
4. **MISS:** We proceed to the next stage.

### Stage B: Rate Limiting (Token Bucket)
Each agent has a configured `maxTokens` (burst) and `refillRate` (tokens per second).
1. We use a Lua script in Redis to atomically check and consume a token for the requested agent.
2. **TOKENS AVAILABLE:** We consume 1 token and proceed immediately.
3. **NO TOKENS:** We proceed to Stage C (Queuing).

### Stage C: Overflow Queuing
If an agent is at capacity, we don't drop the request.
1. The request is pushed to a BullMQ queue specific to that agent.
2. The Express route `awaits` the BullMQ job resolution.
3. A background worker consumes the queue strictly adhering to the agent's rate limit.
4. Once processed by the worker, the result is sent back to the awaiting Express route.

### Stage D: Proxy & Response
1. The request is forwarded via Axios to the real agent (via Kong Gateway at `:9100`) or intercepted by a built-in Mock Agent.
2. Upon success, the response is stored in Redis (Stage A) with a TTL.
3. The response is returned to the client.
4. Background metrics are updated and pushed to the Dashboard via Socket.io.

## 3. Metrics & Real-time Dashboard

The `StatsCollector` intercepts requests at various stages to record:
- **Throughput:** Total requests, successes, failures.
- **Cache Performance:** Hits vs Misses.
- **Latency:** Moving averages and P95 calculations.
- **Queue Depths:** Current waiting jobs per agent.

Every 2 seconds, the server broadcasts these aggregated statistics over a WebSocket (`socket.io`). The React dashboard consumes this stream to render live charts and a scrolling request feed, completely decoupling the observability from the core request path.

## 4. Scalability

Because all state (Cache, Limits, Queues) is backed by Redis:
- You can run multiple instances of the Request Layer Node.js server horizontally.
- The Token Bucket Lua script ensures atomic rate limiting across all instances.
- BullMQ naturally handles distributed job locking and processing.

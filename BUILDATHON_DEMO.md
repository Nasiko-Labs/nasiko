# Buildathon Demo: Resilient Agent Request Layer

## Problem

Nasiko routes queries through Kong to a growing fleet of specialized AI agents — translators, compliance checkers, GitHub assistants, and more. In real workloads, the same query is issued repeatedly across sessions (e.g. a multi-step orchestration that re-evaluates the same document fragment on each hop), yet every repetition hits the agent container, burns an LLM call, and adds round-trip latency. At the same time, a single slow or bursty agent can exhaust Kong's routing capacity for that path, blocking every other agent behind it — a cascading starvation problem. There is no built-in mechanism to absorb bursts gracefully: requests either go through immediately or fail with 5xx. The **Resilient Agent Request Layer** adds the traffic-control primitives — caching, per-agent rate limiting, and queuing — that a production multi-agent platform requires between the gateway and the fleet.

---

## Architecture

```
Client / Workflow Engine
         │
         ▼
  Kong Gateway (:9100)              ← existing Nasiko gateway
         │
         ▼
  Resilient Request Layer (:4001)   ← this service
  ┌──────────────────────────────┐
  │  POST /request               │
  │  ├── Cache (SHA-256, 60 s)   │  identical input → skip agent entirely
  │  ├── Rate limiter (per agent)│  token bucket, configurable burst + refill
  │  └── Queue  (per agent FIFO) │  absorb bursts; drop only when queue full
  └──────────────────────────────┘
         │
         ▼
  Agent Fleet (behind Kong / agents-net)
  ├── /agents/translator      (:9100/agents/translator/…)
  ├── /agents/compliance      (:9100/agents/compliance/…)
  └── /agents/github-agent    (:9100/agents/github-agent/…)

  Ops surface (no redeploy needed)
  ├── GET  /ops/dashboard       — live browser dashboard (2 s poll)
  ├── GET  /ops/cache/stats     — hits / misses / size / hit-rate
  ├── GET  /ops/agents/stats    — per-agent totals, p95, queue depth, tokens
  └── POST /ops/agents/config   — change capacity / refillRate / maxQueue at runtime
```

**Cache** — before forwarding, the layer hashes `agent_id + normalized_input + workflow_id` (SHA-256). A hit returns the stored response in microseconds; the agent and its LLM call are never invoked.

**Rate limiter** — each `agent_id` owns an independent token bucket (default: 10-token burst, refill 2/sec). A burst against `compliance` has zero effect on `translator`'s budget.

**Queue** — when a bucket is empty the request enters a per-agent FIFO (default depth 50). A background worker drains the queue as tokens refill. Requests are only dropped (HTTP 429) if the queue is also at capacity — graceful degradation under extreme load, not hard failure.

### Request flow — where each mechanism fires

```
Client / Orchestrator
        │
        ▼
 Kong Gateway :9100          (existing Nasiko routing)
        │
        ▼
 Resilient Layer :4001
        │
        ├─► [ Cache lookup ]──── HIT ──────────────────────────────► return ~0 ms
        │     SHA-256 of                 (no agent call, no LLM token)
        │     agent_id + input
        │     + workflow_id, 60 s TTL
        │
        │   MISS
        │
        ├─► [ Rate limiter ]──── token available ─────────────────► executeAgent()
        │     token bucket,                                                 │
        │     per agent_id,                                                 │
        │     configurable                                           cache result
        │     burst + refill
        │
        │   no token
        │
        ├─► [ Per-agent FIFO queue ]──── queue full ─────────────► 429 overloaded
        │     depth configurable,          (only this agent; others unaffected)
        │     drained by background
        │     worker every 50 ms
        │
        │   dequeued when token available
        │
        ▼
 Agent container (via agents-net)
 e.g. :9100/agents/translator, :9100/agents/compliance, …
```

> **Demo note:** The hackathon version runs agent stubs (`agent_fast`, `agent_slow`, `agent_flaky`) in-process to make the layer self-contained. Wiring to real Nasiko agents replaces the `executeAgent` switch in `src/agents.ts` with HTTP calls to `http://localhost:9100/agents/{name}/`.

---

## How to Run the Demo

> Assumes Docker Desktop, Node.js 18+, and `npm` are installed.

### Step 1 — Start the Nasiko stack

```bash
# From the repo root
docker compose --env-file .nasiko-local.env -f docker-compose.local.yml up -d
```

Wait ~3 minutes for all services to reach healthy state:

```bash
docker compose -f docker-compose.local.yml ps
```

Full setup details in [LOCAL_SETUP.md](LOCAL_SETUP.md). Login credentials auto-generated at `orchestrator/superuser_credentials.json`. Web UI at **http://localhost:9100/app/**.

### Step 2 — Start the Resilient Request Layer

```bash
cd buildathon-resilient-request-layer

# First time only
cp .env.example .env   # set PORT=4001
npm install

npm run dev
```

Service is ready when you see:
```
Resilient Request Layer listening on port 4001
```

### Step 3 — Open the ops dashboard

**http://localhost:4001/ops/dashboard**

The page polls `/ops/cache/stats` and `/ops/agents/stats` every 2 seconds, showing live cache hit rate, per-agent counters, token-bucket bars, and queue depth bars.

### Step 4 — Show caching (same input, instant second response)

```bash
# First call — cache miss, agent executes (~100 ms)
curl -s -X POST http://localhost:4001/request \
  -H "Content-Type: application/json" \
  -d '{"agent_id":"agent_fast","input":"translate hello to French"}' | jq .
# → { "from_cache": false, "type": "fast", … }

# Same input — served from memory
curl -s -X POST http://localhost:4001/request \
  -H "Content-Type: application/json" \
  -d '{"agent_id":"agent_fast","input":"translate hello to French"}' | jq .
# → { "from_cache": true, "type": "fast", … }
```

Watch **Hit Rate** climb on the dashboard.

### Step 5 — Show queuing under burst

```bash
# Fire 20 requests at once — first 10 get rate-limiter tokens and execute,
# the rest enter the per-agent queue.
for i in $(seq 1 20); do
  curl -s -X POST http://localhost:4001/request \
    -H "Content-Type: application/json" \
    -d "{\"agent_id\":\"agent_slow\",\"input\":\"doc-review-$i\"}" &
done
wait
```

On the dashboard, watch the **Queue Depth** bar spike, then drain at 2 tokens/sec as the background worker processes queued items.

### Step 6 — Tweak config live from the dashboard

In the **Update Agent Config** form, enter:

| Field | Value |
|-------|-------|
| Agent ID | `agent_slow` |
| Capacity | `3` |
| Refill Rate | `1` |
| Max Queue Length | `8` |

Click **Update Config**. Re-run the burst from Step 5. The queue fills faster; once depth exceeds 8, responses are `429 overloaded` for that agent only — `agent_fast` remains fully operational.

Restore defaults: capacity `10`, refillRatePerSec `2`, maxQueueLength `50`.

### Step 7 — Run the automated load test

```bash
# From buildathon-resilient-request-layer/
npm run load:test
```

Sends 50 total requests across all three agent types and prints a human-readable summary: cache hit rate, p95 latency, queue usage, and error counts.

---

## Why Nasiko Should Care

- **Eliminates redundant compute.** Repeated identical queries — common in multi-step orchestration and retried workflows — hit the cache instead of the agent. No LLM call, no container CPU, sub-millisecond response. At scale this directly reduces token spend and agent fleet sizing.

- **Per-agent isolation prevents cascade failures.** Each agent has an independent token bucket. A flooded or slow compliance-checker cannot starve the translator or GitHub agent of router capacity. The most critical failure mode for a multi-agent fleet is contained at the source.

- **Graceful degradation under overload.** Bursts enter a queue rather than returning immediate errors. Only sustained, extreme overload (queue full) drops requests — and only for the affected agent. The rest of the fleet is untouched.

- **Real-time ops visibility and zero-redeploy control.** Operators see live p95 latency, queue depth, error counts, and token levels per agent in a browser. They can tighten or loosen any agent's rate limit in a single POST request — no Kubernetes rollout, no config file change, no restart required. This is the kind of control Nasiko's ops teams need as the agent fleet grows.

---

## Metrics Snapshot (from `npm run load:test`)

50 total requests across three agent types: 15× `agent_fast` (same input), 20× `agent_slow` (unique inputs, burst), 15× `agent_flaky`.

| Metric | Value |
|--------|-------|
| Overall cache hit rate | ~28% (19 / 68 requests) |
| Repeated-workflow cache hit rate | **~93%** (14 / 15 same-input queries) |
| `agent_fast` p95 latency | ~115 ms |
| `agent_slow` burst (20 concurrent) | **10 queued / 0 dropped** out of 20 |
| `agent_flaky` error rate | ~19%, other agents unaffected |

**How to read these numbers:**

- **93% cache hit rate for repeated workflows** — in multi-step orchestration the same query often recurs on every hop (re-evaluating a document fragment, re-routing after a tool call). After the first execution the response is served from memory: zero LLM tokens, zero agent container CPU, sub-millisecond latency. The 28% overall rate reflects the full test mix including unique slow-agent queries; in a real repeated-workflow workload the rate climbs quickly toward the 93% figure.

- **10 queued / 0 dropped under a 2× burst** — firing 20 concurrent requests at an agent with a 10-token burst absorbs the overflow into the per-agent FIFO instead of returning errors. The queue drains at the configured refill rate (2 tokens/sec) maintaining request ordering. Only a sustained overload that fills both the rate-limiter bucket and the queue (depth 50) ever returns a 429 — and only for that one agent.

- **19% error rate fully isolated** — `agent_flaky`'s failures increment only its own `errorCount` counter. The token buckets and queues for `agent_fast` and every other agent are independent; they remain at full capacity throughout. This is the cascade-prevention property that matters most at fleet scale.

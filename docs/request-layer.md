# Request Management Layer — Operator Guide

This document is the runbook for operators of `nasiko-request-layer`, an
opt-in request-control service that sits between Kong and the agent
fleet. It implements the Buildthon *Resilient Agent Request Layer*
brief: caching, rate limiting, queueing, and operational visibility
unified into a single layer.

For a code-level overview, see
[`agent-gateway/request_layer/README.md`](../agent-gateway/request_layer/README.md).

## When to enable it

Turn it on when one or more of these is true:

- An agent is being hammered with repeated or near-identical queries
  (typical of integration testing, web scraping, viral usage spikes).
- Per-agent LLM spend is unpredictable and you want a dollar-denominated
  ceiling rather than only a request-per-second one.
- You want a single dashboard showing live cost saved, hit rate, and
  queue depth — without integrating a third-party CDN.

## Architecture

```
client → Kong (9100)
            │
            ├─→ /agents/translator/*  (default route, agent direct)
            │
            └─→ /agents/translator/*  (opted-in route, via the layer)
                                        │
                                        ▼
                                   nasiko-request-layer (8090)
                                        │
                                        │  L0 normalize
                                        │  L1 exact cache         ── request-layer-redis
                                        │  L2 semantic cache      ── request-layer-redis (HNSW)
                                        │  L3 router cache (opt)  ── request-layer-redis (HNSW)
                                        │  L4 coalesce            ── request-layer-redis (pubsub)
                                        │  L5 rate gate + queue   ── request-layer-redis
                                        │  L6 forward             ── Kong → agent
                                        │  L7 cache fill          ── request-layer-redis
                                        │  L8 Phoenix span        ── phoenix-observability
                                        ▼
                                   agent container
```

The `request-layer-redis` service in `docker-compose.local.yml` is a
dedicated `redis/redis-stack-server` instance — independent from the
main `redis` service that the rest of Nasiko uses.

## Bring it up

The layer is part of `docker-compose.local.yml`:

```sh
docker compose -f docker-compose.local.yml --env-file .nasiko-local.env up -d \
  nasiko-request-layer request-layer-redis
curl http://localhost:8090/health
```

Expected response:

```json
{
  "status": "healthy",
  "model_loaded": true,
  "redis_connected": true,
  "adapter": "nasiko",
  "agents": 1,
  "router_cache_enabled": false
}
```

The first start is slow (~30-60s) because the embedding model warm-up
downloads ~80MB. Subsequent starts are <5s.

## Opt one agent in

Existing Kong routes point at agent containers directly (e.g.
`/agents/translator/*` → `http://agent-translator:8000`). To put a route
behind the layer, change its upstream to the request-layer service:

```sh
# Find the route ID
curl http://localhost:9101/services/agent-translator/routes

# Patch the route's service to point at the layer
curl -X PATCH http://localhost:9101/services/agent-translator \
  -d "url=http://nasiko-request-layer:8090"
```

That's it. The layer receives the request as `/translator/<path>` and
forwards through Kong to `http://kong-gateway:8000/agents/translator/<path>`,
which the registry-managed route still resolves to the agent. To revert,
PATCH the upstream back to `http://agent-translator:8000`.

For local testing without touching Kong, you can also call the layer
directly:

```sh
curl -X POST http://localhost:8090/translator/translate \
  -H "Content-Type: application/json" \
  -d '{"text": "hello", "target": "fr"}'
```

## Optional: enable the router-decision cache

The L3 layer caches the *router service's* output — the
`(query → agent_url)` decision — so a recognized intent skips the router
LLM call entirely. It is off by default because false positives misroute
traffic (rather than just returning a stale response).

```sh
docker compose -f docker-compose.local.yml stop nasiko-request-layer
REQUEST_LAYER_ROUTER_CACHE_ENABLED=true \
  docker compose -f docker-compose.local.yml up -d nasiko-request-layer
```

Then point Kong's `/router/route` route at
`http://nasiko-request-layer:8090` to give the layer first crack at
routing decisions before the router service is consulted. On a miss, the
layer returns a `404` with `X-Cache-Fallthrough: router` so Kong can be
configured to retry against the upstream router.

## Observability

Every cache decision emits structured spans visible at
`http://localhost:6006` under the `nasiko-request-layer` project:

- `cache.hit` — attributes: `cache.layer`, `cache.similarity`,
  `cache.matched_query`, `cache.savings_usd`, `cache.savings_ms`,
  `cache.router_skipped`
- `coalesce.follower` — when a request is held while another runs
- `queue.entry` / `queue.exit` — when a request is gated

The admin API exposes the same data over HTTP:

| Endpoint | Purpose |
| --- | --- |
| `GET  /admin/stats` | aggregate counters |
| `GET  /admin/stream` | Server-Sent Events for cache decisions |
| `GET  /admin/policies` | inferred + override policies per agent |
| `PATCH /admin/policies/{agent}` | override one or more fields |
| `POST /admin/cache/clear` | flush a layer (and optionally one agent) |
| `GET  /admin/queue/{agent}` | queue depth + predicted wait time |
| `GET  /admin/recommendations` | self-tuning suggestions (advisory) |

## Policy inference

When the layer discovers a new agent in the registry, it infers a
sensible default policy from the AgentCard's `capabilities` and
`skills[].tags`:

| Bucket | TTL | Threshold |
| --- | --- | --- |
| translation, summarization, static_analysis | 24h | 0.92 |
| compliance, policy_review, code_review | 1h | 0.95 |
| weather, stock_price, news, realtime | 5m | 0.97 |
| search | 30m | 0.96 |
| (default) | 10m | 0.95 |

Cost cap is $5/min if the agent carries the `expensive` tag, else $1/min.

Operators can override any field; overrides persist in Redis at the hash
key `reqlayer:policy:overrides`.

## Failure modes

The layer is designed so failures degrade rather than break:

- **Embedding model unavailable** — semantic cache gracefully misses;
  exact cache still works.
- **`request-layer-redis` down** — `/health` returns `degraded`. Kong
  routes configured against the layer will return 502, so flip them back
  to the agent containers directly.
- **Phoenix collector unavailable** — spans are dropped, requests still
  succeed.
- **Embedding latency spike** — does not block the event loop (the model
  runs on a thread pool); requests time out at the configured forward
  timeout.
- **Stale router-cache entries after AgentCard change** — the registry
  poller hashes the manifest set every 60s and flushes the routing
  cache on change.

## Roadmap

The current layer ships:

- L0 normalize, L1 exact cache, L2 semantic cache, L3 router cache (opt-in),
  L4 coalesce, L5 rate gate + cost meter + priority queue, L6 forward,
  L7 fill, L8 Phoenix annotation.
- Capability adapter for Nasiko (A2A AgentCard format).
- Read-only self-tuning recommendations.

Follow-up PRs to consider:

- **Capability adapters** for raw MCP `server.json`, generic OpenAPI tags,
  and Google A2A. The `agentcard.NasikoAdapter` interface is already
  narrow enough that this is mostly mechanical.
- **Cross-agent dependency caching** for A2A chains. Today each agent's
  cache is independent.
- **Auto-applying recommendations.** Currently advisory only.
- **A bundled web UI** for the admin endpoints. Today operators consume
  them with `curl` / `jq` or build their own dashboard against
  `/admin/stream`.

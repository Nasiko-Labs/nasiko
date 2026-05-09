# RARL — Resilient Agent Request Layer

Part of the Nasiko AI Agent Platform.

## What is RARL?

RARL (Resilient Agent Request Layer) is a FastAPI-based sidecar service that sits between Kong Gateway and AI agent containers to provide:

- **Redis caching** with SHA-256 cache keys (avoid repeated agent calls)
- **Single-flight coalescing** (merge duplicate concurrent requests → 1 upstream call)
- **Per-agent token bucket rate limiting** with bounded priority queues
- **Live metrics dashboard** with Chart.js (500ms SSE refresh)
- **Admin REST API** for runtime configuration without restarts

## Architecture

```
Client → Kong :9100 → RARL :8010 → Agent Container
                        ↓
                    Redis :6379/2
                        ↓
                  /dashboard + /admin/*
```

## Quick Start

From the `nasiko/` root:

```bash
# Start RARL alongside Nasiko
docker compose -f docker-compose.local.yml -f docker-compose.rarl.yml --env-file .nasiko-local.env up -d rarl

# Check health
curl http://localhost:8010/health

# Open dashboard
open http://localhost:8010/dashboard
```

## Key Endpoints

| Endpoint | Description |
|----------|-------------|
| `GET /health` | Service health check |
| `GET /dashboard` | Live Chart.js monitoring UI |
| `GET /admin/stream` | SSE metrics stream (500ms) |
| `GET /admin/agents` | List all agent lanes with stats |
| `GET /admin/agents/{id}` | Detail for one agent |
| `PUT /admin/agents/{id}/config` | Live retune (rps/burst/max_inflight) |
| `POST /admin/cache/purge` | Purge Redis cache |
| `GET /admin/cache/stats` | Cache hit/miss stats |
| `GET /admin/explain` | Human-readable request decisions |
| `POST /admin/spike-test` | Built-in load tester |
| `ANY /agents/{agent_id}/{path}` | Proxied agent requests |

## Demo

See `nasiko/rarl/tests/demo_load.py` for load testing, or use the dashboard's built-in spike test buttons.

## Configuration

Set via environment variables in `docker-compose.rarl.yml`:

- `REDIS_URL` — Redis connection (default: `redis://redis:6379/2`)
- `AGENT_BASE_URLS` — JSON map of agent_id → upstream URL
- `DEFAULT_RPS` — Token bucket rate (default: 10)
- `DEFAULT_BURST` — Token bucket burst (default: 20)
- `DEFAULT_MAX_INFLIGHT` — Concurrent requests per agent (default: 4)
- `DEFAULT_MAX_QUEUE` — Queue size per agent (default: 100)

## Tech Stack

- Python 3.12, FastAPI, asyncio
- Redis 5 (async client)
- httpx (async HTTP client)
- Chart.js (dashboard)
- SSE (Server-Sent Events)

## Known Limitations

- Single-instance only (in-process single-flight)
- No auth on `/admin` endpoints
- In-memory state (queue/lane stats reset on restart)

Built for the Nasiko Buildthon hackathon.

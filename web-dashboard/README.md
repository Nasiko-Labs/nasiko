# Nasiko Web Dashboard (Challenges 1 & 2)

React + TypeScript dashboards for the Nasiko buildathon:

| Challenge | Route | Feature |
|-----------|--------|---------|
| 1 | `/logs/` | Platform logs table + level filters |
| 2 | `/metrics/` | Per-agent stats + 24h charts |

## View in Nasiko (recommended)

With the full stack running:

- **Logs:** http://localhost:9100/logs/
- **Metrics:** http://localhost:9100/metrics/

Same Kong host as the main app (`/app/`). Use the top nav to switch between Logs and Metrics.

**Requires:** Phoenix observability running and agent traffic for metrics charts.

## Prerequisites

```bash
docker compose -f docker-compose.local.yml --env-file .nasiko-local.env up -d --build nasiko-log-dashboard kong-service-registry
```

Superuser credentials: `orchestrator/superuser_credentials.json`

## Local dev (optional)

```bash
cd web-dashboard
npm install
npm run dev
```

Open http://localhost:3000 — Vite proxies `/api` and `/auth` to Kong at `http://localhost:9100`.

To mimic the gateway path locally:

```bash
VITE_BASE_PATH=/logs/ npm run dev
# → http://localhost:3000/logs/
```

## API

- `GET /api/v1/platform/logs?level=INFO&limit=100` (Bearer JWT)
- `POST /api/v1/platform/logs` (ingest, optional)

## Troubleshooting

**Browser console: `404` or `502` on `/logs/assets/...`**

Kong may cache an old Docker IP after `nasiko-log-dashboard` is recreated. Fix:

```bash
docker compose -f docker-compose.local.yml --env-file .nasiko-local.env up -d --build nasiko-log-dashboard kong-service-registry
docker restart kong-gateway
```

Then hard-refresh: **http://localhost:9100/logs/** (Cmd+Shift+R).

Verify:

```bash
curl -s -o /dev/null -w '%{http_code}\n' http://localhost:9100/logs/
curl -s -o /dev/null -w '%{http_code}\n' http://localhost:9100/logs/assets/index-B37-cChB.js
```

Both should print `200`. Direct container access: http://localhost:3001/

## Notes for reviewers

The shipped Nasiko web UI (`nasiko-web`) is a compiled Flutter bundle without React source in this repo. Challenges 1–2 add **React/TypeScript pages** at **`/logs`** and **`/metrics`** on the Kong gateway.

**Challenge 2 PR tip:** If Challenge 1 is not merged yet, either stack this branch on `feat/challenge-1-logging-dashboard` or rebase onto `main` after C1 merges.

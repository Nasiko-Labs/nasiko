# Nasiko Platform Logs Dashboard (Challenge 1)

React + TypeScript dashboard for **Buildathon Challenge 1**: view platform logs in a table with timestamps and filter by `INFO`, `WARNING`, or `ERROR`.

## View in Nasiko (recommended)

With the full stack running, open:

**http://localhost:9100/logs/**

This is served through Kong on the same gateway as the main web UI (`/app/`). API calls use `/api` and `/auth` on the same origin.

Link back to the main Nasiko app: **http://localhost:9100/app/**

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

The shipped Nasiko web UI (`nasiko-web`) is a compiled Flutter bundle without React source in this repo. Challenge 1 adds a **React/TypeScript page** integrated at **`/logs`** on the Kong gateway alongside **`/app`**.

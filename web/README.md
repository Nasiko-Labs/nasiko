# Nasiko Agent Performance Metrics (Titan Challenge 2)

React/TypeScript metrics UI for the Nasiko Titan Builder Challenge. The official Nasiko control-plane UI ships as a **compiled Flutter Docker image** (`nasiko-web`); this folder is the **editable** web source for Challenge 2.

## Run locally

```bash
cd web
npm install
# Point proxy at Kong (default)
set NASIKO_API_URL=http://localhost:9100/api/v1
npm run dev
```

Open http://localhost:4001 and paste your Nasiko bearer token (`nasiko login`).

## Docker (with full stack)

```bash
docker compose -f docker-compose.local.yml --env-file .nasiko-local.env up -d nasiko-metrics-web
```

Metrics UI: http://localhost:4001

## APIs used

- `GET /api/v1/registry/user/agents`
- `GET /api/v1/observability/session/list?start_time=...`
- `GET /api/v1/observability/agent/{id}/stats?start_time=...`

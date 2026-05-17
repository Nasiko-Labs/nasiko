# Nasiko Metrics Dashboard

Hackathon **Challenge 2** — per-agent performance metrics (latency, success/error, uptime, 24h chart).

**Status:** API + UI working on http://localhost:3003 (Phases B & C complete).

## Features

- Agent selector (Nasiko registry)
- KPI cards: avg response time, success, errors, uptime %
- 24h latency line chart (Recharts)
- Loading, error, no-data, and auth states
- Dark theme aligned with Nasiko shell

## Development

```bash
npm install
cp .env.example .env.local
npm run dev
```

Open **http://localhost:3003**

### Auth (important for port 3003)

The main Nasiko app runs on **port 9100**; metrics dev runs on **3003**. Browser `localStorage` is not shared across ports.

1. Sign in at http://localhost:9100/app/
2. Open DevTools → Application → Local Storage → `localhost:9100`
3. Copy `access_token` from key `nasiko-credentials-{email}-{username}.json`
4. Paste on the metrics dashboard sign-in screen

When deployed behind Kong at `/app/metrics/` on the same host, the token is read automatically.

## Scripts

| Command | Purpose |
|---------|---------|
| `npm run dev` | Dev server on port 3003 |
| `npm run build` | Production build |
| `npm run test` | Vitest — rollup unit tests |
| `npm run smoke` | API smoke test (needs `NASIKO_BEARER_TOKEN`) |

## API smoke (curl)

```bash
curl http://localhost:3003/api/health

curl http://localhost:3003/api/agents \
  -H "Authorization: Bearer <access_token>"

curl "http://localhost:3003/api/metrics?agent=a2a-translator&window=24h" \
  -H "Authorization: Bearer <access_token>"
```

```bash
export NASIKO_BEARER_TOKEN="<access_token>"
npm run smoke
```

## Environment

| Variable | Description |
|----------|-------------|
| `NASIKO_API_URL` | Kong gateway (server, default `http://localhost:9100`) |
| `NEXT_PUBLIC_NASIKO_APP_URL` | Main app link (default `http://localhost:9100/app/`) |

## Architecture

```
Browser → /api/agents, /api/metrics (Next Route Handlers)
       → Nasiko API (Bearer forwarded)
       → Observability stats + sessions + Kong agent health
```

Vault docs: `vault/reference/metrics-dashboard-dev.md`

## Troubleshooting

| Issue | Fix |
|-------|-----|
| HTML 500 from API routes | `rm -rf .next && npm run dev` |
| Sign-in loop on :3003 | Paste token from :9100 credentials key |
| Empty metrics / `no_data` | Use agent in main UI to generate Phoenix traces |
| Agent chat 401 | Fix `.nasiko-local.env` LLM keys; redeploy agent |

## Production

Target: Kong route `/app/metrics/` (docker-compose wiring — Phase D).

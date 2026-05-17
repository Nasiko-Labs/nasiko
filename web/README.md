# Nasiko Agent Metrics Page

Standalone React metrics dashboard for Challenge 2.

## Run Locally

```powershell
cd web
python -m http.server 4000
```

Open `http://localhost:4000`.

## Docker

```powershell
docker build -t nasiko-web-metrics .
docker run --rm -p 4000:4000 nasiko-web-metrics
```

The page uses React 18 and Chart.js from pinned CDN URLs and does not require a local Node package install. `src/app.jsx` is the readable source; `src/app.js` is the browser-ready compiled file used by `index.html`.

## Telemetry Mode

The dashboard is hybrid:

- It uses live Nasiko observability data when an auth token is available and `/api/v1/observability/session/list` returns sessions.
- It falls back to bundled demo telemetry when the backend is unavailable, unauthenticated, or empty.
- The badge shows `Live telemetry` or `Demo telemetry`.

Optional API override:

```js
localStorage.setItem("nasiko_api_base_url", "http://localhost:9100/api/v1")
```

## Nasiko Assistant

The bottom-right `Nasiko Assistant` represents Nasiko inside the Challenge 2 dashboard. It can answer normal questions, explain Nasiko, and discuss the current dashboard data, agents, uptime, errors, latency, telemetry mode, and judge demo talking points.

For a Cursor SDK-backed agent, run the server-side bridge:

```powershell
cd web\cursor-agent-bridge
$env:CURSOR_API_KEY="your_cursor_api_key_here"
docker compose up --build
```

The dashboard auto-checks `http://127.0.0.1:8787/metrics-agent`. When the bridge is ready, the widget shows `Cursor bridge ready`.

Optional endpoint override:

```js
localStorage.setItem("nasiko_cursor_agent_endpoint", "http://localhost:8787/metrics-agent")
```

The browser sends `{ question, context, source }` to that endpoint. Keep `CURSOR_API_KEY` only on the server side; the static dashboard falls back to local metrics context when the bridge is not configured, unavailable, or missing the key.

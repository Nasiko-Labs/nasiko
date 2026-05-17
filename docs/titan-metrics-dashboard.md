# Titan Metrics Dashboard

Nasiko's local stack now includes a Dockerized metrics dashboard for the Titan Builder Challenge.

## Run

```bash
docker compose -f docker-compose.local.yml --env-file .nasiko-local.env up -d --build
```

Open:

```text
http://localhost:9100/metrics
```

## Data Source

The dashboard calls:

```text
GET /api/v1/observability/agent-metrics?window_hours=24
```

The endpoint aggregates the agents available to the authenticated user with Phoenix observability data. Each agent row includes average latency, P50/P99 latency, success/error counts, uptime percentage, last activity, and hourly request buckets for the last 24 hours.

If Phoenix is unavailable for one agent, the API keeps that agent in the response with a zero-state row and an error message so the UI can still render the rest of the fleet.

## Demo Flow

1. Start Nasiko with Docker Compose.
2. Deploy `agents/a2a-translator.zip` from the app.
3. Start a translator session and send a few prompts.
4. Open `http://localhost:9100/metrics`.
5. The dashboard signs in with `orchestrator/superuser_credentials.json` automatically and refreshes the metric data.

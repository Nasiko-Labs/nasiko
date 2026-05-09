# Nasiko Buildthon — Demo Playbook

> **Goal:** Show judges a live, working resilient agent layer in under 10 minutes.  
> Every command below is copy-paste ready. Run them top to bottom.

---

## Prerequisites Checklist

```bash
# 1. Docker is running
docker info | grep "Server Version"

# 2. Env file exists with API keys
cat .nasiko-local.env | grep -E "OPENAI|ROUTER_LLM"

# 3. Stack is up
docker compose -f docker-compose.local.yml --env-file .nasiko-local.env ps
```

All services should show `Up (healthy)`. If not, bring the stack up first:

```bash
docker compose -f docker-compose.local.yml --env-file .nasiko-local.env up -d
```

---

## Step 0 — Get a Token

Every router request needs a JWT. Log in once and export it.

```bash
# Option A — via CLI (if nasiko CLI is installed)
nasiko login
TOKEN=$(nasiko auth token)

# Option B — via curl directly
TOKEN=$(curl -s -X POST http://localhost:8082/auth/login \
  -H "Content-Type: application/json" \
  -d '{"email":"admin@nasiko.com","password":"admin123"}' \
  | python3 -c "import sys,json; print(json.load(sys.stdin).get('token',''))")

echo "Token: ${TOKEN:0:40}..."   # should print first 40 chars
```

---

## Step 1 — Open the Live Dashboard

Open this URL in your browser **before** sending any requests:

```
http://localhost:8081/dashboard/
```

You should see:
- Status badge: `HEALTHY`
- All KPI counters at zero (`—`)
- Empty event stream
- Empty agent health cards

> The dashboard polls every 1 second. Leave it open. Everything you do in the
> next steps will appear here in real time.

---

## Step 2 — Deploy the Traffic Agent

The traffic agent simulates realistic LLM latency (0.8–2.5 seconds) so caching
is visibly useful in seconds.

```bash
# Option A — via the Nasiko web UI
# Go to http://localhost:4000 → Agents → Deploy → nasiko-traffic-agent

# Option B — via CLI
nasiko agent deploy agents/nasiko-traffic-agent/

# Verify the agent is running
docker ps --filter name=agent- --format "table {{.Names}}\t{{.Status}}"
```

Wait for it to appear in the dashboard **Agent Health** cards before continuing.

---

## Step 3 — Cache Demo (The "Wow" Moment)

Run the **same query three times**. The first call is slow; calls 2 and 3 return
from cache in under 10ms.

```bash
QUERY="Analyze revenue trend for Q3 2025"

for i in 1 2 3; do
  echo "--- Call $i ---"
  time curl -s -X POST http://localhost:9100/router \
    -H "Authorization: Bearer $TOKEN" \
    -F "session_id=demo-cache" \
    -F "query=$QUERY" \
    | python3 -c "
import sys, json
lines = sys.stdin.read().strip().split('\n')
last = json.loads(lines[-1])
print('Agent:', last.get('agent_id', '—'))
print('Message:', last.get('message','')[:80])
"
  echo ""
done
```

**What to narrate while it runs:**

| Call | Expected time | Dashboard event |
|------|--------------|-----------------|
| 1st | ~1–2 seconds | `CACHE MISS` (grey) + `AGENT SELECTED` (purple) + `REQUEST COMPLETED` (blue) |
| 2nd | < 10ms | `CACHE HIT` (green) — no agent call at all |
| 3rd | < 10ms | `CACHE HIT` (green) |

> Point to the event stream: "Call 1 went all the way to the agent. Calls 2 and 3
> never left the router — served from Redis in under 10 milliseconds."

---

## Step 4 — Rate Limit Burst Demo

Fire 80 concurrent requests to show the queue and rate-limiting layer.

```bash
echo "Firing 80 concurrent requests..."
for i in $(seq 1 80); do
  curl -s -X POST http://localhost:9100/router \
    -H "Authorization: Bearer $TOKEN" \
    -F "session_id=burst-$i" \
    -F "query=Query variant $((i % 5))" \
    > /dev/null 2>&1 &
done
wait
echo "Done. Check the dashboard."
```

**What to watch on the dashboard:**

- `QUEUED` events (amber) — requests waiting in the Redis sorted-set queue
- `RATE LIMITED` events (red) — requests that timed out and were rejected with a `Retry-After`
- Queue depth KPI spikes then drains as the sliding window resets
- Agent health score stays high because the agent itself isn't overloaded — the queue absorbs the pressure

> "The rate limiter isn't just dropping requests — it's queuing them with
> exponential backoff and serving them as capacity opens up. Only requests that
> can't be served within the timeout window are rejected."

---

## Step 5 — Read the Impact Metrics

```bash
curl -s http://localhost:8081/monitoring/impact | python3 -c "
import sys, json
d = json.load(sys.stdin)
print(f\"Cache coverage:         {d['cache_coverage_percent']:.1f}%\")
print(f\"LLM calls saved:        {d['llm_calls_saved']}\")
print(f\"Compute saved (est):    {d['compute_saved_estimate_ms']:,.0f} ms\")
print(f\"Avg agent latency:      {d['avg_latency_uncached_ms']:.0f} ms\")
print(f\"Avg cache latency:      {d['avg_latency_cached_ms']:.0f} ms\")
print(f\"Total requests seen:    {d['total_requests']}\")
"
```

**Sample output:**

```
Cache coverage:         68.4%
LLM calls saved:        95
Compute saved (est):    173,430 ms
Avg agent latency:      1824 ms
Avg cache latency:      8 ms
Total requests seen:    139
```

> "68% of requests were served without touching the agent. We saved ~173 seconds
> of compute in this demo alone — at scale that compounds into real cost savings."

---

## Step 6 — Ask the Observability Agent

Deploy the observability agent, then ask it questions in plain English through
the Nasiko chat interface.

```bash
# Deploy
nasiko agent deploy agents/nasiko-observability-agent/
```

Then open the web UI at `http://localhost:4000` and ask:

```
Why was the last request slow?
```

```
What has improved since caching was enabled?
```

```
Is the system under pressure right now?
```

```
How much compute has been saved today?
```

The agent fetches live data from `/monitoring/overview`, `/monitoring/impact`,
`/monitoring/agents/health`, and the last 5 events — then synthesizes a
plain-English answer using `gpt-4o-mini`.

> "This isn't a pre-scripted response. It's reading actual Redis metrics from
> this session and reasoning about them in real time."

---

## Step 7 — Show the Health Fallback (Optional, Advanced)

To demonstrate health-aware fallback routing, temporarily degrade the traffic
agent by hitting it with synthetic failures:

```bash
# Check current health score
curl -s http://localhost:8081/monitoring/agents/health | python3 -c "
import sys, json
d = json.load(sys.stdin)
for name, info in d.get('agents', {}).items():
    print(f\"{name}: score={info.get('health_score','?')} status={info.get('status','?')}\")
"
```

If a second agent is deployed and the primary's score drops below `0.3`, the
router automatically routes to the candidate and emits `FALLBACK TRIGGERED`
(orange) in the event stream.

---

## Full Automated Demo

All of the above in one shot:

```bash
bash demo/run_demo.sh $TOKEN
```

Then point judges to:

| URL | What it shows |
|-----|---------------|
| `http://localhost:8081/dashboard/` | Live event stream + charts |
| `http://localhost:8081/monitoring/impact` | JSON impact metrics |
| `http://localhost:8081/monitoring/events` | Raw Redis event log |
| `http://localhost:8081/monitoring/agents/health` | Per-agent health scores |
| `http://localhost:4000` | Full Nasiko web UI |

---

## Teardown (No More Stuck Networks)

When the demo is over, tear down cleanly — agent containers included:

```bash
# Removes deployed agent containers first, then brings the compose stack down
make down
```

This resolves the `app-network` / `agents-net` "Resource is still in use"
errors that occurred before. The Makefile now uses
`docker ps -aq --filter network=agents-net` to find and remove all agent
containers (running or stopped) before releasing the networks.

---

## Troubleshooting

| Symptom | Fix |
|---------|-----|
| Dashboard shows no events | Send a request first — events only appear after router activity |
| `TOKEN` is empty | Re-run Step 0; check auth service is healthy: `docker ps \| grep auth` |
| Agent not appearing in dashboard | Check it's running: `docker ps --filter name=agent-` |
| `cache_hit` never appears | Confirm you're using the **exact same query string** in the same session |
| Networks stuck on `docker compose down` | Run `make stop-agents` first, or use `make down` |
| Router logs show import errors | Rebuild: `docker compose ... up -d --build --no-deps nasiko-router` |

### Quick Health Check

```bash
# All services
docker compose -f docker-compose.local.yml --env-file .nasiko-local.env ps

# Router specifically
curl -s http://localhost:8081/router/health
# Expected: {"status": "ok"}

# Full system overview
curl -s http://localhost:8081/monitoring/overview | python3 -m json.tool
```

---

## What the Judges Are Looking For

| Criterion | Where to point |
|-----------|----------------|
| **Caching works** | Dashboard: `CACHE HIT` event < 10ms vs `CACHE MISS` ~1.8s |
| **Rate limiting is real** | Dashboard: `QUEUED` / `RATE LIMITED` events during burst |
| **Metrics are honest** | `/monitoring/impact` — `cache_coverage_percent` not `compute_saved` |
| **Events are real** | `/monitoring/events` — reads directly from Redis, no inference |
| **Health scoring** | `/monitoring/agents/health` — formula visible in codebase |
| **AI observability** | Observability agent answers from live data, not hardcoded |
| **Clean teardown** | `make down` removes everything including agent containers |

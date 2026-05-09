# Resilient Agent Request Layer

This buildathon solution adds a resilient request-management layer to Nasiko's router so repeated requests are cached, agent overload is controlled with per-agent concurrency limits, and runtime traffic is visible through operational endpoints.

## What Was Built

The solution lives in the router path between the gateway and the agent fleet.

- Request-level response caching backed by Redis
- Per-agent concurrency limits enforced atomically via Redis Lua scripts, with queueing instead of immediate rejection
- Runtime metrics for cache hits, misses, queue depth, timeouts, and active traffic
- Control endpoints to inspect and update agent limits at runtime

## Why This Fits The Problem Statement

The challenge asked for one unified layer that handles:

1. Caching repeated requests before they hit agents
2. Per-agent rate limiting to prevent overload
3. Operational controls for cache, limits, and runtime stats

This implementation covers all three in the router service:

- Cache path: repeated requests are hashed and served from Redis
- Traffic control path: each agent gets a FIFO queue and a concurrency cap enforced atomically
- Observability path: `/router/stats` (with `stats_since` timestamp), `/router/controls/{agent_name}`, and `/router/cache/clear`

## Key Endpoints

- `GET /router/stats`
  - Returns cache metrics, queue metrics, per-agent runtime stats, and a `stats_since` timestamp showing when the process started collecting
- `GET /router/controls/{agent_name}`
  - Returns the live concurrency limit and queue state for one agent
- `PUT /router/controls/{agent_name}`
  - Updates `max_concurrent` for an agent
- `POST /router/cache/clear`
  - Clears cached responses before a fresh demo

## Demo Story For Judges

Use the `Translator Agent` that was already deployed from the quickstart guide as the target agent.

### Demo Part 1: Repeated Request Cache

1. Clear the cache
2. Send a router request such as `Translate 'hello world' to Hindi`
3. Send the exact same request again
4. Show that the second response returns `cache_hit: true`
5. Open `/router/stats` and point out:
   - `hits`
   - `writes`
   - `hit_rate`

### Demo Part 2: Overload Control

1. Set `Translator Agent` to `max_concurrent = 1`
2. Fire two overlapping requests through the router
3. Show that the second request is queued instead of immediately failing
4. If the burst exceeds queue patience, show the timeout as predictable overload handling rather than platform collapse
5. Open `/router/stats` and point out:
   - `queued_requests`
   - `queue_timeouts`
   - `active_requests`
   - per-agent stats

### Demo Part 3: Operational Controls

1. Open `GET /router/controls/Translator%20Agent`
2. Show the current concurrency limit and queue depth
3. Update the limit live with `PUT /router/controls/Translator%20Agent`
4. Re-run traffic and show the behavior changes immediately

The dashboard keeps live stats easy to view, but requires a bearer token for mutation actions such as clearing cache or updating agent limits.

## Suggested Judge Framing

Keep the explanation tight:

- "This layer prevents duplicate compute by caching repeated router requests."
- "It protects individual agents with per-agent concurrency caps and a queue."
- "It exposes runtime controls and measurable stats so operators can tune the system live."

## Metrics To Highlight

- Reduced repeated latency from cache hits
- Reduced duplicate processing from cache writes + hits
- Stable overload handling via queued traffic and bounded timeouts
- Operational visibility from live router stats

## Fast Demo Command

Run the scripted demo:

```powershell
python scripts/demo_resilient_request_layer.py
```

The script auto-discovers the registered agent name from `/router/stats` so it works even if the exact agent name varies between deployments.

If `python` is not on your PATH, use the bundled runtime or your virtual environment's Python executable.

## Submission Checklist

1. Rebuild the router container with the latest code
2. Run the scripted demo once before recording or presenting
3. Capture screenshots of:
   - cache hit response
   - `/router/stats`
   - `/router/controls/{agent_name}`
4. Commit changes on a clean feature branch
5. Push to your fork and open a PR

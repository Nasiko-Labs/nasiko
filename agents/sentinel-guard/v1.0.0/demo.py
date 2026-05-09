"""
Sentinel Guard — Demo Script for Buildathon Presentation.
Populates the dashboard with realistic data and demonstrates all features.
Run this WHILE the sentinel-guard server is running on port 8500.
"""

import asyncio
import json
import time
import httpx

BASE = "http://localhost:8500"

async def main():
    print("=" * 60)
    print("  SENTINEL GUARD — Live Demo")
    print("=" * 60)

    async with httpx.AsyncClient(timeout=30) as c:
        # ── 1. Health Check ────────────────────────────────────────
        print("\n[1] Health Check")
        r = await c.get(f"{BASE}/health")
        h = r.json()
        print(f"    Status: {h['status']}")
        print(f"    Cache model: {h['cache'].get('embedding_model')}")
        print(f"    Redis: {h['cache']['redis_entries']} entries")

        # ── 2. Populate cache with realistic agent queries ─────────
        print("\n[2] Simulating Agent Traffic (populating cache)...")

        queries = [
            ("translator", "Translate 'hello world' to French"),
            ("translator", "Translate 'goodbye' to Spanish"),
            ("translator", "How do you say 'thank you' in Japanese?"),
            ("github-agent", "List all open pull requests"),
            ("github-agent", "Show recent commits on main branch"),
            ("github-agent", "Create a new issue for bug tracking"),
            ("compliance", "Check GDPR compliance for user data export"),
            ("compliance", "Verify SOC2 requirements for API endpoints"),
            ("code-review", "Review this Python function for security"),
            ("code-review", "Analyze performance of database queries"),
        ]

        for agent, query in queries:
            # Store in cache (simulating agent responses)
            resp_data = {
                "result": f"[{agent}] Response for: {query}",
                "agent": agent,
                "timestamp": time.time()
            }
            await c.post(f"{BASE}/cache/store", json={
                "agent": agent, "query": query, "payload": resp_data
            })
            print(f"    Cached: [{agent}] {query[:45]}...")

        print(f"    >> Stored {len(queries)} responses in cache")

        # ── 3. Demo: Cache HIT (exact match) ──────────────────────
        print("\n[3] Cache HIT Demo (exact match)")
        t0 = time.time()
        r = await c.get(f"{BASE}/cache/check",
            params={"query": "Translate 'hello world' to French", "agent": "translator"})
        lat = (time.time() - t0) * 1000
        result = r.json()
        print(f"    Hit: {result['hit']}")
        print(f"    Latency: {lat:.1f}ms")

        # ── 4. Demo: Cache HIT (semantic / similar query) ─────────
        print("\n[4] Cache HIT Demo (semantic similarity)")
        t0 = time.time()
        r = await c.post(f"{BASE}/proxy", json={
            "agent": "translator",
            "query": "Translate the words 'hello world' into French"
        })
        lat = (time.time() - t0) * 1000
        result = r.json()
        print(f"    Source: {result.get('source', 'N/A')}")
        print(f"    Latency: {lat:.1f}ms")
        if result.get('source') == 'cache':
            print(f"    >> SEMANTIC CACHE HIT! Same meaning, different wording")

        # ── 5. Demo: Cache MISS (different query) ─────────────────
        print("\n[5] Cache MISS Demo (new query)")
        t0 = time.time()
        r = await c.get(f"{BASE}/cache/check",
            params={"query": "What is quantum computing?", "agent": "translator"})
        lat = (time.time() - t0) * 1000
        result = r.json()
        print(f"    Hit: {result['hit']}")
        print(f"    Latency: {lat:.1f}ms")
        print(f"    >> Correctly identified as NEW query")

        # ── 6. Demo: Rate Limiting ────────────────────────────────
        print("\n[6] Rate Limiting Demo")
        # Set a low limit for demo
        await c.put(f"{BASE}/config/rate-limit/demo-agent", json={"rpm": 3})
        print("    Set demo-agent limit to 3 RPM")

        for i in range(5):
            r = await c.get(f"{BASE}/rate/check/demo-agent")
            status = r.json()
            if status["allowed"]:
                # Record the request
                await c.post(f"{BASE}/proxy", json={
                    "agent": "demo-agent", "query": f"request {i+1}"
                })
                print(f"    Request {i+1}: ALLOWED (remaining: {status['remaining']})")
            else:
                print(f"    Request {i+1}: BLOCKED! retry_after={status.get('retry_after_ms')}ms")

        # ── 7. Demo: Request Queuing ──────────────────────────────
        print("\n[7] Queue Status")
        r = await c.get(f"{BASE}/queue/status")
        print(f"    Queues: {r.json()}")

        # ── 8. Full Stats ─────────────────────────────────────────
        print("\n[8] Runtime Statistics")
        r = await c.get(f"{BASE}/stats")
        stats = r.json()
        s = stats["summary"]
        print(f"    Total requests:    {s['total_requests']}")
        print(f"    Cache hits:        {s['total_cache_hits']}")
        print(f"    Cache misses:      {s['total_cache_misses']}")
        print(f"    Semantic hits:     {s['total_semantic_hits']}")
        print(f"    Hit rate:          {s['cache_hit_rate_pct']}%")
        print(f"    Avg hit latency:   {s['avg_cache_hit_latency_ms']:.1f}ms")
        print(f"    Total forwarded:   {s['total_forwarded']}")
        print(f"    Total rejected:    {s['total_rejected']}")

        # ── 9. Per-Agent breakdown ────────────────────────────────
        print("\n[9] Per-Agent Breakdown")
        for agent, data in stats.get("per_agent", {}).items():
            print(f"    {agent}: {data['total']} req, {data['hits']} hits, {data.get('hit_rate_pct', 0)}% hit rate")

        # ── 10. Dashboard link ────────────────────────────────────
        print("\n[10] Monitoring Dashboard")
        print(f"    >> Open http://localhost:8500/dashboard in your browser")

    print("\n" + "=" * 60)
    print("  DEMO COMPLETE")
    print("  Dashboard: http://localhost:8500/dashboard")
    print("  Stats API: http://localhost:8500/stats")
    print("=" * 60)


if __name__ == "__main__":
    asyncio.run(main())

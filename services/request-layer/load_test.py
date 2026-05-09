"""
Load test script for Nasiko request-layer.
Sends N concurrent requests to a target agent and reports latency + cache stats.

Usage:
    # Full load test (50 requests, 20 concurrent)
    python load_test.py --target http://localhost:8090/agents/a2a-translator --count 50

    # Quick demo (10 requests, shows cache + queue behaviour)
    python load_test.py --target http://localhost:8090/agents/a2a-translator --demo

    # Custom
    python load_test.py --target http://localhost:8090/agents/a2a-translator --count 30 --concurrency 10
"""
import argparse
import asyncio
import json
import random
import statistics
import time
import uuid

import httpx

# ── Query pool ───────────────────────────────────────────────────────────────
# Group A: Identical queries → should all be CACHE HIT after the first one
# Group B: Paraphrased queries → should be CACHE HIT via semantic similarity
# Group C: Unique queries → CACHE MISS each time
QUERIES_IDENTICAL = [
    "Translate 'Hello world' to Spanish",
    "Translate 'Hello world' to Spanish",
    "Translate 'Hello world' to Spanish",
]

QUERIES_PARAPHRASED = [
    "Convert 'Hello world' into Spanish",
    "How do you say 'Hello world' in Spanish?",
    "Please translate the phrase Hello world to Spanish language",
]

QUERIES_UNIQUE = [
    "Translate 'Good morning' to French",
    "Translate 'Thank you' to German",
    "Translate 'How are you?' to Japanese",
    "Translate 'Goodbye' to Italian",
    "Translate 'I love programming' to Portuguese",
    "Translate 'The weather is nice today' to Korean",
    "Translate 'Where is the nearest hospital?' to Chinese",
    "Translate 'Happy birthday' to Russian",
    "Translate 'Please help me' to Arabic",
    "Translate 'Good night' to Hindi",
]


def _make_payload(query: str) -> str:
    """Build an A2A JSON-RPC message/send payload."""
    return json.dumps({
        "jsonrpc": "2.0",
        "method": "message/send",
        "params": {
            "message": {
                "messageId": str(uuid.uuid4()),
                "role": "user",
                "parts": [{"kind": "text", "text": query}],
            }
        },
        "id": str(uuid.uuid4()),
    })


def _build_query_list(count: int, demo: bool) -> list[str]:
    """Build the list of queries to send."""
    if demo:
        # Demo mode: send queries that showcase all three cache behaviours
        queries = []
        # 1) First unique request (MISS)
        queries.append("Translate 'Hello world' to Spanish")
        # 2) Exact repeat (HIT)
        queries.append("Translate 'Hello world' to Spanish")
        # 3) Paraphrased (semantic HIT)
        queries.append("Convert 'Hello world' into Spanish")
        # 4) Another paraphrase (semantic HIT)
        queries.append("How do you say 'Hello world' in Spanish?")
        # 5-8) Unique queries (MISS each)
        queries.append("Translate 'Good morning' to French")
        queries.append("Translate 'Thank you' to German")
        queries.append("Translate 'Goodbye' to Italian")
        queries.append("Translate 'How are you?' to Japanese")
        # 9-10) Repeats of the unique ones (HIT)
        queries.append("Translate 'Good morning' to French")
        queries.append("Translate 'Thank you' to German")
        return queries

    # Normal mode: mix of all query types
    queries = []
    # 30% identical, 20% paraphrased, 50% unique
    for i in range(count):
        r = random.random()
        if r < 0.3:
            queries.append(random.choice(QUERIES_IDENTICAL))
        elif r < 0.5:
            queries.append(random.choice(QUERIES_PARAPHRASED))
        else:
            queries.append(random.choice(QUERIES_UNIQUE))
    return queries


HEADERS = {"Content-Type": "application/json"}


async def _send(client: httpx.AsyncClient, url: str, query: str, idx: int) -> dict:
    payload = _make_payload(query)
    start = time.monotonic()
    try:
        resp = await client.post(url, content=payload, headers=HEADERS, timeout=120)
        elapsed = (time.monotonic() - start) * 1000
        return {
            "idx": idx,
            "query": query[:50],
            "status": resp.status_code,
            "latency_ms": round(elapsed, 1),
            "cache": resp.headers.get("x-cache", "—"),
            "queued": resp.status_code == 202,
        }
    except Exception as exc:
        elapsed = (time.monotonic() - start) * 1000
        return {"idx": idx, "query": query[:50], "status": 0, "latency_ms": round(elapsed, 1), "error": str(exc)}


async def run(target: str, count: int, concurrency: int, demo: bool) -> None:
    queries = _build_query_list(count, demo)
    actual_count = len(queries)

    print(f"\n{'='*65}")
    print(f"  Nasiko Request-Layer Load Test")
    print(f"  Target      : {target}")
    print(f"  Requests    : {actual_count}  |  Concurrency: {concurrency}")
    print(f"  Mode        : {'DEMO' if demo else 'LOAD TEST'}")
    print(f"{'='*65}\n")

    results = []

    async with httpx.AsyncClient() as client:
        sem = asyncio.Semaphore(concurrency)

        async def bounded(idx, query):
            async with sem:
                return await _send(client, target, query, idx)

        wall_start = time.monotonic()
        tasks = [bounded(i, q) for i, q in enumerate(queries)]
        results = await asyncio.gather(*tasks)
        wall_elapsed = time.monotonic() - wall_start

    # ── Results ──────────────────────────────────────────────────────────
    success = [r for r in results if r["status"] in range(200, 300)]
    queued = [r for r in results if r.get("queued")]
    failed = [r for r in results if r["status"] == 0 or r["status"] >= 500]
    cached = [r for r in results if r.get("cache") == "HIT"]
    missed = [r for r in results if r.get("cache") == "MISS"]

    latencies = [r["latency_ms"] for r in results if r["status"] != 0]
    hit_latencies = [r["latency_ms"] for r in cached]
    miss_latencies = [r["latency_ms"] for r in missed]

    print(f"  +-----------------------------------------------------------+")
    print(f"  |  RESULTS                                                  |")
    print(f"  +-----------------------------------------------------------+")
    print(f"  |  Total requests    : {actual_count:<36}|")
    print(f"  |  Success (2xx)     : {len(success):<36}|")
    print(f"  |  Queued (202)      : {len(queued):<36}|")
    print(f"  |  Failed            : {len(failed):<36}|")
    print(f"  |  Cache HIT         : {len(cached):<36}|")
    print(f"  |  Cache MISS        : {len(missed):<36}|")
    pct = round(len(cached) / actual_count * 100) if actual_count > 0 else 0
    print(f"  |  Hit Rate          : {pct}%{' '*(34-len(str(pct)))}|")
    print(f"  +-----------------------------------------------------------+")
    if latencies:
        print(f"  |  LATENCY (all)                                            |")
        avg_l = round(statistics.mean(latencies), 1)
        med_l = round(statistics.median(latencies), 1)
        p99 = sorted(latencies)[int(len(latencies) * 0.99) - 1] if len(latencies) > 1 else latencies[0]
        print(f"  |    Avg             : {avg_l} ms{' '*(30-len(str(avg_l)))}|")
        print(f"  |    p50             : {med_l} ms{' '*(30-len(str(med_l)))}|")
        print(f"  |    p99             : {round(p99, 1)} ms{' '*(30-len(str(round(p99,1))))}|")
    if hit_latencies:
        avg_h = round(statistics.mean(hit_latencies), 1)
        print(f"  |  CACHE HIT latency : {avg_h} ms avg{' '*(26-len(str(avg_h)))}|")
    if miss_latencies:
        avg_m = round(statistics.mean(miss_latencies), 1)
        print(f"  |  CACHE MISS latency: {avg_m} ms avg{' '*(26-len(str(avg_m)))}|")
    print(f"  |  Wall time         : {round(wall_elapsed, 2)}s{' '*(33-len(str(round(wall_elapsed,2))))}|")
    print(f"  +-----------------------------------------------------------+\n")

    if demo:
        print("  Per-request detail:")
        print(f"  {'#':<4} {'Cache':<8} {'Latency':<12} {'Query'}")
        print(f"  {'----'} {'------'} {'-----------'} {'----------------------------------------'}")
        for r in sorted(results, key=lambda x: x["idx"]):
            cache_tag = "[HIT] " if r.get("cache") == "HIT" else "[MISS]"
            print(f"  {r['idx']:<4} {cache_tag:<8} {r['latency_ms']:>8.1f} ms  {r['query']}")
        print()


def main():
    parser = argparse.ArgumentParser(description="Nasiko request-layer load test")
    parser.add_argument("--target", default="http://localhost:8090/agents/a2a-translator", help="Full agent URL")
    parser.add_argument("--count", type=int, default=50, help="Total requests to send")
    parser.add_argument("--concurrency", type=int, default=20, help="Max concurrent requests")
    parser.add_argument("--demo", action="store_true", help="Demo mode: 10 curated requests showing cache behaviour")
    args = parser.parse_args()
    if args.demo:
        args.concurrency = 3  # Low concurrency so ordering is more visible
    asyncio.run(run(args.target, args.count, args.concurrency, args.demo))


if __name__ == "__main__":
    main()

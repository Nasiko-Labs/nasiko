"""Async load tester for RARL. Usage: python tests/demo_load.py --url URL [options]"""
import argparse
import asyncio
import json
import time
from pathlib import Path

import httpx


async def fire(
    client: httpx.AsyncClient,
    url: str,
    body: bytes | None,
    vary: bool,
    seq: int,
) -> dict:
    t0 = time.monotonic()
    try:
        payload = body
        if vary and body:
            try:
                parsed = json.loads(body)
                parsed["_seq"] = seq
                payload = json.dumps(parsed).encode()
            except (json.JSONDecodeError, UnicodeDecodeError):
                pass

        if payload:
            resp = await client.post(url, content=payload, headers={"Content-Type": "application/json"})
        else:
            resp = await client.get(url)

        latency_ms = (time.monotonic() - t0) * 1000
        return {
            "status": resp.status_code,
            "latency_ms": latency_ms,
            "x_cache": resp.headers.get("x-cache", "NONE"),
        }
    except Exception as e:
        return {
            "status": 0,
            "latency_ms": (time.monotonic() - t0) * 1000,
            "x_cache": "ERROR",
            "error": str(e),
        }


async def run(url: str, concurrency: int, n_requests: int, body: bytes | None, vary: bool) -> None:
    sem = asyncio.Semaphore(concurrency)
    results = []

    async with httpx.AsyncClient(timeout=60.0) as client:
        async def bounded(i: int) -> dict:
            async with sem:
                return await fire(client, url, body, vary, i)

        t_start = time.monotonic()
        results = await asyncio.gather(*[bounded(i) for i in range(n_requests)])
        total_time = time.monotonic() - t_start

    status_counts: dict[str, int] = {}
    cache_counts: dict[str, int] = {}
    latencies = []

    for r in results:
        k = str(r["status"])
        status_counts[k] = status_counts.get(k, 0) + 1
        xc = r["x_cache"]
        cache_counts[xc] = cache_counts.get(xc, 0) + 1
        latencies.append(r["latency_ms"])

    latencies.sort()
    n = len(latencies)

    def pct(p: float) -> float:
        return latencies[max(0, int(p * n) - 1)] if latencies else 0.0

    print(f"\n{'='*55}")
    print(f"  RARL Load Test Results — {url}")
    print(f"{'='*55}")
    print(f"  Total requests : {n_requests}")
    print(f"  Concurrency    : {concurrency}")
    print(f"  Wall-clock     : {total_time:.2f}s")
    print(f"  Throughput     : {n_requests / total_time:.1f} req/s")
    print(f"\n  Status codes   : {status_counts}")
    print(f"  X-Cache        : {cache_counts}")
    print(f"\n  Latency (ms):")
    print(f"    p50  : {pct(0.50):.1f}")
    print(f"    p95  : {pct(0.95):.1f}")
    print(f"    p99  : {pct(0.99):.1f}")
    print(f"    max  : {max(latencies):.1f}")
    print(f"{'='*55}\n")


def main() -> None:
    parser = argparse.ArgumentParser(description="RARL async load tester")
    parser.add_argument("--url", required=True, help="Target URL")
    parser.add_argument("--concurrency", type=int, default=100)
    parser.add_argument("--requests", type=int, default=100)
    parser.add_argument("--body", help="Path to JSON file to use as POST body")
    parser.add_argument("--vary", action="store_true", help="Vary body with unique _seq field per request")
    args = parser.parse_args()

    body: bytes | None = None
    if args.body:
        body = Path(args.body).read_bytes()

    asyncio.run(run(args.url, args.concurrency, args.requests, body, args.vary))


if __name__ == "__main__":
    main()

#!/usr/bin/env python3
from __future__ import annotations

import argparse
import concurrent.futures
import json
import time
import urllib.request
import uuid


def payload(text: str) -> bytes:
    return json.dumps(
        {
            "jsonrpc": "2.0",
            "id": str(uuid.uuid4()),
            "method": "message/send",
            "params": {
                "message": {
                    "role": "user",
                    "parts": [{"kind": "text", "text": text}],
                    "messageId": str(uuid.uuid4()),
                },
                "metadata": {},
            },
        }
    ).encode("utf-8")


def post(url: str, text: str, user: str) -> tuple[int, float, str]:
    req = urllib.request.Request(
        url,
        data=payload(text),
        headers={"Content-Type": "application/json", "X-Subject-ID": user},
        method="POST",
    )
    started = time.perf_counter()
    with urllib.request.urlopen(req, timeout=90) as resp:
        resp.read()
        return resp.status, (time.perf_counter() - started) * 1000, resp.headers.get("X-Request-Layer-Cache", "unknown").lower()


def main() -> None:
    parser = argparse.ArgumentParser(description="Fire concurrent identical misses to prove duplicate processing reduction.")
    parser.add_argument("--url", default="http://localhost:9100/agents/agent-demo-request-layer/")
    parser.add_argument("--text", default="Translate 'single flight protects expensive agents' to French")
    parser.add_argument("--user", default="demo-user")
    parser.add_argument("--concurrency", type=int, default=12)
    args = parser.parse_args()

    with concurrent.futures.ThreadPoolExecutor(max_workers=args.concurrency) as pool:
        futures = [pool.submit(post, args.url, args.text, args.user) for _ in range(args.concurrency)]
        samples = [future.result() for future in concurrent.futures.as_completed(futures)]

    hits = sum(1 for _, _, cache in samples if cache == "hit")
    misses = sum(1 for _, _, cache in samples if cache == "miss")
    print("Single-flight KPI")
    print(f"requests={len(samples)} cache_hits={hits} cache_misses={misses}")
    print(f"duplicate_processing_avoided≈{max(len(samples) - max(misses, 1), 0)}")
    for status, elapsed_ms, cache in sorted(samples, key=lambda item: item[1]):
        print(f"status={status} cache={cache} latency_ms={elapsed_ms:.1f}")


if __name__ == "__main__":
    main()

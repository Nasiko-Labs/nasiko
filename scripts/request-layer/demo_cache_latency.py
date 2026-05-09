#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
import statistics
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
        headers={
            "Content-Type": "application/json",
            "X-Subject-ID": user,
        },
        method="POST",
    )
    started = time.perf_counter()
    with urllib.request.urlopen(req, timeout=90) as resp:
        resp.read()
        elapsed_ms = (time.perf_counter() - started) * 1000
        return resp.status, elapsed_ms, resp.headers.get("X-Request-Layer-Cache", "unknown").lower()


def main() -> None:
    parser = argparse.ArgumentParser(description="Show cold vs cached latency for repeated agent requests.")
    parser.add_argument("--url", default="http://localhost:9100/agents/agent-demo-request-layer/")
    parser.add_argument("--text", default="Translate 'good morning, resilient agents' to Spanish")
    parser.add_argument("--user", default="demo-user")
    parser.add_argument("--runs", type=int, default=6)
    args = parser.parse_args()

    samples = [post(args.url, args.text, args.user) for _ in range(args.runs)]
    cold = samples[0][1]
    warm = [sample[1] for sample in samples[1:]]
    hit_count = sum(1 for _, _, cache in samples if cache == "hit")

    print("Cache latency KPI")
    for index, (status, elapsed_ms, cache) in enumerate(samples, start=1):
        print(f"run={index} status={status} cache={cache} latency_ms={elapsed_ms:.1f}")
    if warm:
        print(f"cold_ms={cold:.1f}")
        print(f"warm_avg_ms={statistics.mean(warm):.1f}")
        print(f"latency_reduction={(1 - (statistics.mean(warm) / cold)) * 100:.1f}%")
    print(f"cache_hit_rate={(hit_count / len(samples)) * 100:.1f}%")


if __name__ == "__main__":
    main()

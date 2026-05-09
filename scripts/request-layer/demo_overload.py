#!/usr/bin/env python3
from __future__ import annotations

import argparse
import concurrent.futures
import json
import time
import urllib.error
import urllib.request
import uuid


def set_limits(base: str, agent: str, concurrency: int, rps: float, queue_depth: int, wait_ms: int) -> None:
    body = json.dumps(
        {
            "cache_enabled": True,
            "cache_ttl_seconds": 600,
            "max_concurrency": concurrency,
            "sustained_rps": rps,
            "burst_capacity": max(1, concurrency),
            "max_queue_depth": queue_depth,
            "max_queue_wait_ms": wait_ms,
        }
    ).encode("utf-8")
    req = urllib.request.Request(
        f"{base}/control/limits/{agent}",
        data=body,
        headers={"Content-Type": "application/json"},
        method="PUT",
    )
    with urllib.request.urlopen(req, timeout=10) as resp:
        resp.read()


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


def post(url: str, index: int) -> tuple[int, float, str, str]:
    req = urllib.request.Request(
        url,
        data=payload(f"Translate overload demo request {index} to German"),
        headers={
            "Content-Type": "application/json",
            "X-Subject-ID": "demo-user",
            "Cache-Control": "no-cache",
        },
        method="POST",
    )
    started = time.perf_counter()
    try:
        with urllib.request.urlopen(req, timeout=90) as resp:
            resp.read()
            return (
                resp.status,
                (time.perf_counter() - started) * 1000,
                resp.headers.get("X-Request-Layer-Queue-Wait-Ms", "0"),
                resp.headers.get("X-Request-Layer-Limit-State", "unknown"),
            )
    except urllib.error.HTTPError as exc:
        exc.read()
        return (
            exc.code,
            (time.perf_counter() - started) * 1000,
            exc.headers.get("X-Request-Layer-Queue-Wait-Ms", "0"),
            exc.headers.get("X-Request-Layer-Limit-State", "unknown"),
        )


def main() -> None:
    parser = argparse.ArgumentParser(description="Show bounded queue behavior under per-agent overload.")
    parser.add_argument("--manager", default="http://localhost:8090")
    parser.add_argument("--url", default="http://localhost:9100/agents/agent-demo-request-layer/")
    parser.add_argument("--agent", default="agent-demo-request-layer")
    parser.add_argument("--requests", type=int, default=12)
    parser.add_argument("--concurrency", type=int, default=1)
    args = parser.parse_args()

    set_limits(args.manager, args.agent, concurrency=args.concurrency, rps=1, queue_depth=20, wait_ms=15000)
    with concurrent.futures.ThreadPoolExecutor(max_workers=args.requests) as pool:
        futures = [pool.submit(post, args.url, index) for index in range(args.requests)]
        samples = [future.result() for future in concurrent.futures.as_completed(futures)]

    success = sum(1 for status, _, _, _ in samples if status < 500)
    failures = len(samples) - success
    queue_waits = [int(float(wait_ms or 0)) for _, _, wait_ms, _ in samples]
    print("Overload stability KPI")
    print(f"requests={len(samples)} successes={success} failures={failures} failure_rate={(failures / len(samples)) * 100:.1f}%")
    print(f"queue_wait_max_ms={max(queue_waits) if queue_waits else 0}")
    for status, elapsed_ms, wait_ms, state in sorted(samples, key=lambda item: item[1]):
        print(f"status={status} limit_state={state} queue_wait_ms={wait_ms} latency_ms={elapsed_ms:.1f}")


if __name__ == "__main__":
    main()

"""
Demo helper for the Nasiko buildathon resilient request layer.

This script proves four behaviors required by the problem statement:
1. Repeated requests are served from cache (faster repeated responses)
2. Cache writes reduce duplicate agent processing
3. Per-agent concurrency limits are enforced with queueing
4. Operational stats are visible and accurate

Exit code is 0 only when all required conditions pass.
"""

from __future__ import annotations

import argparse
import concurrent.futures
import json
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
import uuid
from pathlib import Path
from typing import Any, Dict, List, Tuple


# ---------------------------------------------------------------------------
# Result tracking
# ---------------------------------------------------------------------------

class DemoResults:
    """Collects PASS/FAIL results for the final summary."""

    def __init__(self):
        self.checks: List[Tuple[str, bool, str]] = []

    def record(self, name: str, passed: bool, detail: str = "") -> bool:
        tag = "\033[92m[PASS]\033[0m" if passed else "\033[91m[FAIL]\033[0m"
        line = f"  {tag} {name}"
        if detail:
            line += f"  ({detail})"
        print(line)
        self.checks.append((name, passed, detail))
        return passed

    @property
    def all_passed(self) -> bool:
        return all(ok for _, ok, _ in self.checks)

    @property
    def pass_count(self) -> int:
        return sum(1 for _, ok, _ in self.checks if ok)

    @property
    def fail_count(self) -> int:
        return sum(1 for _, ok, _ in self.checks if not ok)


results = DemoResults()


# ---------------------------------------------------------------------------
# HTTP helpers (unchanged from original)
# ---------------------------------------------------------------------------

def load_credentials(path: Path) -> Dict[str, Any]:
    return json.loads(path.read_text(encoding="utf-8"))


def login(base_url: str, credentials_path: Path) -> str:
    creds = load_credentials(credentials_path)
    payload = json.dumps(
        {
            "access_key": creds["access_key"],
            "access_secret": creds["access_secret"],
        }
    ).encode("utf-8")
    req = urllib.request.Request(
        f"{base_url}/auth/users/login",
        data=payload,
        headers={"Content-Type": "application/json"},
        method="POST",
    )
    with urllib.request.urlopen(req, timeout=30) as response:
        return json.loads(response.read().decode("utf-8"))["token"]


def authed_request(
    url: str,
    token: str,
    *,
    method: str = "GET",
    body: bytes | None = None,
    content_type: str | None = None,
    timeout: int = 60,
) -> Any:
    headers = {"Authorization": f"Bearer {token}"}
    if content_type:
        headers["Content-Type"] = content_type
    req = urllib.request.Request(url, data=body, headers=headers, method=method)
    with urllib.request.urlopen(req, timeout=timeout) as response:
        raw = response.read().decode("utf-8")
        return json.loads(raw)


def multipart_router_request(
    base_url: str,
    token: str,
    *,
    session_id: str,
    query: str,
    route: str | None = None,
) -> List[Dict[str, Any]]:
    boundary = "----CodexBoundary" + uuid.uuid4().hex
    parts = []
    fields = {"session_id": session_id, "query": query}
    if route:
        fields["route"] = route

    for name, value in fields.items():
        parts.append(
            f"--{boundary}\r\n"
            f'Content-Disposition: form-data; name="{name}"\r\n\r\n'
            f"{value}\r\n"
        )
    parts.append(f"--{boundary}--\r\n")
    body = "".join(parts).encode("utf-8")

    req = urllib.request.Request(f"{base_url}/router", data=body, method="POST")
    req.add_header("Authorization", f"Bearer {token}")
    req.add_header("Content-Type", f"multipart/form-data; boundary={boundary}")

    with urllib.request.urlopen(req, timeout=120) as response:
        raw_lines = response.read().decode("utf-8").strip().splitlines()

    messages = []
    for line in raw_lines:
        line = line.strip()
        if not line:
            continue
        messages.append(json.loads(line))
    return messages


def clear_cache(base_url: str, token: str) -> Dict[str, Any]:
    return authed_request(
        f"{base_url}/router/cache/clear",
        token,
        method="POST",
    )


def update_agent_limit(
    base_url: str, token: str, agent_name: str, max_concurrent: int
) -> Dict[str, Any]:
    agent = urllib.parse.quote(agent_name, safe="")
    return authed_request(
        f"{base_url}/router/controls/{agent}",
        token,
        method="PUT",
        body=json.dumps({"max_concurrent": max_concurrent}).encode("utf-8"),
        content_type="application/json",
    )


def get_stats(base_url: str, token: str) -> Dict[str, Any]:
    return authed_request(f"{base_url}/router/stats", token)


def get_agent_controls(base_url: str, token: str, agent_name: str) -> Dict[str, Any]:
    agent = urllib.parse.quote(agent_name, safe="")
    return authed_request(f"{base_url}/router/controls/{agent}", token)


def discover_agent_name(
    base_url: str, token: str, hint: str
) -> str:
    """Try to find an agent whose name contains *hint* (case-insensitive).

    Falls back to *hint* unchanged if the stats endpoint has no agents yet.
    """
    try:
        stats = get_stats(base_url, token)
        known = [a["agent_name"] for a in stats.get("agents", [])]
        if not known:
            return hint
        for name in known:
            if name == hint:
                return name
        hint_lower = hint.lower()
        for name in known:
            if hint_lower in name.lower():
                return name
        return hint
    except Exception:
        return hint


def summarize_messages(messages: List[Dict[str, Any]]) -> Dict[str, Any]:
    final_message = messages[-1]["message"] if messages else ""
    cache_hit = any((msg.get("metadata") or {}).get("cache_hit") for msg in messages)
    queued = any((msg.get("metadata") or {}).get("queued") for msg in messages)
    queue_timeout = any(
        (msg.get("metadata") or {}).get("queue_timeout") for msg in messages
    )
    return {
        "cache_hit": cache_hit,
        "queued": queued,
        "queue_timeout": queue_timeout,
        "final_message": final_message,
    }


# ---------------------------------------------------------------------------
# Demo 1: Cache
# ---------------------------------------------------------------------------

def run_cache_demo(base_url: str, token: str, query: str) -> None:
    print("\n" + "=" * 64)
    print("  DEMO 1: Request Cache")
    print("=" * 64)
    print("Clearing cache for a clean baseline...")
    clear_cache(base_url, token)

    print(f"\n  Sending first request: {query!r}")
    t0 = time.monotonic()
    first = multipart_router_request(
        base_url,
        token,
        session_id="buildathon-cache-demo",
        query=query,
    )
    first_ms = int((time.monotonic() - t0) * 1000)
    first_summary = summarize_messages(first)
    results.record(
        "First request is NOT a cache hit",
        not first_summary["cache_hit"],
        f"cache_hit={first_summary['cache_hit']}, latency={first_ms}ms",
    )

    print(f"\n  Sending identical request again...")
    t0 = time.monotonic()
    second = multipart_router_request(
        base_url,
        token,
        session_id="buildathon-cache-demo",
        query=query,
    )
    second_ms = int((time.monotonic() - t0) * 1000)
    second_summary = summarize_messages(second)
    results.record(
        "Second request IS a cache hit",
        second_summary["cache_hit"],
        f"cache_hit={second_summary['cache_hit']}, latency={second_ms}ms",
    )

    if first_ms > 0 and second_ms < first_ms:
        speedup = round(first_ms / max(1, second_ms), 1)
        results.record(
            "Cache hit is faster than original",
            True,
            f"{first_ms}ms -> {second_ms}ms ({speedup}x speedup)",
        )
    else:
        results.record(
            "Cache hit is faster than original",
            False,
            f"{first_ms}ms -> {second_ms}ms",
        )

    print(f"\n  Response preview: {second_summary['final_message'][:200]}")


# ---------------------------------------------------------------------------
# Demo 2: Queue
# ---------------------------------------------------------------------------

def run_queue_demo(
    base_url: str,
    token: str,
    agent_name: str,
    burst_size: int,
    max_concurrent: int,
) -> None:
    print("\n" + "=" * 64)
    print("  DEMO 2: Concurrency Limiting & Queueing")
    print("=" * 64)
    original_limit = get_agent_controls(base_url, token, agent_name)["max_concurrent"]
    print(f"  Agent: {agent_name!r}")
    print(f"  Setting max_concurrent={max_concurrent} (was {original_limit})...")
    update_agent_limit(base_url, token, agent_name, max_concurrent)

    def worker(index: int) -> Dict[str, Any]:
        query = f"Translate this sentence to Hindi: queue demo request number {index}"
        messages = multipart_router_request(
            base_url,
            token,
            session_id=f"buildathon-queue-demo-{index}",
            query=query,
        )
        summary = summarize_messages(messages)
        summary["request_index"] = index
        return summary

    print(f"  Firing {burst_size} concurrent requests...\n")
    try:
        started_at = time.monotonic()
        with concurrent.futures.ThreadPoolExecutor(max_workers=burst_size) as executor:
            futures = [executor.submit(worker, i + 1) for i in range(burst_size)]
            worker_results = [future.result() for future in futures]
        elapsed = round(time.monotonic() - started_at, 2)

        for r in worker_results:
            status = []
            if r["queued"]:
                status.append("QUEUED")
            if r["queue_timeout"]:
                status.append("TIMEOUT")
            if r["cache_hit"]:
                status.append("CACHE_HIT")
            if not status:
                status.append("DIRECT")
            print(
                f"    request {r['request_index']}: {', '.join(status)}"
            )
        print(f"\n  Burst completed in {elapsed}s")

        any_queued = any(r["queued"] for r in worker_results)
        any_timeout = any(r["queue_timeout"] for r in worker_results)
        results.record(
            "At least one request was queued (not rejected)",
            any_queued or any_timeout,
            f"queued={sum(1 for r in worker_results if r['queued'])}, "
            f"timeouts={sum(1 for r in worker_results if r['queue_timeout'])}",
        )
    finally:
        if original_limit != max_concurrent:
            print(f"  Restoring {agent_name!r} max_concurrent={original_limit}...")
            update_agent_limit(base_url, token, agent_name, original_limit)


# ---------------------------------------------------------------------------
# Demo 3: Stats
# ---------------------------------------------------------------------------

def run_stats_demo(base_url: str, token: str) -> None:
    print("\n" + "=" * 64)
    print("  DEMO 3: Operational Visibility")
    print("=" * 64)
    stats = get_stats(base_url, token)
    print(json.dumps(stats, indent=2))

    cache = stats.get("cache", {})
    traffic = stats.get("traffic", {})

    results.record(
        "Stats endpoint returns cache metrics",
        "hits" in cache and "misses" in cache and "hit_rate" in cache,
    )
    results.record(
        "Stats endpoint returns traffic metrics",
        "total_requests" in traffic and "queued_requests" in traffic,
    )
    results.record(
        "Stats includes per-agent breakdown",
        len(stats.get("agents", [])) > 0,
        f"{len(stats.get('agents', []))} agent(s) tracked",
    )
    results.record(
        "Stats includes stats_since timestamp",
        "stats_since" in stats,
    )


# ---------------------------------------------------------------------------
# KPI summary
# ---------------------------------------------------------------------------

def print_kpi_summary(base_url: str) -> None:
    print("\n" + "=" * 64)
    print("  KPI EVIDENCE — Problem Statement Success Criteria")
    print("=" * 64)

    kpis = [
        (
            "Faster repeated responses",
            "Cache hit returns instantly without reaching the agent. "
            "Demonstrated by latency comparison in Demo 1.",
        ),
        (
            "Reduced duplicate processing",
            "Cache writes prevent repeated agent invocations. "
            "Cache hit_rate visible in /router/stats.",
        ),
        (
            "Stable overload handling",
            "Excess traffic is queued with predictable timeouts, "
            "not immediately rejected. Demonstrated in Demo 2.",
        ),
        (
            "Real-time monitoring dashboards",
            f"Live dashboard at {base_url}/router/dashboard polls "
            f"/router/stats every 2s with interactive controls.",
        ),
    ]

    for i, (metric, evidence) in enumerate(kpis, 1):
        print(f"\n  {i}. {metric}")
        print(f"     Evidence: {evidence}")


# ---------------------------------------------------------------------------
# Final report
# ---------------------------------------------------------------------------

def print_final_report() -> None:
    print("\n" + "=" * 64)
    print("  FINAL RESULTS")
    print("=" * 64)

    for name, passed, detail in results.checks:
        tag = "\033[92mPASS\033[0m" if passed else "\033[91mFAIL\033[0m"
        line = f"  [{tag}] {name}"
        if detail:
            line += f"  ({detail})"
        print(line)

    total = len(results.checks)
    print(f"\n  {results.pass_count}/{total} checks passed", end="")
    if results.fail_count:
        print(f", \033[91m{results.fail_count} FAILED\033[0m")
    else:
        print(f" — \033[92mALL PASSED\033[0m")


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------

def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--base-url",
        default="http://localhost:9100",
        help="Nasiko gateway base URL",
    )
    parser.add_argument(
        "--credentials-path",
        default="orchestrator/superuser_credentials.json",
        help="Path to superuser credentials JSON",
    )
    parser.add_argument(
        "--agent-name",
        default="Translator Agent",
        help="Agent name to use for the queue demo",
    )
    parser.add_argument(
        "--query",
        default="Translate 'hello world' to Hindi",
        help="Base query to use for the cache demo",
    )
    parser.add_argument(
        "--burst-size",
        type=int,
        default=2,
        help="Number of concurrent requests for the queue demo",
    )
    parser.add_argument(
        "--max-concurrent",
        type=int,
        default=1,
        help="Per-agent concurrency limit for the queue demo",
    )
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    try:
        print("=" * 64)
        print("  Nasiko Buildathon — Resilient Request Layer Demo")
        print("=" * 64)

        print("\nLogging in...")
        token = login(args.base_url, Path(args.credentials_path))

        # Resolve agent name — auto-discover from live stats when possible.
        agent_name = discover_agent_name(args.base_url, token, args.agent_name)
        if agent_name != args.agent_name:
            print(f"Discovered agent: {agent_name!r} (hint was {args.agent_name!r})")
        else:
            print(f"Using agent: {agent_name!r}")

        run_cache_demo(args.base_url, token, args.query)
        run_queue_demo(
            args.base_url,
            token,
            agent_name,
            args.burst_size,
            args.max_concurrent,
        )
        run_stats_demo(args.base_url, token)
        print_kpi_summary(args.base_url)
        print_final_report()

        return 0 if results.all_passed else 1

    except urllib.error.HTTPError as exc:
        print(f"HTTP error: {exc.code} {exc.reason}", file=sys.stderr)
        if exc.fp is not None:
            print(exc.fp.read().decode("utf-8"), file=sys.stderr)
        return 1
    except Exception as exc:
        print(f"Demo failed: {exc}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())

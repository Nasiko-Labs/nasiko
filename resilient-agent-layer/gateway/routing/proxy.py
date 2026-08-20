import time
import asyncio
from typing import Any, Dict, Optional
import httpx

from gateway.cache.cache_manager import cache_manager
from gateway.rate_limiter.token_bucket import rate_limiter
from gateway.queue.request_queue import queue_manager, QueuedRequest
from gateway.config import load_agent_fleet


# Metrics
_request_counts: Dict[str, int] = {}
_cache_bypass_counts: Dict[str, int] = {}
_latencies: Dict[str, list] = {}  # agent_id -> list of latencies in ms
_error_counts: Dict[str, int] = {}

AGENT_FLEET: Dict[str, dict] = {}


def init_fleet():
    global AGENT_FLEET
    AGENT_FLEET = load_agent_fleet()
    for agent_id, cfg in AGENT_FLEET.items():
        rate_limiter.configure_agent(
            agent_id,
            rps=cfg.get("rate_limit_rps", 10),
            burst=cfg.get("burst", 20),
        )
        queue_manager.get_or_create(
            agent_id,
            max_size=cfg.get("queue_max", 100),
            timeout_ms=cfg.get("queue_timeout_ms", 5000),
        )
        queue_manager.start_worker(agent_id, _forward_to_agent_raw)


def get_agent_url(agent_id: str) -> Optional[str]:
    cfg = AGENT_FLEET.get(agent_id)
    return cfg["base_url"] if cfg else None


async def _forward_to_agent_raw(request: QueuedRequest) -> dict:
    """
    Directly forward a request to the agent (bypasses rate limiter — already rate-limited).
    Used by queue workers.
    """
    return await _call_agent(request.agent_id, request.payload, request.headers)


async def _call_agent(agent_id: str, payload: Any, headers: dict) -> dict:
    """Make the actual HTTP call to the downstream agent."""
    cfg = AGENT_FLEET.get(agent_id, {})
    base_url = cfg.get("base_url", f"http://{agent_id}:8000")
    url = f"{base_url}/invoke"

    start = time.perf_counter()
    try:
        async with httpx.AsyncClient(timeout=30.0) as client:
            # Forward request with original headers stripped of hop-by-hop
            forward_headers = {
                k: v for k, v in headers.items()
                if k.lower() not in ("host", "content-length", "transfer-encoding")
            }
            resp = await client.post(url, json=payload, headers=forward_headers)
            resp.raise_for_status()
            data = resp.json()

    except httpx.HTTPStatusError as e:
        _error_counts[agent_id] = _error_counts.get(agent_id, 0) + 1
        raise
    except Exception as e:
        _error_counts[agent_id] = _error_counts.get(agent_id, 0) + 1
        raise

    elapsed_ms = (time.perf_counter() - start) * 1000
    _latencies.setdefault(agent_id, []).append(elapsed_ms)
    # Keep only last 1000 samples
    if len(_latencies[agent_id]) > 1000:
        _latencies[agent_id] = _latencies[agent_id][-1000:]

    return data


async def handle_request(
    agent_id: str,
    payload: Any,
    headers: dict,
    bypass_cache: bool = False,
    priority: int = 0,
) -> tuple[dict, str]:
    """
    Main entrypoint for all agent requests.
    Returns (response_dict, source) where source is 'cache' | 'agent' | 'queue'.
    """
    _request_counts[agent_id] = _request_counts.get(agent_id, 0) + 1

    # 1. Cache check
    if not bypass_cache:
        cached = await cache_manager.get(agent_id, payload)
        if cached is not None:
            return cached, "cache"
    else:
        _cache_bypass_counts[agent_id] = _cache_bypass_counts.get(agent_id, 0) + 1

    cfg = AGENT_FLEET.get(agent_id, {})
    ttl = cfg.get("cache_ttl", 60)

    # 2. Rate limit check
    allowed = await rate_limiter.is_allowed(agent_id)

    if allowed:
        # 3a. Within rate limit — call agent directly
        result = await _call_agent(agent_id, payload, headers)
        if not bypass_cache:
            await cache_manager.set(agent_id, payload, result, ttl=ttl)
        return result, "agent"
    else:
        # 3b. Over rate limit — enqueue
        loop = asyncio.get_event_loop()
        future = loop.create_future()
        req = QueuedRequest(
            agent_id=agent_id,
            payload=payload,
            headers=headers,
            future=future,
            priority=priority,
        )
        queue = queue_manager.get_or_create(agent_id)
        enqueued = await queue.enqueue(req)

        if not enqueued:
            raise RuntimeError(f"Queue full for agent '{agent_id}'")

        # Wait for queue worker to process
        result = await asyncio.wait_for(
            future,
            timeout=(queue.timeout_ms / 1000) + 1.0  # +1s grace
        )
        if not bypass_cache:
            await cache_manager.set(agent_id, payload, result, ttl=ttl)
        return result, "queue"


def get_proxy_stats() -> dict:
    stats = {}
    for agent_id, latencies in _latencies.items():
        if latencies:
            sorted_l = sorted(latencies)
            n = len(sorted_l)
            stats[agent_id] = {
                "total_requests": _request_counts.get(agent_id, 0),
                "errors": _error_counts.get(agent_id, 0),
                "cache_bypasses": _cache_bypass_counts.get(agent_id, 0),
                "latency_p50_ms": round(sorted_l[int(n * 0.50)], 2),
                "latency_p95_ms": round(sorted_l[int(n * 0.95)], 2),
                "latency_p99_ms": round(sorted_l[min(int(n * 0.99), n - 1)], 2),
                "latency_avg_ms": round(sum(sorted_l) / n, 2),
            }
    return {
        "total_requests": sum(_request_counts.values()),
        "total_errors": sum(_error_counts.values()),
        "per_agent": stats,
    }

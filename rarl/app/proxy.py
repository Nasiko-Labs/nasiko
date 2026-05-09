import base64
import uuid
from typing import Any

import httpx
from fastapi import Request
from starlette.responses import Response

from app.cache import CACHEABLE_METHODS, make_cache_key
from app.config import get_settings

_client: httpx.AsyncClient | None = None

DROP_REQ_HEADERS = {"host", "content-length"}
DROP_RESP_HEADERS = {"content-encoding", "transfer-encoding", "content-length"}


def _get_client() -> httpx.AsyncClient:
    global _client
    if _client is None:
        _client = httpx.AsyncClient(timeout=30.0)
    return _client


def _build_response(result: dict[str, Any], extra_headers: dict[str, str]) -> Response:
    body_raw = result["body"]
    if result.get("body_b64"):
        content = base64.b64decode(body_raw)
    else:
        content = body_raw.encode("utf-8") if isinstance(body_raw, str) else body_raw

    headers = {**result["headers"], **extra_headers}
    return Response(
        content=content,
        status_code=result["status"],
        headers=headers,
        media_type=result["headers"].get("content-type"),
    )


async def _do_fetch(
    method: str,
    base_url: str,
    path: str,
    headers: dict,
    body: bytes,
    query_params,
) -> dict[str, Any]:
    client = _get_client()
    url = f"{base_url}/{path}" if path else base_url
    resp = await client.request(
        method, url, headers=headers, content=body, params=query_params
    )
    resp_headers = {
        k: v for k, v in resp.headers.items() if k.lower() not in DROP_RESP_HEADERS
    }
    try:
        body_str = resp.content.decode("utf-8")
        return {"status": resp.status_code, "body": body_str, "headers": resp_headers, "body_b64": False}
    except UnicodeDecodeError:
        return {
            "status": resp.status_code,
            "body": base64.b64encode(resp.content).decode("ascii"),
            "headers": resp_headers,
            "body_b64": True,
        }


async def handle_request(agent_id: str, path: str, request: Request) -> Response:
    settings = get_settings()
    base_url = settings.agent_base_urls.get(agent_id)
    if base_url is None:
        return Response(
            content=b'{"error":"unknown agent"}',
            status_code=404,
            media_type="application/json",
        )

    request_id = uuid.uuid4().hex[:12]
    method = request.method
    body = await request.body()
    query = str(request.query_params)
    req_headers = {
        k: v for k, v in request.headers.items() if k.lower() not in DROP_REQ_HEADERS
    }

    cacheable = method in CACHEABLE_METHODS
    cache_key = make_cache_key(agent_id, method, path, query, body, req_headers) if cacheable else ""

    decision: dict[str, Any] = {
        "request_id": request_id,
        "agent_id": agent_id,
        "cache": "NOT_CACHEABLE",
        "queue_position": 0,
        "eta": 0.0,
    }

    async def fetch_upstream() -> dict[str, Any]:
        return await _do_fetch(method, base_url, path, req_headers, body, request.query_params)

    was_from_cache = False
    was_leader = True
    meta: dict[str, Any] = {"queue_position": 0, "eta_seconds": 0.0, "wait_seconds": 0.0}
    result: dict[str, Any]

    if cacheable:
        cached = await request.app.state.cache.get(cache_key)
        if cached is not None:
            decision["cache"] = "HIT"
            _record_decision(request, decision)
            request.app.state.metrics.record_request(
                agent_id, was_cache_hit=True, was_coalesced=False,
                client_latency=0.0, upstream_latency=None
            )
            return _build_response(cached, {"X-Cache": "HIT", "X-RARL-Phase": "3-queue", "X-Request-Id": request_id})

    from app.main import get_or_create_lane
    from app.queue_lane import QueueOverflow

    priority = int(request.headers.get("X-Priority", "5"))
    lane = get_or_create_lane(request.app, agent_id)

    try:
        if cacheable:
            async def upstream_via_lane() -> dict[str, Any]:
                res, m = await lane.submit(priority, fetch_upstream)
                return {"result": res, "meta": m}

            wrapped, was_leader = await request.app.state.singleflight.do(cache_key, upstream_via_lane)
            result = wrapped["result"]
            meta = wrapped["meta"]

            if was_leader and 200 <= result["status"] < 300:
                await request.app.state.cache.set(cache_key, result, settings.default_ttl_seconds)

            cache_header = "MISS" if was_leader else "COALESCED"
            decision["cache"] = cache_header
        else:
            result, meta = await lane.submit(priority, fetch_upstream)
            cache_header = "BYPASS"
            decision["cache"] = cache_header

    except QueueOverflow:
        decision["cache"] = "QUEUE_OVERFLOW"
        _record_decision(request, decision)
        return Response(
            content=b'{"error":"queue_full","retry_after_seconds":5}',
            status_code=503,
            media_type="application/json",
            headers={"Retry-After": "5", "X-RARL-Phase": "3-queue"},
        )
    except httpx.TimeoutException:
        decision["cache"] = "UPSTREAM_TIMEOUT"
        _record_decision(request, decision)
        return Response(
            content=b'{"error":"upstream_timeout"}',
            status_code=504,
            media_type="application/json",
            headers={"X-RARL-Phase": "3-queue", "X-Request-Id": request_id},
        )
    except httpx.HTTPError as exc:
        decision["cache"] = "UPSTREAM_ERROR"
        _record_decision(request, decision)
        return Response(
            content=f'{{"error":"upstream_error","detail":"{exc}"}}'.encode(),
            status_code=502,
            media_type="application/json",
            headers={"X-RARL-Phase": "3-queue", "X-Request-Id": request_id},
        )

    decision["queue_position"] = meta["queue_position"]
    decision["eta"] = meta["eta_seconds"]
    _record_decision(request, decision)

    request.app.state.metrics.record_request(
        agent_id,
        was_cache_hit=was_from_cache,
        was_coalesced=not was_leader,
        client_latency=meta["wait_seconds"],
        upstream_latency=meta["wait_seconds"],
    )

    extra = {
        "X-Cache": cache_header if cacheable else "BYPASS",
        "X-Queue-Position": str(meta["queue_position"]),
        "X-Queue-ETA-Seconds": f"{meta['eta_seconds']:.3f}",
        "X-Queue-Wait-Seconds": f"{meta['wait_seconds']:.3f}",
        "X-RARL-Phase": "3-queue",
        "X-Request-Id": request_id,
    }
    return _build_response(result, extra)


def _record_decision(request: Request, decision: dict[str, Any]) -> None:
    import time
    decision["timestamp"] = time.time()
    request.app.state.recent_decisions.append(decision)

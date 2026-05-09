from __future__ import annotations

import time
import uuid
from typing import Any

import httpx
from fastapi import Request
from fastapi.responses import JSONResponse, Response

from request_manager.cache import CachePolicy, RedisResponseCache
from request_manager.circuit_breaker import CircuitBreaker
from request_manager.limiter import RequestLimiter
from request_manager.metrics import MetricsRecorder
from request_manager.models import CachedResponse, CacheState, LimitState
from request_manager.singleflight import SingleFlight
from request_manager.target_resolver import LimitResolver, TargetResolver

HOP_BY_HOP_HEADERS = {
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailers",
    "transfer-encoding",
    "upgrade",
    "host",
    "content-length",
}


def extract_agent_id(path: str) -> tuple[str, str]:
    if not path.startswith("/agents/"):
        raise ValueError("path must start with /agents/")
    remainder = path[len("/agents/") :]
    if "/" not in remainder:
        return remainder, ""
    agent_id, subpath = remainder.split("/", 1)
    return agent_id, f"/{subpath}"


def build_upstream_url(target, subpath: str) -> str:
    return f"{target.upstream_url}{subpath or '/'}"


def copy_request_headers(headers: dict[str, str]) -> dict[str, str]:
    return {key: value for key, value in headers.items() if key.lower() not in HOP_BY_HOP_HEADERS}


def copy_response_headers(headers: dict[str, str]) -> dict[str, str]:
    return {key: value for key, value in headers.items() if key.lower() not in HOP_BY_HOP_HEADERS}


class RequestProxy:
    def __init__(
        self,
        target_resolver: TargetResolver,
        limit_resolver: LimitResolver,
        cache: RedisResponseCache,
        cache_policy: CachePolicy,
        singleflight: SingleFlight,
        limiter: RequestLimiter,
        circuit_breaker: CircuitBreaker,
        metrics: MetricsRecorder,
        http_client: httpx.AsyncClient,
    ) -> None:
        self.target_resolver = target_resolver
        self.limit_resolver = limit_resolver
        self.cache = cache
        self.cache_policy = cache_policy
        self.singleflight = singleflight
        self.limiter = limiter
        self.circuit_breaker = circuit_breaker
        self.metrics = metrics
        self.http_client = http_client

    async def handle(self, request: Request) -> Response:
        try:
            agent_id, subpath = extract_agent_id(request.url.path)
        except ValueError:
            return JSONResponse({"error": "invalid_agent_path"}, status_code=404)

        target = await self.target_resolver.resolve(agent_id)
        if target is None:
            return JSONResponse(
                {"error": "agent_target_not_found", "agent_id": agent_id},
                status_code=404,
                headers={"X-Request-Layer-Agent": agent_id, "X-Request-Layer-Cache": CacheState.bypass.value},
            )

        limits = await self.limit_resolver.resolve(agent_id)
        body = await request.body()
        headers = dict(request.headers)
        json_body: dict[str, Any] | None = None
        if request.method.upper() == "POST" and "application/json" in headers.get("content-type", ""):
            try:
                json_body = await request.json()
            except Exception:
                json_body = None

        cache_decision = (
            self.cache_policy.decide(
                agent_id=agent_id,
                headers=headers,
                json_body=json_body,
                target=target,
                limits=limits,
            )
            if json_body is not None
            else None
        )

        claim = None
        if cache_decision and cache_decision.cacheable and cache_decision.cache_key:
            cached = await self.cache.get(cache_decision.cache_key)
            if cached:
                await self.metrics.increment(agent_id, "cache_hits")
                return self._cached_response(agent_id, cached)
            claim = await self.singleflight.claim(cache_decision.cache_key)
            if not claim.owner:
                await self.metrics.increment(agent_id, "singleflight_waiters")
                if await self.singleflight.wait_until_ready(cache_decision.cache_key):
                    cached = await self.cache.get(cache_decision.cache_key)
                    if cached:
                        await self.metrics.increment(agent_id, "cache_hits")
                        return self._cached_response(agent_id, cached)
                return JSONResponse(
                    {"error": "singleflight_timeout", "agent_id": agent_id},
                    status_code=503,
                    headers={
                        "Retry-After": "1",
                        "X-Request-Layer-Agent": agent_id,
                        "X-Request-Layer-Cache": CacheState.miss.value,
                        "X-Request-Layer-Limit-State": LimitState.normal.value,
                    },
                )
        else:
            await self.metrics.increment(agent_id, "cache_bypasses")

        await self.metrics.increment(agent_id, "cache_misses")
        circuit = await self.circuit_breaker.before_request(agent_id)
        if not circuit.allowed:
            if claim:
                await self.singleflight.release(claim)
            return JSONResponse(
                {"error": "circuit_open", "agent_id": agent_id},
                status_code=503,
                headers={
                    "Retry-After": str(circuit.retry_after_seconds),
                    "X-Request-Layer-Agent": agent_id,
                    "X-Request-Layer-Cache": CacheState.miss.value,
                    "X-Request-Layer-Limit-State": LimitState.circuit_open.value,
                },
            )

        request_id = str(uuid.uuid4())
        acquired = await self.limiter.acquire(agent_id, limits, request_id)
        await self.metrics.record_queue_wait(agent_id, acquired.queue_wait_ms)
        limit_state = LimitState.degraded.value if acquired.degraded else LimitState.normal.value
        if not acquired.acquired:
            if acquired.reason == "queue-timeout":
                await self.metrics.increment(agent_id, "queue_timeouts")
            if claim:
                await self.singleflight.release(claim)
            return JSONResponse(
                {"error": acquired.reason, "agent_id": agent_id},
                status_code=429,
                headers={
                    "Retry-After": str(acquired.retry_after_seconds),
                    "X-Request-Layer-Agent": agent_id,
                    "X-Request-Layer-Cache": CacheState.miss.value,
                    "X-Request-Layer-Queue-Wait-Ms": str(acquired.queue_wait_ms),
                    "X-Request-Layer-Limit-State": limit_state,
                },
            )

        started = time.monotonic()
        upstream_success = False
        try:
            await self.metrics.increment(agent_id, "upstream_requests")
            upstream_response = await self.http_client.request(
                request.method,
                build_upstream_url(target, subpath),
                content=body,
                headers=copy_request_headers(headers),
                params=dict(request.query_params),
            )
            upstream_success = 200 <= upstream_response.status_code < 500
            response_headers = copy_response_headers(dict(upstream_response.headers))
            response_headers.update(
                {
                    "X-Request-Layer-Agent": agent_id,
                    "X-Request-Layer-Cache": CacheState.miss.value,
                    "X-Request-Layer-Queue-Wait-Ms": str(acquired.queue_wait_ms),
                    "X-Request-Layer-Limit-State": limit_state,
                }
            )
            if (
                cache_decision
                and cache_decision.cacheable
                and cache_decision.cache_key
                and 200 <= upstream_response.status_code < 300
                and "application/json" in upstream_response.headers.get("content-type", "")
            ):
                await self.cache.set(
                    cache_decision.cache_key,
                    CachedResponse(
                        status_code=upstream_response.status_code,
                        media_type=upstream_response.headers.get("content-type"),
                        body=upstream_response.content,
                        headers=copy_response_headers(dict(upstream_response.headers)),
                    ),
                    ttl_seconds=limits.cache_ttl_seconds,
                )
            return Response(
                content=upstream_response.content,
                status_code=upstream_response.status_code,
                media_type=upstream_response.headers.get("content-type"),
                headers=response_headers,
            )
        except httpx.TimeoutException:
            await self.metrics.increment(agent_id, "upstream_errors")
            return JSONResponse(
                {"error": "upstream_timeout", "agent_id": agent_id},
                status_code=504,
                headers={
                    "X-Request-Layer-Agent": agent_id,
                    "X-Request-Layer-Cache": CacheState.miss.value,
                    "X-Request-Layer-Queue-Wait-Ms": str(acquired.queue_wait_ms),
                    "X-Request-Layer-Limit-State": limit_state,
                },
            )
        except httpx.HTTPError:
            await self.metrics.increment(agent_id, "upstream_errors")
            return JSONResponse(
                {"error": "upstream_error", "agent_id": agent_id},
                status_code=502,
                headers={
                    "X-Request-Layer-Agent": agent_id,
                    "X-Request-Layer-Cache": CacheState.miss.value,
                    "X-Request-Layer-Queue-Wait-Ms": str(acquired.queue_wait_ms),
                    "X-Request-Layer-Limit-State": limit_state,
                },
            )
        finally:
            elapsed_ms = int((time.monotonic() - started) * 1000)
            await self.metrics.record_latency(agent_id, elapsed_ms)
            await self.circuit_breaker.record_result(agent_id, success=upstream_success)
            await self.limiter.release(agent_id, request_id=request_id, degraded=acquired.degraded)
            if claim:
                await self.singleflight.release(claim)

    def _cached_response(self, agent_id: str, cached: CachedResponse) -> Response:
        headers = copy_response_headers(cached.headers)
        headers.update(
            {
                "X-Request-Layer-Agent": agent_id,
                "X-Request-Layer-Cache": CacheState.hit.value,
                "X-Request-Layer-Queue-Wait-Ms": "0",
                "X-Request-Layer-Limit-State": LimitState.normal.value,
            }
        )
        return Response(content=cached.body, status_code=cached.status_code, media_type=cached.media_type, headers=headers)

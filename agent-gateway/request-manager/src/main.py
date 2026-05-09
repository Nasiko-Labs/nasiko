import asyncio
import json
import logging
import time
from contextlib import asynccontextmanager
from typing import Dict, Optional

import httpx
import redis.asyncio as aioredis
from fastapi import FastAPI, HTTPException, Request, Response
from opentelemetry import trace
from opentelemetry.exporter.otlp.proto.grpc.trace_exporter import OTLPSpanExporter
from opentelemetry.sdk.resources import Resource
from opentelemetry.sdk.trace import TracerProvider
from opentelemetry.sdk.trace.export import BatchSpanProcessor
from pythonjsonlogger import jsonlogger

from src.cache import CacheManager, RequestLogger, build_cache_key, deduplicate_history, extract_query_text
from src.config import settings
from src.dashboard import DASHBOARD_HTML
from src.rate_limiter import RateLimiterAndQueue

_handler = logging.StreamHandler()
_handler.setFormatter(jsonlogger.JsonFormatter())
logging.basicConfig(level=settings.LOG_LEVEL, handlers=[_handler])
logger = logging.getLogger(__name__)

_redis: Optional[aioredis.Redis] = None
_cache: Optional[CacheManager] = None
_limiter: Optional[RateLimiterAndQueue] = None
_req_log: Optional[RequestLogger] = None

_stats: Dict[str, int] = {
    "total_requests": 0,
    "cache_hits": 0,
    "cache_misses": 0,
    "rate_limited": 0,
    "queued": 0,
    "rejected_queue_full": 0,
    "rejected_timeout": 0,
    "errors": 0,
}

# ── OpenTelemetry setup ─────────────────────────────────────────────────────

def _setup_tracing():
    resource = Resource.create({"service.name": settings.OTEL_SERVICE_NAME})
    provider = TracerProvider(resource=resource)
    exporter = OTLPSpanExporter(endpoint=settings.OTEL_EXPORTER_OTLP_ENDPOINT, insecure=True)
    provider.add_span_processor(BatchSpanProcessor(exporter))
    trace.set_tracer_provider(provider)

_setup_tracing()
tracer = trace.get_tracer("request-manager")


@asynccontextmanager
async def lifespan(app: FastAPI):
    global _redis, _cache, _limiter, _req_log
    _redis = aioredis.from_url(settings.REDIS_URL, decode_responses=False)
    _cache = CacheManager(_redis)
    _limiter = RateLimiterAndQueue(_redis)
    _req_log = RequestLogger(_redis)
    logger.info("request-manager started", extra={"redis_url": settings.REDIS_URL})
    yield
    await _redis.aclose()


app = FastAPI(
    title="Nasiko Request Manager",
    version="1.0.0",
    description="Caching, rate limiting, and queueing layer between Kong and agent containers",
    lifespan=lifespan,
)


# ── Upstream helpers ────────────────────────────────────────────────────────

async def _proxy_upstream(upstream_url: str, body: dict, headers: dict) -> dict:
    timeout = httpx.Timeout(
        connect=settings.UPSTREAM_CONNECT_TIMEOUT,
        read=settings.UPSTREAM_READ_TIMEOUT,
        write=30.0,
        pool=5.0,
    )
    with tracer.start_as_current_span("upstream.call") as span:
        span.set_attribute("upstream.url", upstream_url)
        async with httpx.AsyncClient(timeout=timeout) as client:
            resp = await client.post(upstream_url, json=body, headers=headers)
            span.set_attribute("upstream.status_code", resp.status_code)
            resp.raise_for_status()
            return resp.json()


def _safe_headers(request: Request) -> dict:
    hop_by_hop = {
        "host", "connection", "keep-alive", "transfer-encoding",
        "te", "trailer", "upgrade", "proxy-authenticate",
        "proxy-authorization", "content-length",
    }
    return {k: v for k, v in request.headers.items() if k.lower() not in hop_by_hop}


# ── Main proxy endpoint ─────────────────────────────────────────────────────

@app.api_route(
    "/proxy/{agent_host}/{port}",
    methods=["GET", "POST", "PUT", "DELETE", "PATCH", "OPTIONS"],
)
async def proxy_root(request: Request, agent_host: str, port: int) -> Response:
    return await proxy(request, agent_host, port, "")


@app.api_route(
    "/proxy/{agent_host}/{port}/{path:path}",
    methods=["GET", "POST", "PUT", "DELETE", "PATCH", "OPTIONS"],
)
async def proxy(request: Request, agent_host: str, port: int, path: str) -> Response:
    _stats["total_requests"] += 1
    t_start = time.monotonic()
    upstream_url = f"http://{agent_host}:{port}/{path}"
    headers = _safe_headers(request)

    with tracer.start_as_current_span("request-manager.proxy") as span:
        span.set_attribute("agent.name", agent_host)
        span.set_attribute("upstream.url", upstream_url)
        span.set_attribute("http.method", request.method)

        # File uploads: bypass cache and rate limit, proxy raw bytes
        content_type = request.headers.get("content-type", "")
        if "multipart/form-data" in content_type:
            raw = await request.body()
            timeout = httpx.Timeout(connect=settings.UPSTREAM_CONNECT_TIMEOUT,
                                    read=settings.UPSTREAM_READ_TIMEOUT, write=30.0, pool=5.0)
            async with httpx.AsyncClient(timeout=timeout) as client:
                resp = await client.request(
                    method=request.method, url=upstream_url, content=raw, headers=headers
                )
            span.set_attribute("request.type", "multipart_bypass")
            return Response(content=resp.content, status_code=resp.status_code,
                            headers=dict(resp.headers),
                            media_type=resp.headers.get("content-type", "application/json"))

        # Parse JSON body; proxy raw if unparseable
        try:
            body: dict = await request.json()
        except Exception:
            raw = await request.body()
            span.set_attribute("request.type", "raw_bypass")
            async with httpx.AsyncClient() as client:
                resp = await client.request(
                    method=request.method, url=upstream_url, content=raw, headers=headers
                )
            return Response(content=resp.content, status_code=resp.status_code,
                            media_type=resp.headers.get("content-type", "application/json"))

        agent_name = agent_host
        body = deduplicate_history(body)
        query = extract_query_text(body)
        method = body.get("method", "unknown")
        span.set_attribute("query.text", query[:200])
        span.set_attribute("jsonrpc.method", method)

        # Cache check
        cache_key = build_cache_key(agent_name, body)
        cached = await _cache.get(cache_key, agent_name)
        if cached is not None:
            _stats["cache_hits"] += 1
            latency_ms = (time.monotonic() - t_start) * 1000
            span.set_attribute("cache.result", "HIT")
            span.set_attribute("latency_ms", round(latency_ms, 1))
            await _req_log.log(agent_name, query, "HIT", latency_ms, method=method)
            return Response(content=cached, status_code=200,
                            media_type="application/json",
                            headers={"X-Cache": "HIT"})
        _stats["cache_misses"] += 1
        span.set_attribute("cache.result", "MISS")

        # Rate limit check
        allowed = await _limiter.check(agent_name)
        span.set_attribute("rate_limit.allowed", allowed)

        if allowed:
            try:
                data = await _proxy_upstream(upstream_url, body, headers)
            except httpx.HTTPStatusError as exc:
                _stats["errors"] += 1
                latency_ms = (time.monotonic() - t_start) * 1000
                span.set_attribute("error", True)
                span.set_attribute("error.status_code", exc.response.status_code)
                await _req_log.log(agent_name, query, "MISS", latency_ms, error=True, method=method)
                raise HTTPException(status_code=exc.response.status_code, detail=exc.response.text)
            except Exception as exc:
                _stats["errors"] += 1
                latency_ms = (time.monotonic() - t_start) * 1000
                span.set_attribute("error", True)
                span.set_attribute("error.message", str(exc))
                await _req_log.log(agent_name, query, "MISS", latency_ms, error=True, method=method)
                logger.error("upstream error", extra={"agent": agent_name, "error": str(exc)})
                raise HTTPException(status_code=502, detail="Bad Gateway")

            ttl = await _cache.get_ttl_for_agent(agent_name)
            response_str = json.dumps(data)
            await _cache.set(cache_key, agent_name, response_str, ttl)
            latency_ms = (time.monotonic() - t_start) * 1000
            span.set_attribute("cache.stored", True)
            span.set_attribute("cache.ttl", ttl)
            span.set_attribute("latency_ms", round(latency_ms, 1))
            await _req_log.log(agent_name, query, "MISS", latency_ms, method=method)
            return Response(content=response_str, status_code=200,
                            media_type="application/json",
                            headers={"X-Cache": "MISS"})

        # Rate limit exceeded — enqueue
        _stats["rate_limited"] += 1
        span.set_attribute("queued", True)
        logger.info("rate limited, queueing", extra={"agent": agent_name})

        try:
            data = await _limiter.enqueue(
                agent_name=agent_name,
                upstream_url=upstream_url,
                body=body,
                headers=headers,
                proxy_fn=_proxy_upstream,
            )
            _stats["queued"] += 1
            ttl = await _cache.get_ttl_for_agent(agent_name)
            response_str = json.dumps(data)
            await _cache.set(cache_key, agent_name, response_str, ttl)
            latency_ms = (time.monotonic() - t_start) * 1000
            span.set_attribute("latency_ms", round(latency_ms, 1))
            await _req_log.log(agent_name, query, "MISS", latency_ms, queued=True, method=method)
            return Response(content=response_str, status_code=200,
                            media_type="application/json",
                            headers={"X-Cache": "MISS", "X-Queued": "true"})

        except asyncio.QueueFull:
            _stats["rejected_queue_full"] += 1
            latency_ms = (time.monotonic() - t_start) * 1000
            span.set_attribute("error", True)
            span.set_attribute("error.type", "queue_full")
            await _req_log.log(agent_name, query, "REJECTED", latency_ms, error=True, method=method)
            raise HTTPException(status_code=429, detail={
                "error": "rate_limit_exceeded",
                "message": f"Agent '{agent_name}' queue is full ({settings.QUEUE_MAX_DEPTH} max). Retry later.",
            })
        except asyncio.TimeoutError:
            _stats["rejected_timeout"] += 1
            latency_ms = (time.monotonic() - t_start) * 1000
            span.set_attribute("error", True)
            span.set_attribute("error.type", "queue_timeout")
            await _req_log.log(agent_name, query, "REJECTED", latency_ms, error=True, method=method)
            raise HTTPException(status_code=429, detail={
                "error": "queue_timeout",
                "message": f"Request for '{agent_name}' timed out after {settings.QUEUE_TIMEOUT_SECONDS}s in queue.",
            })
        except Exception as exc:
            _stats["errors"] += 1
            latency_ms = (time.monotonic() - t_start) * 1000
            span.set_attribute("error", True)
            span.set_attribute("error.message", str(exc))
            await _req_log.log(agent_name, query, "MISS", latency_ms, error=True, method=method)
            logger.error("queued proxy error", extra={"agent": agent_name, "error": str(exc)})
            raise HTTPException(status_code=502, detail="Bad Gateway")


# ── Monitoring endpoints ────────────────────────────────────────────────────

@app.get("/manage/dashboard", response_class=Response)
async def dashboard():
    return Response(content=DASHBOARD_HTML, media_type="text/html")


@app.get("/manage/health")
async def health():
    try:
        await _redis.ping()
        redis_ok = True
    except Exception:
        redis_ok = False
    return {
        "status": "healthy" if redis_ok else "degraded",
        "redis": "connected" if redis_ok else "disconnected",
        "timestamp": time.time(),
    }


@app.get("/manage/stats")
async def stats():
    # Derive totals from Redis so they survive restarts
    cache_data = await _cache.get_stats()
    total_hits   = sum(v.get("hits",   0) for v in cache_data.values())
    total_misses = sum(v.get("misses", 0) for v in cache_data.values())

    rl_events = await _limiter.rate_limit_stats()
    total_limited = sum(v.get("limited", 0) for v in rl_events.values())
    total_queued  = sum(v.get("queued",  0) for v in rl_events.values())

    global_stats = {
        "total_requests":      total_hits + total_misses,
        "cache_hits":          total_hits,
        "cache_misses":        total_misses,
        "rate_limited":        total_limited,
        "queued":              total_queued,
        "rejected_queue_full": _stats["rejected_queue_full"],
        "rejected_timeout":    _stats["rejected_timeout"],
        "errors":              _stats["errors"],
    }
    return {
        "global": global_stats,
        "rate_limit_events": rl_events,
    }


@app.get("/manage/cache/stats")
async def cache_stats():
    return await _cache.get_stats()


@app.get("/manage/requests")
async def requests_all(limit: int = 50):
    """Recent requests across all agents — newest first."""
    return await _req_log.get_all_recent(limit=limit)


@app.get("/manage/requests/{agent_name}")
async def requests_agent(agent_name: str, limit: int = 20):
    """Recent requests for a specific agent — newest first."""
    entries = await _req_log.get_recent(agent_name, limit=limit)
    hits = sum(1 for e in entries if e["cache"] == "HIT")
    total = len(entries)
    return {
        "agent": agent_name,
        "hit_rate": round(hits / total, 3) if total else 0.0,
        "requests": entries,
    }


@app.delete("/manage/cache")
async def cache_clear_all():
    count = await _cache.clear_all()
    return {"cleared_keys": count}


@app.delete("/manage/cache/{agent_name}")
async def cache_clear_agent(agent_name: str):
    count = await _cache.clear_agent(agent_name)
    return {"agent": agent_name, "cleared_keys": count}


@app.get("/manage/rate-limits")
async def rate_limits_list():
    return {
        "default": {
            "requests_per_minute": settings.DEFAULT_RATE_LIMIT_RPM,
            "window_seconds": settings.RATE_LIMIT_WINDOW_SECONDS,
        },
        "per_agent_overrides": await _limiter.get_all_limits(),
    }


@app.post("/manage/rate-limits/{agent_name}")
async def rate_limit_set(agent_name: str, body: dict):
    rpm = body.get("requests_per_minute")
    if not isinstance(rpm, int) or rpm <= 0:
        raise HTTPException(status_code=422, detail="requests_per_minute must be a positive integer")
    await _limiter.set_limit(agent_name, rpm)
    return {"agent": agent_name, "requests_per_minute": rpm}


@app.delete("/manage/rate-limits/{agent_name}")
async def rate_limit_reset(agent_name: str):
    await _limiter.reset_limit(agent_name)
    return {"agent": agent_name, "reset_to_default": settings.DEFAULT_RATE_LIMIT_RPM}


@app.get("/manage/queue/stats")
async def queue_stats():
    return _limiter.queue_stats()

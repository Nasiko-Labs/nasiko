"""
Nasiko Router Service — main application.

Adds a unified request management layer (caching + rate limiting) on top of
the existing RouterOrchestrator, and exposes operational monitoring endpoints.
"""

import logging
from io import BytesIO
from typing import List, Optional

from fastapi import FastAPI, File, Form, HTTPException, UploadFile
from fastapi.middleware.cors import CORSMiddleware
from fastapi.params import Depends
from fastapi.responses import StreamingResponse
from fastapi.security import HTTPAuthorizationCredentials, HTTPBearer
from pydantic import BaseModel

from router.src.config import settings
from router.src.entities import UserRequest
from router.src.services import RequestManager

# ---------------------------------------------------------------------------
# Logging
# ---------------------------------------------------------------------------
logging.basicConfig(
    level=getattr(logging, settings.LOG_LEVEL),
    format="%(asctime)s [%(levelname)s] %(name)s: %(message)s",
)
logger = logging.getLogger(__name__)

# ---------------------------------------------------------------------------
# Security
# ---------------------------------------------------------------------------
security = HTTPBearer()

# ---------------------------------------------------------------------------
# FastAPI app
# ---------------------------------------------------------------------------
app = FastAPI(
    title="Nasiko Router Service",
    description=(
        "AI-powered agent routing service with unified request management "
        "(response caching + per-agent rate limiting)."
    ),
    version="3.0.0",
)

app.add_middleware(
    CORSMiddleware,
    allow_origins=settings.cors_origins_list,
    allow_credentials=True,
    allow_methods=["*"],
    allow_headers=["*"],
)

# ---------------------------------------------------------------------------
# Singleton request manager (cache + rate limiter + orchestrator)
# ---------------------------------------------------------------------------
request_manager = RequestManager()


@app.on_event("startup")
async def _startup():
    await request_manager.startup()


@app.on_event("shutdown")
async def _shutdown():
    await request_manager.shutdown()


# ===========================================================================
# Core routing endpoint
# ===========================================================================

@app.post("/router")
async def process_request(
    session_id: str = Form(...),
    query: str = Form(...),
    route: Optional[str] = Form(None),
    files: Optional[List[UploadFile]] = File(
        None,
        max_length=settings.MAX_FILE_SIZE,
        description="Optional files to upload (PDF, TXT, DOCX, XLSX)",
    ),
    credentials: HTTPAuthorizationCredentials = Depends(security),
) -> StreamingResponse:
    """
    Process a user request through the router pipeline.

    The request passes through:
    1. Per-agent rate limiter (token bucket + async queue)
    2. Response cache (Redis-backed; LRU fallback)
    3. RouterOrchestrator (LLM routing + agent call) — only on cache miss

    Returns a streaming response of newline-delimited JSON RouterResponse objects.
    """
    try:
        validation_error = _validate_inputs(session_id, query)
        if validation_error:
            raise HTTPException(status_code=400, detail=validation_error)

        files_to_forward = await _process_files(files)
        request = UserRequest(session_id=session_id, query=query, route=route)
        token = credentials.credentials

        logger.info(f"Incoming request: session={session_id}, query={query[:80]!r}")

        return StreamingResponse(
            request_manager.handle_request(request, files_to_forward, token),
            media_type="application/json",
        )

    except HTTPException:
        raise
    except Exception as exc:
        logger.error(f"Unexpected error in /router: {exc}")
        raise HTTPException(status_code=500, detail=f"Internal server error: {exc}")


# ===========================================================================
# Health endpoints
# ===========================================================================

@app.get("/health")
async def health_check():
    """Aggregate health check (router + cache + rate limiter)."""
    try:
        return await request_manager.health_check()
    except Exception as exc:
        logger.error(f"Health check failed: {exc}")
        raise HTTPException(status_code=503, detail="Service unhealthy")


@app.get("/router/health")
async def router_health():
    return {"status": "ok"}


# ===========================================================================
# Monitoring — cache
# ===========================================================================

@app.get("/monitor/cache/stats")
async def cache_stats():
    """
    Return overall cache statistics.

    Includes hit rate, miss count, backend type (redis/lru), and Redis memory
    usage when Redis is configured.
    """
    return await request_manager.get_cache_stats()


@app.get("/monitor/cache/stats/{agent_name}")
async def agent_cache_stats(agent_name: str):
    """Return cache hit/miss statistics for a specific agent."""
    return await request_manager.get_agent_cache_stats(agent_name)


@app.delete("/monitor/cache")
async def clear_all_cache():
    """
    Flush the entire response cache.

    Use this after deploying updated agents or when stale responses are suspected.
    """
    return await request_manager.clear_cache()


@app.delete("/monitor/cache/{agent_name}")
async def clear_agent_cache(agent_name: str):
    """Flush cached responses for a specific agent."""
    return await request_manager.clear_agent_cache(agent_name)


# ===========================================================================
# Monitoring — rate limiter
# ===========================================================================

@app.get("/monitor/rate-limits/configs/list")
async def list_rate_limit_configs():
    """List all agents with custom rate limit configurations."""
    return request_manager.list_rate_limit_configs()


@app.get("/monitor/rate-limits")
async def rate_limit_stats():
    """
    Return rate limiter statistics for all agents.

    Shows token bucket state, queue depth, acceptance/rejection counts, and
    average queue wait time per agent.
    """
    return request_manager.get_rate_limit_stats()


@app.get("/monitor/rate-limits/{agent_name}")
async def agent_rate_limit_stats(agent_name: str):
    """Return rate limiter statistics for a specific agent."""
    stats = request_manager.get_agent_rate_limit_stats(agent_name)
    if stats is None:
        raise HTTPException(
            status_code=404,
            detail=f"No rate limit data for agent '{agent_name}' (no requests seen yet)",
        )
    return stats


class RateLimitConfigRequest(BaseModel):
    requests_per_second: float = 5.0
    burst_capacity: int = 10
    queue_size: int = 20


@app.put("/monitor/rate-limits/{agent_name}")
async def configure_rate_limit(agent_name: str, config: RateLimitConfigRequest):
    """
    Set or update the rate limit for a specific agent.

    - **requests_per_second**: sustained token refill rate
    - **burst_capacity**: maximum tokens (burst size)
    - **queue_size**: maximum requests that can wait in queue before rejection

    Changes take effect immediately for new requests.
    """
    if config.requests_per_second <= 0:
        raise HTTPException(status_code=400, detail="requests_per_second must be > 0")
    if config.burst_capacity < 1:
        raise HTTPException(status_code=400, detail="burst_capacity must be >= 1")
    if config.queue_size < 0:
        raise HTTPException(status_code=400, detail="queue_size must be >= 0")

    return request_manager.configure_rate_limit(
        agent_name,
        config.requests_per_second,
        config.burst_capacity,
        config.queue_size,
    )


@app.delete("/monitor/rate-limits/{agent_name}/config")
async def remove_rate_limit_config(agent_name: str):
    """
    Remove the custom rate limit configuration for an agent,
    reverting it to the global defaults.
    """
    return request_manager.remove_rate_limit_config(agent_name)


# ===========================================================================
# Monitoring — combined dashboard
# ===========================================================================

@app.get("/monitor/dashboard")
async def monitoring_dashboard():
    """
    Real-time operational dashboard.

    Returns a single JSON object with:
    - cache statistics (hit rate, backend, TTL)
    - rate limiter statistics (per-agent token bucket state, queue depths)
    - router health
    """
    cache_stats = await request_manager.get_cache_stats()
    rate_stats = request_manager.get_rate_limit_stats()
    health = await request_manager.health_check()

    return {
        "health": health,
        "cache": cache_stats,
        "rate_limiter": rate_stats,
    }


# ===========================================================================
# Legacy metrics stub (kept for backward compatibility)
# ===========================================================================

@app.get("/metrics")
async def get_metrics():
    """
    Router service metrics.

    For detailed stats use /monitor/dashboard, /monitor/cache/stats,
    and /monitor/rate-limits.
    """
    cache_stats = await request_manager.get_cache_stats()
    rate_stats = request_manager.get_rate_limit_stats()

    total_agents = len(rate_stats.get("agents", {}))
    total_requests = sum(
        v.get("total_requests", 0)
        for v in rate_stats.get("agents", {}).values()
    )
    total_rejected = sum(
        v.get("rejected_requests", 0)
        for v in rate_stats.get("agents", {}).values()
    )

    return {
        "requests_processed": total_requests,
        "active_agents": total_agents,
        "cache_hit_rate_pct": cache_stats.get("hit_rate_pct", 0.0),
        "cache_hits": cache_stats.get("hits", 0),
        "cache_misses": cache_stats.get("misses", 0),
        "rate_limit_rejections": total_rejected,
        "error_rate": round(total_rejected / total_requests * 100, 2) if total_requests > 0 else 0.0,
    }


# ===========================================================================
# Input helpers
# ===========================================================================

def _validate_inputs(session_id: str, query: str) -> Optional[str]:
    if not session_id or not session_id.strip():
        return "session_id cannot be empty"
    if not query or not query.strip():
        return "query cannot be empty"
    return None


async def _process_files(files: Optional[List[UploadFile]]) -> List[tuple]:
    files_to_forward = []
    if not files:
        return files_to_forward

    for file in files:
        try:
            if file.size and file.size > settings.MAX_FILE_SIZE:
                raise HTTPException(
                    status_code=413,
                    detail=f"File {file.filename} exceeds maximum size",
                )
            content_bytes = await file.read()
            bio = BytesIO(content_bytes)
            bio.seek(0)
            files_to_forward.append(
                (
                    "files",
                    (
                        file.filename,
                        bio,
                        file.content_type or "application/octet-stream",
                    ),
                )
            )
        except HTTPException:
            raise
        except Exception as exc:
            logger.error(f"Error reading file {file.filename}: {exc}")
            raise HTTPException(
                status_code=400,
                detail=f"Failed to read file {file.filename}: {exc}",
            )

    return files_to_forward


if __name__ == "__main__":
    import uvicorn

    uvicorn.run(
        "main:app",
        host=settings.HOST,
        port=settings.PORT,
        reload=settings.RELOAD,
        log_level=settings.LOG_LEVEL.lower(),
    )

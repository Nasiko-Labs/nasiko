"""
RAL API Routes
==============
All routes are mounted under /ral (which becomes /api/v1/ral via the main router).
No authentication required — the dashboard is intentionally public/internal-network.
"""

from __future__ import annotations

from fastapi import APIRouter, Query
from fastapi.responses import HTMLResponse

from app.api.handlers import HandlerFactory


def create_ral_routes(handlers: HandlerFactory) -> APIRouter:
    """Create and return the RAL router with all endpoints registered."""
    router = APIRouter(prefix="/ral", tags=["Resilient Agent Request Layer"])
    h = handlers.ral

    @router.get(
        "/metrics",
        summary="RAL metrics snapshot",
        description=(
            "Returns a point-in-time snapshot of all RAL metrics: active requests, "
            "requests/sec, cache hit ratio, queue depth, per-agent traffic, latency, "
            "error rates, retry counts, and throttle counts."
        ),
    )
    async def get_metrics():
        return await h.get_metrics()

    @router.get(
        "/agents",
        summary="Per-agent traffic stats",
        description="Returns request counts, error counts, and average latency per agent.",
    )
    async def get_agent_stats():
        return await h.get_agent_stats()

    @router.get(
        "/logs",
        summary="Recent request logs",
        description=(
            "Returns the most recent request log entries including query, selected agent, "
            "latency, cache/throttle flags, and status."
        ),
    )
    async def get_logs(limit: int = Query(default=50, ge=1, le=500)):
        return await h.get_logs(limit)

    @router.post(
        "/cache/clear",
        summary="Flush RAL response cache",
        description="Deletes all cached LLM responses. Returns the number of entries removed.",
    )
    async def flush_cache():
        return await h.flush_cache()

    @router.get(
        "/health",
        summary="RAL subsystem health",
        description=(
            "Returns the health status of RAL components (Redis connectivity, "
            "metric freshness) plus current active request count and queue depth."
        ),
    )
    async def get_health():
        return await h.get_health()

    @router.get(
        "/dashboard",
        summary="RAL monitoring dashboard",
        description=(
            "Serves the standalone HTML monitoring dashboard. "
            "No authentication required — intended for internal network access."
        ),
        response_class=HTMLResponse,
    )
    async def get_dashboard():
        return await h.get_dashboard()

    return router

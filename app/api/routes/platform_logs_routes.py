from typing import Literal

from fastapi import APIRouter, Query

from app.utils.log_buffer import LOG_LEVELS, get_recent_platform_logs


def create_platform_logs_routes(handlers) -> APIRouter:
    router = APIRouter(prefix="/platform", tags=["platform-logs"])

    @router.get("/logs", summary="Get recent platform logs")
    async def get_recent_logs(
        level: Literal["INFO", "WARNING", "ERROR"] | None = Query(
            default=None,
            description="Filter logs by severity",
        ),
        limit: int = Query(default=200, ge=1, le=500),
    ):
        logs = get_recent_platform_logs(level=level, limit=limit)
        return {
            "items": logs,
            "levels": list(LOG_LEVELS),
            "count": len(logs),
            "limit": limit,
        }

    return router

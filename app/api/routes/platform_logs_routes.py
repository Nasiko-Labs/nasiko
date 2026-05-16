"""
Platform Logs Routes - Logging dashboard API.
"""

from typing import Optional

from fastapi import APIRouter, Depends, Query

from app.entity.platform_logs import PlatformLogCreate, PlatformLogsResponse
from ..auth import get_user_id_from_token
from ..handlers import HandlerFactory


def create_platform_logs_routes(handlers: HandlerFactory) -> APIRouter:
    router = APIRouter(tags=["platform-logs"], prefix="/platform/logs")

    @router.get(
        "",
        response_model=PlatformLogsResponse,
        summary="List platform logs",
        description="Returns recent platform logs with optional level filter (INFO, WARNING, ERROR).",
    )
    async def list_platform_logs(
        level: Optional[str] = Query(
            None, description="Filter by log level: INFO, WARNING, or ERROR"
        ),
        limit: int = Query(100, ge=1, le=500),
        skip: int = Query(0, ge=0),
        _user_id: str = Depends(get_user_id_from_token),
    ):
        return await handlers.platform_logs.list_logs(
            level=level, limit=limit, skip=skip
        )

    @router.post(
        "",
        summary="Ingest a platform log entry",
        description="Record a platform log entry (requires authentication).",
    )
    async def create_platform_log(
        body: PlatformLogCreate,
        _user_id: str = Depends(get_user_id_from_token),
    ):
        return await handlers.platform_logs.create_log(body)

    return router

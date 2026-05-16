"""
Platform Logs Handler - Query and ingest platform-wide logs.
"""

from typing import Any, Dict, Optional

from fastapi import HTTPException

from app.entity.platform_logs import PlatformLogCreate
from app.service.platform_logs_service import PlatformLogsService
from .base_handler import BaseHandler


class PlatformLogsHandler(BaseHandler):
    def __init__(self, service, logger):
        super().__init__(service, logger)
        self.platform_logs: PlatformLogsService = service.platform_logs

    async def list_logs(
        self,
        level: Optional[str] = None,
        limit: int = 100,
        skip: int = 0,
    ) -> Dict[str, Any]:
        try:
            return await self.platform_logs.list_logs(
                level=level, limit=limit, skip=skip
            )
        except ValueError as e:
            raise HTTPException(status_code=400, detail=str(e)) from e

    async def create_log(self, body: PlatformLogCreate) -> Dict[str, Any]:
        log_id = await self.platform_logs.record_log(
            message=body.message,
            level=body.level,
            service=body.service,
            metadata=body.metadata,
        )
        return {"id": log_id, "status": "created"}

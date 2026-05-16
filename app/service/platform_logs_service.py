"""
Platform Logs Service - Business logic for platform log ingestion and queries.
"""

from typing import Any, Dict, List, Optional

from app.repository.platform_logs_repository import PlatformLogsRepository


class PlatformLogsService:
    def __init__(self, repo: PlatformLogsRepository, logger):
        self.repo = repo
        self.logger = logger

    async def record_log(
        self,
        message: str,
        level: str = "INFO",
        service: str = "nasiko-backend",
        logger_name: Optional[str] = None,
        metadata: Optional[Dict[str, Any]] = None,
    ) -> str:
        entry = {
            "level": level.upper(),
            "message": message,
            "service": service,
            "logger": logger_name or "app",
        }
        if metadata:
            entry["metadata"] = metadata
        return await self.repo.insert_log(entry)

    async def list_logs(
        self,
        level: Optional[str] = None,
        limit: int = 100,
        skip: int = 0,
    ) -> Dict[str, Any]:
        normalized_level = level.upper() if level else None
        if normalized_level and normalized_level not in ("INFO", "WARNING", "ERROR"):
            raise ValueError(
                f"Invalid log level: {level}. Use INFO, WARNING, or ERROR."
            )

        logs = await self.repo.list_logs(
            level=normalized_level, limit=limit, skip=skip
        )
        total = await self.repo.count_logs(level=normalized_level)
        return {
            "logs": logs,
            "total": total,
            "limit": limit,
            "skip": skip,
            "level_filter": normalized_level,
        }

    async def seed_if_empty(self) -> int:
        return await self.repo.seed_sample_logs_if_empty()

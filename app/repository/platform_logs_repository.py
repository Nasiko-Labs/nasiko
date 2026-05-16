"""
Platform Logs Repository - Persist and query platform-wide log entries.
"""

from datetime import datetime, timezone
from typing import Any, Dict, List, Optional

from .base_repository import BaseRepository


class PlatformLogsRepository(BaseRepository):
    """Repository for platform log entries stored in MongoDB."""

    def __init__(self, db, logger):
        super().__init__(db, logger)
        self.collection = db["platform_logs"]

    async def ensure_indexes(self):
        try:
            await self.collection.create_index([("timestamp", -1)])
            await self.collection.create_index("level")
            await self.collection.create_index("service")
            self.logger.info("Platform logs collection indexes initialized")
        except Exception as e:
            self.logger.warning(f"Error ensuring platform_logs indexes: {e}")

    async def insert_log(self, entry: Dict[str, Any]) -> str:
        if "timestamp" not in entry:
            entry["timestamp"] = datetime.now(timezone.utc)
        result = await self.collection.insert_one(entry)
        return str(result.inserted_id)

    async def count_logs(self, level: Optional[str] = None) -> int:
        query: Dict[str, Any] = {}
        if level:
            query["level"] = level.upper()
        return await self.collection.count_documents(query)

    async def list_logs(
        self,
        level: Optional[str] = None,
        limit: int = 100,
        skip: int = 0,
    ) -> List[Dict[str, Any]]:
        query: Dict[str, Any] = {}
        if level:
            query["level"] = level.upper()

        cursor = (
            self.collection.find(query)
            .sort("timestamp", -1)
            .skip(skip)
            .limit(min(limit, 500))
        )
        logs = await cursor.to_list(length=min(limit, 500))
        for log in logs:
            if "_id" in log:
                log["id"] = str(log.pop("_id"))
            if isinstance(log.get("timestamp"), datetime):
                log["timestamp"] = log["timestamp"].isoformat()
        return logs

    async def seed_sample_logs_if_empty(self) -> int:
        count = await self.collection.count_documents({})
        if count > 0:
            return 0

        now = datetime.now(timezone.utc)
        samples = [
            ("INFO", "Nasiko platform logging dashboard initialized"),
            ("INFO", "Database collections and indexes verified"),
            ("INFO", "Search service ready"),
            ("WARNING", "No agents registered yet — deploy an agent to get started"),
            ("INFO", "Kong gateway routes registered"),
            ("ERROR", "Example error entry for dashboard filter testing"),
            ("INFO", "Auth service connection healthy"),
            ("WARNING", "Phoenix observability optional — set PHOENIX_URL to enable traces"),
        ]
        entries = []
        for i, (level, message) in enumerate(samples):
            ts = now.replace(microsecond=0)
            entries.append(
                {
                    "timestamp": ts,
                    "level": level,
                    "message": message,
                    "service": "nasiko-backend",
                    "logger": "app.platform_logs",
                }
            )
        await self.collection.insert_many(entries)
        return len(entries)

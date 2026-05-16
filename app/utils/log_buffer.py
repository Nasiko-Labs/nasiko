from __future__ import annotations

import logging
import threading
from collections import deque
from datetime import datetime, timezone
from typing import Any


LOG_LEVELS = ("INFO", "WARNING", "ERROR")
_records: deque[dict[str, Any]] = deque(maxlen=500)
_lock = threading.Lock()


class PlatformLogBufferHandler(logging.Handler):
    """Keeps recent platform logs available for the dashboard API."""

    def emit(self, record: logging.LogRecord) -> None:
        if record.levelname not in LOG_LEVELS:
            return

        entry = {
            "timestamp": datetime.fromtimestamp(
                record.created, tz=timezone.utc
            ).isoformat().replace("+00:00", "Z"),
            "level": record.levelname,
            "logger": record.name,
            "message": record.getMessage(),
        }

        if record.exc_info:
            entry["exception"] = self.formatException(record.exc_info)

        with _lock:
            _records.append(entry)


def install_platform_log_handler(capacity: int = 500) -> None:
    global _records

    with _lock:
        if _records.maxlen != capacity:
            _records = deque(_records, maxlen=capacity)

    root_logger = logging.getLogger()
    if any(isinstance(handler, PlatformLogBufferHandler) for handler in root_logger.handlers):
        return

    handler = PlatformLogBufferHandler(level=logging.INFO)
    root_logger.addHandler(handler)


def get_recent_platform_logs(
    level: str | None = None, limit: int = 100
) -> list[dict[str, Any]]:
    normalized_level = level.upper() if level else None
    with _lock:
        records = list(_records)

    if normalized_level:
        records = [record for record in records if record["level"] == normalized_level]

    return list(reversed(records))[:limit]

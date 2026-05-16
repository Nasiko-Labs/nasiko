from __future__ import annotations

import itertools
import logging
import threading
from collections import deque
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Literal


LOG_LEVELS = ("INFO", "WARNING", "ERROR")
LogLevel = Literal["INFO", "WARNING", "ERROR"]

_records: deque[dict[str, Any]] = deque(maxlen=500)
_lock = threading.Lock()
_counter = itertools.count(1)
_exception_formatter = logging.Formatter()


class PlatformLogBufferHandler(logging.Handler):
    """Keeps recent platform logs available to the web dashboard."""

    def emit(self, record: logging.LogRecord) -> None:
        if record.levelname not in LOG_LEVELS:
            return

        created_at = datetime.fromtimestamp(record.created, tz=timezone.utc)
        source_file = Path(record.pathname).name if record.pathname else record.module
        sequence = next(_counter)

        entry: dict[str, Any] = {
            "id": f"log-{int(record.created * 1000)}-{sequence}",
            "timestamp": created_at.isoformat().replace("+00:00", "Z"),
            "level": record.levelname,
            "logger": record.name,
            "service": _service_from_logger(record.name),
            "message": record.getMessage(),
            "source": f"{source_file}:{record.lineno}",
        }

        if record.exc_info:
            entry["exception"] = _exception_formatter.formatException(record.exc_info)

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

    root_logger.addHandler(PlatformLogBufferHandler(level=logging.INFO))


def get_recent_platform_logs(
    level: LogLevel | None = None, limit: int = 100
) -> list[dict[str, Any]]:
    with _lock:
        records = list(_records)

    if level:
        records = [record for record in records if record["level"] == level]

    return list(reversed(records))[:limit]


def _service_from_logger(logger_name: str) -> str:
    parts = logger_name.split(".")
    if len(parts) >= 2 and parts[0] == "app":
        return parts[1]
    return parts[0] if parts else "platform"

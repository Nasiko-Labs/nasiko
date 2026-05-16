from __future__ import annotations

import itertools
import logging
import os
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

        log_id = f"log-{int(record.created * 1000)}-{sequence}"
        service = _string_attr(record, "service") or _service_from_logger(record.name)
        route = _string_attr(record, "route") or record.name
        trace_id = (
            _string_attr(record, "trace_id")
            or _string_attr(record, "traceId")
            or log_id
        )
        request_id = (
            _string_attr(record, "request_id")
            or _string_attr(record, "requestId")
            or trace_id
        )
        latency_ms = (
            _int_attr(record, "latency_ms") or _int_attr(record, "latencyMs") or 0
        )

        entry: dict[str, Any] = {
            "id": log_id,
            "timestamp": created_at.isoformat().replace("+00:00", "Z"),
            "level": record.levelname,
            "logger": record.name,
            "service": service,
            "route": route,
            "message": record.getMessage(),
            "trace_id": trace_id,
            "request_id": request_id,
            "latency_ms": latency_ms,
            "pod": _string_attr(record, "pod") or os.getenv("HOSTNAME", service),
            "source": _string_attr(record, "source") or f"{source_file}:{record.lineno}",
            "commit": _string_attr(record, "commit") or os.getenv("GIT_SHA", "runtime"),
        }

        status_code = _int_attr(record, "status_code")
        if status_code is not None:
            entry["status_code"] = status_code

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


def clear_platform_log_buffer() -> None:
    """Clear buffered logs for focused tests and local demos."""

    with _lock:
        _records.clear()


def _service_from_logger(logger_name: str) -> str:
    parts = logger_name.split(".")
    if len(parts) >= 2 and parts[0] == "app":
        return parts[1]
    return parts[0] if parts else "platform"


def _string_attr(record: logging.LogRecord, name: str) -> str | None:
    value = getattr(record, name, None)
    if value is None:
        return None

    text = str(value).strip()
    return text or None


def _int_attr(record: logging.LogRecord, name: str) -> int | None:
    value = getattr(record, name, None)
    if value is None:
        return None

    try:
        return int(value)
    except (TypeError, ValueError):
        return None

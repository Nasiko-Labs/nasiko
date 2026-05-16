"""
Logging handler that enqueues platform log entries for async MongoDB persistence.
"""

import asyncio
import logging
from typing import Optional

_log_queue: Optional[asyncio.Queue] = None
_SERVICE_NAME = "nasiko-backend"


def enqueue_log_entry(entry: dict) -> None:
    if _log_queue is None:
        return
    try:
        loop = asyncio.get_running_loop()
        loop.create_task(_log_queue.put(entry))
    except RuntimeError:
        pass


class MongoPlatformLogHandler(logging.Handler):
    """Captures Python log records and queues them for MongoDB storage."""

    def emit(self, record: logging.LogRecord) -> None:
        try:
            level = record.levelname
            if level not in ("INFO", "WARNING", "ERROR"):
                if record.levelno >= logging.ERROR:
                    level = "ERROR"
                elif record.levelno >= logging.WARNING:
                    level = "WARNING"
                else:
                    level = "INFO"

            enqueue_log_entry(
                {
                    "level": level,
                    "message": record.getMessage(),
                    "service": _SERVICE_NAME,
                    "logger": record.name,
                }
            )
        except Exception:
            self.handleError(record)


def setup_platform_logging(
    log_queue: asyncio.Queue, service_name: str = _SERVICE_NAME
) -> MongoPlatformLogHandler:
    global _log_queue, _SERVICE_NAME
    _log_queue = log_queue
    _SERVICE_NAME = service_name

    handler = MongoPlatformLogHandler()
    handler.setLevel(logging.INFO)

    root = logging.getLogger()
    if not any(isinstance(h, MongoPlatformLogHandler) for h in root.handlers):
        root.addHandler(handler)

    for name in ("app", "uvicorn", "uvicorn.access"):
        app_logger = logging.getLogger(name)
        if not any(isinstance(h, MongoPlatformLogHandler) for h in app_logger.handlers):
            app_logger.addHandler(handler)

    return handler

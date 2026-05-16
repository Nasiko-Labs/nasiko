"""
Background worker that drains the platform log queue into MongoDB.
"""

import asyncio
import logging
from typing import Optional

from app.repository.platform_logs_repository import PlatformLogsRepository

logger = logging.getLogger(__name__)

_worker_task: Optional[asyncio.Task] = None


async def _process_queue(
    repo: PlatformLogsRepository, log_queue: asyncio.Queue
) -> None:
    while True:
        try:
            entry = await log_queue.get()
            await repo.insert_log(entry)
            log_queue.task_done()
        except asyncio.CancelledError:
            break
        except Exception as e:
            logger.warning(f"Failed to persist platform log: {e}")


async def start_log_worker(
    repo: PlatformLogsRepository, log_queue: asyncio.Queue
) -> asyncio.Task:
    global _worker_task
    _worker_task = asyncio.create_task(_process_queue(repo, log_queue))
    return _worker_task


async def stop_log_worker() -> None:
    global _worker_task
    if _worker_task and not _worker_task.done():
        _worker_task.cancel()
        try:
            await _worker_task
        except asyncio.CancelledError:
            pass
    _worker_task = None

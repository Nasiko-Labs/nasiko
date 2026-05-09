"""
RAL Async Request Queue
=======================
Bounded async FIFO queue that absorbs traffic spikes and retries transient
failures without blocking the calling coroutine.

Behaviour
---------
* Excess requests (over the concurrency cap) are enqueued, not rejected.
* Each queued item has a deadline; it is dropped with a timeout error if it
  waits longer than RAL_QUEUE_TIMEOUT seconds.
* Transient failures (any exception raised by the work function) are retried
  up to RAL_MAX_RETRIES times with exponential back-off.
* The queue size is capped at RAL_MAX_QUEUE_SIZE; submissions to a full
  queue raise `QueueFullError` immediately (fast-fail rather than silent drop).

Integration
-----------
The orchestrator calls `queue.submit(work_fn, agent_id)` and awaits the
result.  The queue dispatches work items via a fixed pool of worker
coroutines (one per asyncio task), keeping the event loop non-blocking.
"""

from __future__ import annotations

import asyncio
import logging
import time
from dataclasses import dataclass, field
from typing import Awaitable, Callable, Any, TypeVar, Optional

import redis.asyncio as aioredis

from .config import ral_settings

logger = logging.getLogger(__name__)

T = TypeVar("T")


class QueueFullError(RuntimeError):
    """Raised when the request queue has reached its capacity."""


class QueueTimeoutError(TimeoutError):
    """Raised when a queued request waits longer than the configured timeout."""


@dataclass
class _WorkItem:
    fn: Callable[[], Awaitable[Any]]
    agent_id: str
    enqueued_at: float
    future: asyncio.Future
    retries_left: int


class AsyncRequestQueue:
    """
    Bounded async work queue with timeout and retry semantics.

    Usage
    -----
    ```python
    q = AsyncRequestQueue()
    await q.start()                        # launch worker tasks

    result = await q.submit(my_coro_fn, agent_id="my-agent")

    await q.stop()                         # drain and shut down
    ```
    """

    def __init__(self) -> None:
        self._max_size: int         = ral_settings.RAL_MAX_QUEUE_SIZE
        self._timeout: float        = ral_settings.RAL_QUEUE_TIMEOUT
        self._max_retries: int      = ral_settings.RAL_MAX_RETRIES
        self._retry_delay: float    = ral_settings.RAL_RETRY_DELAY
        self._concurrency: int      = ral_settings.RAL_MAX_CONCURRENT * 4  # worker pool

        self._queue: asyncio.Queue[_WorkItem] = asyncio.Queue(maxsize=self._max_size)
        self._workers: list[asyncio.Task] = []
        self._running = False

        # Redis client for cross-process stats persistence
        self._redis: Optional[aioredis.Redis] = None
        self._pfx = ral_settings.RAL_REDIS_PREFIX

        # Counters
        self._enqueued: int   = 0
        self._completed: int  = 0
        self._failed: int     = 0
        self._retried: int    = 0
        self._timed_out: int  = 0
        self._dropped: int    = 0

    # ------------------------------------------------------------------
    # Lifecycle
    # ------------------------------------------------------------------

    async def start(self) -> None:
        """Spawn the worker coroutine pool and connect Redis."""
        if self._running:
            return
        self._running = True
        # Connect Redis for cross-process stats
        try:
            self._redis = aioredis.from_url(
                ral_settings.redis_dsn,
                encoding="utf-8",
                decode_responses=True,
                max_connections=5,
            )
        except Exception as exc:
            logger.warning("AsyncRequestQueue: Redis unavailable (%s) — stats won't persist", exc)
        self._workers = [
            asyncio.create_task(self._worker(i), name=f"ral-queue-worker-{i}")
            for i in range(self._concurrency)
        ]
        logger.info(
            "RAL queue started: %d workers, max_size=%d, timeout=%.1fs",
            self._concurrency, self._max_size, self._timeout,
        )

    async def stop(self) -> None:
        """Drain the queue and cancel workers gracefully."""
        self._running = False
        for task in self._workers:
            task.cancel()
        await asyncio.gather(*self._workers, return_exceptions=True)
        self._workers.clear()
        if self._redis:
            await self._redis.aclose()
        logger.info("RAL queue stopped.")

    # ------------------------------------------------------------------
    # Public API
    # ------------------------------------------------------------------

    async def submit(
        self,
        fn: Callable[[], Awaitable[T]],
        agent_id: str = "default",
    ) -> T:
        """
        Enqueue a work item and await its result.

        Parameters
        ----------
        fn : async callable
            Zero-argument coroutine function to execute.
        agent_id : str
            Used for per-agent stats tracking.

        Raises
        ------
        QueueFullError   — if the queue is at capacity.
        QueueTimeoutError — if the item waits too long.
        Exception        — propagates failures after all retries are exhausted.
        """
        loop = asyncio.get_event_loop()
        future: asyncio.Future = loop.create_future()
        item = _WorkItem(
            fn=fn,
            agent_id=agent_id,
            enqueued_at=time.monotonic(),
            future=future,
            retries_left=self._max_retries,
        )

        try:
            self._queue.put_nowait(item)
            self._enqueued += 1
        except asyncio.QueueFull:
            self._dropped += 1
            raise QueueFullError(
                f"RAL queue is full ({self._max_size} items). "
                "Agent may be overloaded."
            )

        return await future

    # ------------------------------------------------------------------
    # Stats
    # ------------------------------------------------------------------

    @property
    def size(self) -> int:
        return self._queue.qsize()

    def get_stats(self) -> dict:
        return {
            "queue_size": self.size,
            "max_queue_size": self._max_size,
            "enqueued": self._enqueued,
            "completed": self._completed,
            "failed": self._failed,
            "retried": self._retried,
            "timed_out": self._timed_out,
            "dropped": self._dropped,
        }

    # ------------------------------------------------------------------
    # Worker
    # ------------------------------------------------------------------

    async def _worker(self, worker_id: int) -> None:
        """Long-running coroutine that drains the queue."""
        while self._running:
            try:
                item: _WorkItem = await asyncio.wait_for(
                    self._queue.get(), timeout=1.0
                )
            except asyncio.TimeoutError:
                continue  # poll loop — check self._running again
            except asyncio.CancelledError:
                break

            # Check deadline before we even start
            wait_time = time.monotonic() - item.enqueued_at
            if wait_time > self._timeout:
                self._timed_out += 1
                err = QueueTimeoutError(
                    f"Request waited {wait_time:.1f}s in queue (limit {self._timeout}s)"
                )
                if not item.future.done():
                    item.future.set_exception(err)
                self._queue.task_done()
                continue

            # Execute with retry
            await self._execute_with_retry(item)
            self._queue.task_done()

    async def _execute_with_retry(self, item: _WorkItem) -> None:
        delay = self._retry_delay
        last_error: Optional[Exception] = None

        while True:
            try:
                result = await item.fn()
                self._completed += 1
                if not item.future.done():
                    item.future.set_result(result)
                await self._persist_stats()
                return
            except Exception as exc:
                last_error = exc
                item.retries_left -= 1
                if item.retries_left <= 0:
                    break
                self._retried += 1
                logger.warning(
                    "RAL queue: retry agent=%s retries_left=%d error=%s",
                    item.agent_id, item.retries_left, exc,
                )
                await asyncio.sleep(delay)
                delay = min(delay * 2, self._timeout / 2)  # exponential back-off

        self._failed += 1
        await self._persist_stats()
        if not item.future.done():
            item.future.set_exception(
                last_error or RuntimeError("Unknown queue worker failure")
            )

    async def _persist_stats(self) -> None:
        """Write current queue stats to Redis so the backend API can read them."""
        if not self._redis:
            return
        try:
            stats = self.get_stats()
            k = f"{self._pfx}:metrics:queue"
            await self._redis.hset(k, mapping={str(kk): str(vv) for kk, vv in stats.items()})
            await self._redis.expire(k, ral_settings.RAL_METRICS_RETENTION)
        except Exception:
            pass  # never crash the hot path

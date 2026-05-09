"""
In-memory async queue infrastructure for overloaded AI requests.

The queue acts as a resilience buffer between rate limiting and agent
execution. It accepts over-limit work quickly, returns a queued response to the
API caller, and lets a background worker drain requests gradually. The manager
is FastAPI-free; Redis Streams, Celery, or Kafka can later replace this module
behind the same enqueue/status/worker contract.
"""

import asyncio
import time
from dataclasses import dataclass, field
from datetime import datetime
from enum import Enum
from typing import Any, Awaitable, Callable, Dict, Optional
from uuid import uuid4


class RequestStatus(str, Enum):
    """Request lifecycle states."""

    QUEUED = "queued"
    PROCESSING = "processing"
    COMPLETED = "completed"
    FAILED = "failed"
    CANCELLED = "cancelled"


@dataclass
class QueuedRequest:
    """
    Request wrapper with queue metadata.

    EXTENSION POINT:
    - Add priority fields
    - Add retry policy
    - Add trace/correlation IDs
    - Persist status in Redis/database for multi-instance visibility
    """

    agent: str
    query: str
    cache_key: str
    request_id: str = field(default_factory=lambda: str(uuid4()))
    status: RequestStatus = RequestStatus.QUEUED
    created_at: datetime = field(default_factory=datetime.utcnow)
    started_at: Optional[datetime] = None
    completed_at: Optional[datetime] = None
    retry_count: int = 0
    error_message: Optional[str] = None
    result: Optional[Dict[str, Any]] = None

    def to_dict(self) -> Dict[str, Any]:
        """Convert the queued request to a JSON-serializable dictionary."""
        return {
            "request_id": self.request_id,
            "agent": self.agent,
            "query": self.query,
            "cache_key": self.cache_key,
            "status": self.status.value,
            "created_at": self.created_at.isoformat(),
            "started_at": self.started_at.isoformat() if self.started_at else None,
            "completed_at": self.completed_at.isoformat()
            if self.completed_at
            else None,
            "retry_count": self.retry_count,
            "error_message": self.error_message,
            "result": self.result,
        }


@dataclass
class QueueAdmission:
    """Result returned when a request is admitted to or rejected by the queue."""

    accepted: bool
    status: str
    request_id: Optional[str]
    queue_position: int
    estimated_wait_time: float
    queue_size: int
    max_queue_size: int
    message: str

    def to_dict(self) -> Dict[str, Any]:
        """Convert admission result to a JSON-serializable dictionary."""
        return {
            "accepted": self.accepted,
            "status": self.status,
            "request_id": self.request_id,
            "queue_position": self.queue_position,
            "estimated_wait_time": self.estimated_wait_time,
            "queue_size": self.queue_size,
            "max_queue_size": self.max_queue_size,
            "message": self.message,
        }


QueueProcessor = Callable[[QueuedRequest], Awaitable[Dict[str, Any]]]


class QueueManager:
    """
    Async in-memory request queue with a single background worker.

    The queue acts as a small resilience buffer. It accepts over-limit requests
    quickly, returns a queued response to the API caller, and lets the worker
    drain work at a controlled pace. The processor callback is injected from
    the application layer so queue scheduling stays separate from business
    processing.
    """

    def __init__(
        self,
        max_queue_size: int = 100,
        worker_delay_seconds: float = 0.25,
        default_processing_seconds: float = 0.75,
        history_limit: int = 100,
    ):
        """
        Initialize queue manager.

        Args:
            max_queue_size: Maximum buffered requests.
            worker_delay_seconds: Small delay before each queued job to make
                smoothing visible and avoid a hot processing loop.
            default_processing_seconds: Fallback for initial wait estimates.
            history_limit: Completed/failed request records retained in memory.
        """
        self.max_queue_size = max_queue_size
        self.worker_delay_seconds = worker_delay_seconds
        self.default_processing_seconds = default_processing_seconds
        self.history_limit = history_limit

        # asyncio.Queue keeps route handlers non-blocking: enqueue is fast,
        # and the worker consumes at its own pace.
        self.queue: asyncio.Queue[QueuedRequest] = asyncio.Queue(
            maxsize=max_queue_size
        )
        self.queued_requests: Dict[str, QueuedRequest] = {}
        self.processing: Dict[str, QueuedRequest] = {}
        self.completed: Dict[str, QueuedRequest] = {}

        self.worker_task: Optional[asyncio.Task] = None
        self.worker_status = "stopped"
        self.processed_queue_count = 0
        self.failed_queue_count = 0
        self.queue_overflow_count = 0
        self._average_processing_time = default_processing_seconds
        self._processor: Optional[QueueProcessor] = None
        self._stop_requested = False

    async def enqueue_request(
        self,
        agent: str,
        query: str,
        cache_key: str,
    ) -> QueueAdmission:
        """
        Add an overloaded request to the async queue without blocking the route.

        Returns a QueueAdmission payload with position and estimated wait time.
        """
        if self.queue.full():
            # Queue overflow is the fallback when the resilience buffer itself
            # is saturated; callers can surface this as a retryable 503.
            self.queue_overflow_count += 1
            return QueueAdmission(
                accepted=False,
                status="queue_full",
                request_id=None,
                queue_position=0,
                estimated_wait_time=self.estimate_wait_time(self.queue.qsize()),
                queue_size=self.queue.qsize(),
                max_queue_size=self.max_queue_size,
                message="Queue is full; retry later.",
            )

        queued_request = QueuedRequest(
            agent=agent.strip().lower(),
            query=query,
            cache_key=cache_key,
        )
        queue_position = self.queue.qsize() + 1

        self.queue.put_nowait(queued_request)
        self.queued_requests[queued_request.request_id] = queued_request

        return QueueAdmission(
            accepted=True,
            status=RequestStatus.QUEUED.value,
            request_id=queued_request.request_id,
            queue_position=queue_position,
            estimated_wait_time=self.estimate_wait_time(queue_position),
            queue_size=self.queue.qsize(),
            max_queue_size=self.max_queue_size,
            message="Request accepted into the resilience queue.",
        )

    async def process_queue(self) -> None:
        """
        Continuously consume queued requests and process them in the background.

        The processor callback is provided by the application layer so this
        manager remains infrastructure-only. Redis/Celery/Kafka can later own
        this loop while preserving the same high-level contract.
        """
        if self._processor is None:
            raise RuntimeError("Queue worker cannot start without a processor")

        self.worker_status = "running"
        self._stop_requested = False

        while not self._stop_requested:
            try:
                # This await is the worker's idle state. It consumes no CPU
                # while waiting for the next overloaded request.
                queued_request = await self.queue.get()
            except asyncio.CancelledError:
                self.worker_status = "stopped"
                raise

            self._mark_processing(queued_request)
            started = time.perf_counter()

            try:
                if self.worker_delay_seconds > 0:
                    # A tiny delay makes queue draining visible in demos and
                    # avoids an unrealistically hot loop.
                    await asyncio.sleep(self.worker_delay_seconds)

                # Business processing is delegated to the application callback.
                result = await self._processor(queued_request)
            except asyncio.CancelledError:
                queued_request.status = RequestStatus.CANCELLED
                self.worker_status = "stopped"
                raise
            except Exception as exc:
                self.failed_queue_count += 1
                self._mark_failed(queued_request, str(exc))
            else:
                elapsed = time.perf_counter() - started
                self._update_average_processing_time(elapsed)
                self.processed_queue_count += 1
                self._mark_completed(queued_request, result)
            finally:
                self.processing.pop(queued_request.request_id, None)
                self.queue.task_done()

        self.worker_status = "stopped"

    def start_worker(self, processor: QueueProcessor) -> asyncio.Task:
        """
        Start the background worker if it is not already running.

        Args:
            processor: Async callback that performs cache/agent/stat work for
                each QueuedRequest.
        """
        self._processor = processor

        if self.worker_task and not self.worker_task.done():
            return self.worker_task

        self.worker_task = asyncio.create_task(self.process_queue())
        self.worker_status = "starting"
        return self.worker_task

    async def stop_worker(self) -> None:
        """Stop the background worker during application shutdown."""
        self._stop_requested = True

        if self.worker_task and not self.worker_task.done():
            self.worker_task.cancel()
            try:
                await self.worker_task
            except asyncio.CancelledError:
                pass

        self.worker_status = "stopped"

    def get_queue_size(self) -> int:
        """Return the number of requests waiting in the queue."""
        return self.queue.qsize()

    def get_queue_status(self) -> Dict[str, Any]:
        """Return live queue depth, worker state, and drain-rate estimates."""
        queue_size = self.get_queue_size()
        return {
            "queue_size": queue_size,
            "current_queue_size": queue_size,
            "max_queue_size": self.max_queue_size,
            "processing_count": len(self.processing),
            "worker_status": self.worker_status,
            "worker_running": self.worker_task is not None
            and not self.worker_task.done(),
            "processed_queue_count": self.processed_queue_count,
            "processed_from_queue": self.processed_queue_count,
            "failed_queue_count": self.failed_queue_count,
            "queue_overflow_count": self.queue_overflow_count,
            "estimated_wait_time": self.estimate_wait_time(queue_size),
            "average_processing_time": self._average_processing_time,
            "utilization_percent": (
                queue_size / self.max_queue_size * 100
                if self.max_queue_size > 0
                else 0
            ),
        }

    def get_queue_stats(self) -> Dict[str, Any]:
        """Backward-compatible alias for queue monitoring."""
        return self.get_queue_status()

    def get_request_status(self, request_id: str) -> Optional[Dict[str, Any]]:
        """Get status for a queued, processing, completed, or failed request."""
        request = (
            self.queued_requests.get(request_id)
            or self.processing.get(request_id)
            or self.completed.get(request_id)
        )

        if request is None:
            return None

        return request.to_dict()

    def estimate_wait_time(self, queue_position: int) -> float:
        """Estimate wait time from queue position and observed processing time."""
        if queue_position <= 0:
            return 0.0

        per_request_seconds = max(
            self._average_processing_time,
            self.default_processing_seconds,
        )
        return round(queue_position * per_request_seconds, 3)

    def clear_queue(self) -> None:
        """Clear queued requests. Processing work is left untouched."""
        while not self.queue.empty():
            try:
                request = self.queue.get_nowait()
            except asyncio.QueueEmpty:
                break

            request.status = RequestStatus.CANCELLED
            self.queued_requests.pop(request.request_id, None)
            self.queue.task_done()

    def _mark_processing(self, queued_request: QueuedRequest) -> None:
        """
        Move a request from waiting to active processing state.

        This state split keeps queue status useful during demos and mirrors
        how a durable queue backend would expose pending vs. in-flight jobs.
        """
        queued_request.status = RequestStatus.PROCESSING
        queued_request.started_at = datetime.utcnow()
        self.queued_requests.pop(queued_request.request_id, None)
        self.processing[queued_request.request_id] = queued_request

    def _mark_completed(
        self,
        queued_request: QueuedRequest,
        result: Optional[Dict[str, Any]],
    ) -> None:
        """Record successful worker output and retain bounded history."""
        queued_request.status = RequestStatus.COMPLETED
        queued_request.completed_at = datetime.utcnow()
        queued_request.result = result
        self._remember_completed(queued_request)

    def _mark_failed(self, queued_request: QueuedRequest, error: str) -> None:
        """Record worker failure without stopping the queue loop."""
        queued_request.status = RequestStatus.FAILED
        queued_request.completed_at = datetime.utcnow()
        queued_request.error_message = error
        self._remember_completed(queued_request)

    def _remember_completed(self, queued_request: QueuedRequest) -> None:
        """
        Retain a small in-memory completion history for request inspection.

        A production queue would persist this status externally; bounded local
        history keeps this MVP observable without unbounded memory growth.
        """
        self.completed[queued_request.request_id] = queued_request

        while len(self.completed) > self.history_limit:
            oldest_request_id = next(iter(self.completed))
            self.completed.pop(oldest_request_id, None)

    def _update_average_processing_time(self, elapsed_seconds: float) -> None:
        """Maintain a lightweight running average for wait-time estimates."""
        if self.processed_queue_count == 0:
            self._average_processing_time = elapsed_seconds
            return

        observed = self.processed_queue_count
        self._average_processing_time = (
            (self._average_processing_time * observed) + elapsed_seconds
        ) / (observed + 1)


# Global queue manager instance
queue_manager = QueueManager()

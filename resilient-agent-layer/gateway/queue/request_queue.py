import asyncio
import time
from dataclasses import dataclass, field
from typing import Any, Dict, Optional

from gateway.config import DEFAULT_QUEUE_MAX_SIZE, DEFAULT_QUEUE_TIMEOUT_MS


@dataclass
class QueuedRequest:
    agent_id: str
    payload: Any
    headers: dict
    future: asyncio.Future = field(default_factory=asyncio.Future)
    enqueued_at: float = field(default_factory=time.monotonic)
    priority: int = 0  # lower = higher priority


class AgentQueue:
    """Per-agent async priority queue with configurable depth and timeout."""

    def __init__(self, agent_id: str, max_size: int = DEFAULT_QUEUE_MAX_SIZE, timeout_ms: int = DEFAULT_QUEUE_TIMEOUT_MS):
        self.agent_id = agent_id
        self.max_size = max_size
        self.timeout_ms = timeout_ms
        self._queue: asyncio.PriorityQueue = asyncio.PriorityQueue()
        self._size = 0

        # Stats
        self._total_enqueued = 0
        self._total_dequeued = 0
        self._total_timed_out = 0
        self._total_rejected = 0

    @property
    def depth(self) -> int:
        return self._size

    @property
    def is_full(self) -> bool:
        return self._size >= self.max_size

    async def enqueue(self, request: QueuedRequest) -> bool:
        """
        Enqueue a request. Returns False immediately if queue is full.
        The caller awaits request.future to get the result.
        """
        if self.is_full:
            self._total_rejected += 1
            return False

        self._size += 1
        self._total_enqueued += 1
        # PriorityQueue orders by first element; use negative priority for min-heap
        await self._queue.put((-request.priority, time.monotonic(), request))
        return True

    async def dequeue_loop(self, handler):
        """
        Background worker that dequeues requests and calls handler(request).
        Handler should set request.future result when done.
        """
        while True:
            _, _, request = await self._queue.get()
            self._size -= 1
            self._total_dequeued += 1

            # Check if request timed out while in queue
            wait_ms = (time.monotonic() - request.enqueued_at) * 1000
            timeout_ms = self.timeout_ms

            if wait_ms > timeout_ms:
                self._total_timed_out += 1
                if not request.future.done():
                    request.future.set_exception(
                        asyncio.TimeoutError(
                            f"Request queued for {wait_ms:.0f}ms, exceeded timeout {timeout_ms}ms"
                        )
                    )
            else:
                try:
                    result = await handler(request)
                    if not request.future.done():
                        request.future.set_result(result)
                except Exception as e:
                    if not request.future.done():
                        request.future.set_exception(e)

            self._queue.task_done()

    def get_stats(self) -> dict:
        return {
            "agent_id": self.agent_id,
            "current_depth": self.depth,
            "max_size": self.max_size,
            "timeout_ms": self.timeout_ms,
            "total_enqueued": self._total_enqueued,
            "total_dequeued": self._total_dequeued,
            "total_timed_out": self._total_timed_out,
            "total_rejected": self._total_rejected,
        }


class QueueManager:
    """Manages per-agent queues and their background workers."""

    def __init__(self):
        self._queues: Dict[str, AgentQueue] = {}
        self._workers: Dict[str, asyncio.Task] = {}

    def get_or_create(self, agent_id: str, max_size: int = DEFAULT_QUEUE_MAX_SIZE, timeout_ms: int = DEFAULT_QUEUE_TIMEOUT_MS) -> AgentQueue:
        if agent_id not in self._queues:
            self._queues[agent_id] = AgentQueue(agent_id, max_size, timeout_ms)
        return self._queues[agent_id]

    def start_worker(self, agent_id: str, handler):
        if agent_id not in self._workers or self._workers[agent_id].done():
            queue = self.get_or_create(agent_id)
            self._workers[agent_id] = asyncio.create_task(
                queue.dequeue_loop(handler), name=f"queue-worker-{agent_id}"
            )

    def get_all_stats(self) -> dict:
        return {
            "queues": {aid: q.get_stats() for aid, q in self._queues.items()},
            "total_depth": sum(q.depth for q in self._queues.values()),
        }

    def get_agent_stats(self, agent_id: str) -> Optional[dict]:
        q = self._queues.get(agent_id)
        return q.get_stats() if q else None


# Singleton
queue_manager = QueueManager()

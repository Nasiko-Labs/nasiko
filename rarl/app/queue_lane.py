import asyncio
import collections
import itertools
import time
from typing import Any, Awaitable, Callable

from app.ratelimit import TokenBucket


class QueueOverflow(Exception):
    pass


class AgentLane:
    _seq = itertools.count()

    def __init__(
        self,
        agent_id: str,
        rps: float,
        burst: int,
        max_inflight: int,
        max_queue: int = 100,
    ) -> None:
        self.agent_id = agent_id
        self.bucket = TokenBucket(rps, burst)
        self.queue: asyncio.PriorityQueue = asyncio.PriorityQueue(maxsize=max_queue)
        self.max_inflight = max_inflight
        self.semaphore = asyncio.Semaphore(max_inflight)
        self.latencies: collections.deque[float] = collections.deque(maxlen=50)
        self.ema = 0.5
        self.served = 0
        self.queued_total = 0
        self.rejected = 0
        self._worker_task = asyncio.create_task(self._worker())

    @property
    def queue_depth(self) -> int:
        return self.queue.qsize()

    @property
    def p95(self) -> float:
        if not self.latencies:
            return 0.0
        s = sorted(self.latencies)
        return s[max(0, int(0.95 * len(s)) - 1)]

    def _record_latency(self, dur: float) -> None:
        self.latencies.append(dur)
        self.ema = 0.2 * dur + 0.8 * self.ema

    async def submit(
        self, priority: int, factory: Callable[[], Awaitable[Any]]
    ) -> tuple[Any, dict]:
        """Returns (result, meta) where meta = {"queue_position", "eta_seconds", "wait_seconds"}."""
        if self.bucket.try_acquire() and self.queue.empty() and not self.semaphore.locked():
            t0 = time.monotonic()
            async with self.semaphore:
                started = time.monotonic()
                try:
                    result = await factory()
                    return result, {
                        "queue_position": 0,
                        "eta_seconds": 0.0,
                        "wait_seconds": time.monotonic() - t0,
                    }
                finally:
                    self._record_latency(time.monotonic() - started)
                    self.served += 1

        if self.queue.full():
            self.rejected += 1
            raise QueueOverflow(f"queue full for agent {self.agent_id}")

        position = self.queue.qsize()
        eta = (position + 1) * self.ema / max(1, self.max_inflight)
        fut: asyncio.Future = asyncio.get_running_loop().create_future()
        enqueued_at = time.monotonic()
        await self.queue.put((priority, next(AgentLane._seq), fut, factory, enqueued_at))
        self.queued_total += 1

        result = await fut
        return result, {
            "queue_position": position,
            "eta_seconds": eta,
            "wait_seconds": time.monotonic() - enqueued_at,
        }

    async def _worker(self) -> None:
        while True:
            try:
                priority, _, fut, factory, _ = await self.queue.get()
            except asyncio.CancelledError:
                break

            try:
                while True:
                    if self.bucket.try_acquire():
                        break
                    await asyncio.sleep(self.bucket.time_until_available() + 0.01)

                async with self.semaphore:
                    started = time.monotonic()
                    try:
                        result = await factory()
                        if not fut.done():
                            fut.set_result(result)
                    except Exception as e:
                        if not fut.done():
                            fut.set_exception(e)
                    finally:
                        self._record_latency(time.monotonic() - started)
                        self.served += 1
            finally:
                self.queue.task_done()

"""
Standalone request-management middleware skeleton for the Nasiko hackathon MVP.

This module is intentionally simple. It does not wire into the existing Nasiko
application yet. The goal is to provide a clean, hackathon-friendly foundation
that can later be connected to the router service or a FastAPI endpoint.

The request path is:
1. Receive a request.
2. Generate a stable cache key.
3. Check the in-memory cache.
4. Apply per-agent rate limiting.
5. Queue overflow requests.
6. Forward the request to a placeholder agent executor.
7. Store metrics.
8. Return the response.

TODO: Replace the placeholder agent executor with real Nasiko router/agent calls.
TODO: Replace in-memory state with Redis or another shared store after the MVP.
"""

from __future__ import annotations

import asyncio
import hashlib
import json
import logging
import time
from dataclasses import dataclass, field
from typing import Any, Dict, Optional


logger = logging.getLogger(__name__)


@dataclass
class RequestContext:
    """Small container for one incoming request.

    Keeping the request payload in a dataclass makes the code easier to read
    and gives us one place to attach request metadata later.
    """

    request_id: str
    agent_id: str
    payload: Dict[str, Any]
    created_at: float = field(default_factory=time.time)


class CacheManager:
    """Very small in-memory cache for hackathon demo purposes.

    This cache is intentionally simple: a Python dictionary with TTL support.
    It is good enough for a demo and easy to replace later.
    """

    def __init__(self, ttl_seconds: int = 30):
        self.ttl_seconds = ttl_seconds
        self._cache: Dict[str, Dict[str, Any]] = {}

    def get(self, key: str) -> Optional[Any]:
        """Return cached data if it is still fresh."""
        entry = self._cache.get(key)
        if not entry:
            return None

        age = time.time() - entry["created_at"]
        if age > self.ttl_seconds:
            # Cache entry is stale, so remove it and behave like a miss.
            self._cache.pop(key, None)
            return None

        return entry["value"]

    def set(self, key: str, value: Any) -> None:
        """Store a response in memory for later reuse."""
        self._cache[key] = {
            "value": value,
            "created_at": time.time(),
        }

    def get_entry(self, key: str) -> Optional[Dict[str, Any]]:
        """Return the full cache entry so the demo can report cache timing.

        The response value is still the important part, but the timestamp is
        useful when we want to explain why a repeated request was fast.
        """

        entry = self._cache.get(key)
        if not entry:
            return None

        age = time.time() - entry["created_at"]
        if age > self.ttl_seconds:
            # If the cached result is too old, throw it away and behave like a miss.
            self._cache.pop(key, None)
            return None

        return entry

    def clear(self) -> None:
        """Remove all cached values."""
        self._cache.clear()


class RateLimiter:
    """Per-agent rate limiter with a tiny in-memory token counter.

    This limiter is deliberately basic. It tracks requests per agent in a
    rolling time window and tells the request manager whether the request can
    run immediately or should be queued.
    """

    def __init__(
        self,
        max_requests: int = 5,
        window_seconds: int = 10,
        agent_limits: Optional[Dict[str, Dict[str, int]]] = None,
    ):
        self.max_requests = max_requests
        self.window_seconds = window_seconds
        # Per-agent policy keeps the demo easy to tune for different workloads.
        self.agent_limits = agent_limits or {}
        self._requests: Dict[str, list[float]] = {}
        self._active_counts: Dict[str, int] = {}

    def _get_policy(self, agent_id: str) -> tuple[int, int]:
        """Return the configured limit for a specific agent."""

        policy = self.agent_limits.get(agent_id, {})
        max_requests = int(policy.get("max_requests", self.max_requests))
        window_seconds = int(policy.get("window_seconds", self.window_seconds))
        return max_requests, window_seconds

    def allow(self, agent_id: str) -> bool:
        """Check whether the agent can receive another immediate request."""
        now = time.time()
        max_requests, window_seconds = self._get_policy(agent_id)
        window_start = now - window_seconds

        timestamps = self._requests.setdefault(agent_id, [])
        # Keep only requests that are still inside the active window.
        timestamps[:] = [ts for ts in timestamps if ts >= window_start]

        if len(timestamps) >= max_requests:
            return False

        timestamps.append(now)
        return True

    def get_window_state(self, agent_id: str) -> Dict[str, int]:
        """Return the current request-window shape for logging and stats."""

        timestamps = self._requests.get(agent_id, [])
        max_requests, window_seconds = self._get_policy(agent_id)
        now = time.time()
        window_start = now - window_seconds
        active_window_requests = len([ts for ts in timestamps if ts >= window_start])
        return {
            "active_window_requests": active_window_requests,
            "max_requests": max_requests,
            "active_requests": self._active_counts.get(agent_id, 0),
        }

    def mark_active_start(self, agent_id: str) -> None:
        """Track when a request starts executing for one agent."""

        self._active_counts[agent_id] = self._active_counts.get(agent_id, 0) + 1

    def mark_active_end(self, agent_id: str) -> None:
        """Track when a request finishes executing for one agent."""

        if agent_id not in self._active_counts:
            return

        if self._active_counts[agent_id] <= 1:
            self._active_counts.pop(agent_id, None)
        else:
            self._active_counts[agent_id] -= 1


@dataclass
class QueueItem:
    """Work item stored in the overflow queue."""

    context: RequestContext
    future: asyncio.Future
    queued_at: float = field(default_factory=time.time)


class MetricsCollector:
    """In-memory metrics for the MVP.

    This collector keeps the demo simple while still showing the important
    operational signals: cache hits, cache misses, queue depth, rate-limit hits,
    and successful/failed request counts.
    """

    def __init__(self):
        self.total_requests = 0
        self.cache_hits = 0
        self.cache_misses = 0
        self.rate_limited = 0
        self.queued_requests = 0
        self.active_requests = 0
        self.completed_requests = 0
        self.failed_requests = 0
        self.total_latency_ms = 0.0
        self.total_queue_wait_ms = 0.0
        self.queue_wait_samples = 0
        self.per_agent: Dict[str, Dict[str, float]] = {}

    def _agent_bucket(self, agent_id: str) -> Dict[str, float]:
        """Create or fetch the stats bucket for one agent."""

        if agent_id not in self.per_agent:
            self.per_agent[agent_id] = {
                "requests_processed": 0,
                "rate_limit_triggers": 0,
                "total_response_time_ms": 0.0,
                "active_requests": 0,
            }
        return self.per_agent[agent_id]

    def record_request(self) -> None:
        self.total_requests += 1

    def record_cache_hit(self) -> None:
        self.cache_hits += 1

    def record_cache_miss(self) -> None:
        self.cache_misses += 1

    def record_rate_limited(self, agent_id: Optional[str] = None) -> None:
        self.rate_limited += 1
        if agent_id is not None:
            self._agent_bucket(agent_id)["rate_limit_triggers"] += 1

    def record_queued(self) -> None:
        self.queued_requests += 1

    def record_active_start(self, agent_id: Optional[str] = None) -> None:
        self.active_requests += 1
        if agent_id is not None:
            self._agent_bucket(agent_id)["active_requests"] += 1

    def record_active_end(self, agent_id: Optional[str] = None) -> None:
        if self.active_requests > 0:
            self.active_requests -= 1
        if agent_id is not None:
            bucket = self._agent_bucket(agent_id)
            if bucket["active_requests"] > 0:
                bucket["active_requests"] -= 1

    def record_completed(self, latency_ms: float, agent_id: Optional[str] = None) -> None:
        self.completed_requests += 1
        self.total_latency_ms += latency_ms
        if agent_id is not None:
            bucket = self._agent_bucket(agent_id)
            bucket["requests_processed"] += 1
            bucket["total_response_time_ms"] += latency_ms

    def record_failed(self) -> None:
        self.failed_requests += 1

    def record_queue_wait(self, wait_ms: float) -> None:
        self.total_queue_wait_ms += wait_ms
        self.queue_wait_samples += 1

    def snapshot(self) -> Dict[str, Any]:
        """Return a simple metrics snapshot for debugging or a /metrics endpoint."""
        average_latency_ms = (
            self.total_latency_ms / self.completed_requests
            if self.completed_requests
            else 0.0
        )
        average_queue_wait_ms = (
            self.total_queue_wait_ms / self.queue_wait_samples
            if self.queue_wait_samples
            else 0.0
        )

        return {
            "total_requests": self.total_requests,
            "cache_hits": self.cache_hits,
            "cache_misses": self.cache_misses,
            "cache_hit_rate": (
                self.cache_hits / self.total_requests if self.total_requests else 0.0
            ),
            "rate_limited": self.rate_limited,
            "queued_requests": self.queued_requests,
            "active_requests": self.active_requests,
            "completed_requests": self.completed_requests,
            "failed_requests": self.failed_requests,
            "average_latency_ms": average_latency_ms,
            "average_queue_wait_ms": average_queue_wait_ms,
            "queue_depth": None,
            "per_agent": {
                agent_id: {
                    "requests_processed": int(stats["requests_processed"]),
                    "rate_limit_triggers": int(stats["rate_limit_triggers"]),
                    "active_requests": int(stats["active_requests"]),
                    "average_response_time_ms": (
                        stats["total_response_time_ms"] / stats["requests_processed"]
                        if stats["requests_processed"]
                        else 0.0
                    ),
                }
                for agent_id, stats in self.per_agent.items()
            },
        }


class RequestManager:
    """Orchestrates caching, rate limiting, queueing, and request execution.

    This class is the main entry point for the middleware skeleton. Later, it
    can be called from a FastAPI route or middleware layer.
    """

    def __init__(
        self,
        cache_ttl_seconds: int = 30,
        rate_limit_max_requests: int = 5,
        rate_limit_window_seconds: int = 10,
        queue_maxsize: int = 100,
        mock_delay_seconds: float = 0.05,
        agent_limits: Optional[Dict[str, Dict[str, int]]] = None,
    ):
        self.cache_manager = CacheManager(ttl_seconds=cache_ttl_seconds)
        self.rate_limiter = RateLimiter(
            max_requests=rate_limit_max_requests,
            window_seconds=rate_limit_window_seconds,
            agent_limits=agent_limits,
        )
        self.metrics = MetricsCollector()
        self.queue: asyncio.Queue[QueueItem] = asyncio.Queue(maxsize=queue_maxsize)
        self._queue_worker_task: Optional[asyncio.Task] = None
        self.mock_delay_seconds = mock_delay_seconds

    def generate_request_key(self, context: RequestContext) -> str:
        """Build a stable cache key from the agent id and request payload.

        This is the simplest useful cache key for the MVP. If the same agent
        receives the same payload, we can reuse the prior response.
        """

        normalized_payload = json.dumps(context.payload, sort_keys=True, default=str)
        raw_key = f"{context.agent_id}:{normalized_payload}"
        return hashlib.sha256(raw_key.encode("utf-8")).hexdigest()

    async def start(self) -> None:
        """Start the queue worker once the application is ready."""
        if self._queue_worker_task is None:
            self._queue_worker_task = asyncio.create_task(self._queue_worker())
            logger.warning("QUEUE PROCESSOR ACTIVE")

    async def stop(self) -> None:
        """Stop the queue worker cleanly."""
        if self._queue_worker_task:
            self._queue_worker_task.cancel()
            try:
                await self._queue_worker_task
            except asyncio.CancelledError:
                pass
            self._queue_worker_task = None

    async def handle_request(
        self,
        agent_id: str,
        payload: Dict[str, Any],
        request_id: Optional[str] = None,
    ) -> Dict[str, Any]:
        """Main request entrypoint.

        This is the method that later can be called from a FastAPI endpoint.
        The flow is intentionally simple so it is easy to explain in a demo.
        """

        context = RequestContext(
            request_id=request_id or self._generate_request_id(),
            agent_id=agent_id,
            payload=payload,
        )

        self.metrics.record_request()
        request_key = self.generate_request_key(context)

        # Step 1: Cache lookup.
        cached_response = self.cache_manager.get(request_key)
        if cached_response is not None:
            self.metrics.record_cache_hit()
            cache_entry = self.cache_manager.get_entry(request_key)
            cached_at = cache_entry["created_at"] if cache_entry else None
            logger.info(
                "CACHE HIT agent_id=%s request_id=%s cached_at=%s",
                agent_id,
                context.request_id,
                cached_at,
            )
            logger.info(
                "RESPONSE SERVED FROM CACHE agent_id=%s request_id=%s",
                agent_id,
                context.request_id,
            )
            return {
                "source": "cache",
                "request_id": context.request_id,
                "agent_id": agent_id,
                "response": cached_response,
                "cache": {
                    "cached_at": cached_at,
                    "served_from_cache": True,
                },
            }

        self.metrics.record_cache_miss()
        logger.info("CACHE MISS agent_id=%s request_id=%s", agent_id, context.request_id)

        # Step 2: Rate limiting.
        if not self.rate_limiter.allow(agent_id):
            self.metrics.record_rate_limited(agent_id)
            window_state = self.rate_limiter.get_window_state(agent_id)
            logger.warning(
                "RATE LIMIT TRIGGERED agent_id=%s request_id=%s active_window_requests=%s max_requests=%s active_requests=%s",
                agent_id,
                context.request_id,
                window_state["active_window_requests"],
                window_state["max_requests"],
                window_state["active_requests"],
            )
            logger.warning(
                "OVERLOAD PROTECTION ACTIVE agent_id=%s request_id=%s queue_depth=%s",
                agent_id,
                context.request_id,
                self.queue.qsize(),
            )

            # Step 3: Queue overflow traffic instead of dropping it immediately.
            queued_response = await self._enqueue_and_wait(context)
            return queued_response

        # Step 4: Execute the request immediately.
        return await self._execute_and_store(context, request_key)

    async def _enqueue_and_wait(self, context: RequestContext) -> Dict[str, Any]:
        """Place the request into the overflow queue and wait for processing.

        TODO: Later this can become a distributed queue backed by Redis or a
        task broker. For the MVP, asyncio.Queue is enough to demonstrate the flow.
        """

        loop = asyncio.get_running_loop()
        future: asyncio.Future = loop.create_future()
        item = QueueItem(context=context, future=future)

        self.metrics.record_queued()
        logger.warning(
            "REQUEST QUEUED agent_id=%s request_id=%s queue_depth=%s maxsize=%s",
            context.agent_id,
            context.request_id,
            self.queue.qsize(),
            self.queue.maxsize,
        )
        # asyncio.Queue acts like a safe waiting room here.
        # If the queue is temporarily full, this request pauses instead of being dropped.
        await self.queue.put(item)

        # Wait for the queue worker to complete the request.
        return await future

    async def _queue_worker(self) -> None:
        """Background task that processes queued requests in FIFO order.

        Queueing is better than dropping traffic because it converts overload
        into a short wait instead of an immediate failure.
        """

        while True:
            item = await self.queue.get()
            try:
                logger.warning(
                    "REQUEST DEQUEUED agent_id=%s request_id=%s queue_depth=%s",
                    item.context.agent_id,
                    item.context.request_id,
                    self.queue.qsize(),
                )
                queue_wait_ms = (time.time() - item.queued_at) * 1000
                self.metrics.record_queue_wait(queue_wait_ms)
                logger.warning(
                    "queue_wait_time agent_id=%s request_id=%s wait_ms=%.2f",
                    item.context.agent_id,
                    item.context.request_id,
                    queue_wait_ms,
                )
                request_key = self.generate_request_key(item.context)
                response = await self._execute_and_store(item.context, request_key)

                if not item.future.done():
                    item.future.set_result(response)
                logger.warning(
                    "request_completed_from_queue agent_id=%s request_id=%s",
                    item.context.agent_id,
                    item.context.request_id,
                )
            except Exception as exc:
                self.metrics.record_failed()
                if not item.future.done():
                    item.future.set_result(
                        {
                            "source": "queue",
                            "request_id": item.context.request_id,
                            "agent_id": item.context.agent_id,
                            "response": {
                                "status": "error",
                                "message": f"Queued request failed: {exc}",
                            },
                        }
                    )
            finally:
                self.queue.task_done()

    async def _execute_and_store(
        self, context: RequestContext, request_key: str
    ) -> Dict[str, Any]:
        """Run the placeholder agent executor and store the response in cache."""

        start_time = time.time()
        self.metrics.record_active_start(context.agent_id)
        self.rate_limiter.mark_active_start(context.agent_id)

        try:
            # TODO: Replace this mock with the real Nasiko agent invocation.
            agent_response = await self._mock_agent_execute(context)

            # Cache the successful result so identical requests can be served faster.
            self.cache_manager.set(request_key, agent_response)

            latency_ms = (time.time() - start_time) * 1000
            self.metrics.record_completed(latency_ms, context.agent_id)
            logger.info(
                "request_processed agent_id=%s request_id=%s latency_ms=%.2f",
                context.agent_id,
                context.request_id,
                latency_ms,
            )

            return {
                "source": "agent",
                "request_id": context.request_id,
                "agent_id": context.agent_id,
                "response": agent_response,
                "latency_ms": round(latency_ms, 2),
            }
        except Exception as exc:
            self.metrics.record_failed()
            raise RuntimeError(f"Request execution failed: {exc}") from exc
        finally:
            self.metrics.record_active_end(context.agent_id)
            self.rate_limiter.mark_active_end(context.agent_id)

    async def _mock_agent_execute(self, context: RequestContext) -> Dict[str, Any]:
        """Placeholder agent call used for the MVP skeleton.

        This function simulates work so the request manager can demonstrate the
        full request lifecycle before Nasiko integration is added.
        """

        # TODO: Replace this with the real router-to-agent execution path in Nasiko.
        await asyncio.sleep(self.mock_delay_seconds)
        return {
            "status": "ok",
            "message": "Mock agent executed successfully",
            "agent_id": context.agent_id,
            "echo": context.payload,
        }

    def get_metrics(self) -> Dict[str, Any]:
        """Return metrics along with current queue depth."""

        snapshot = self.metrics.snapshot()
        snapshot["queue_depth"] = self.queue.qsize()
        snapshot["queue_maxsize"] = self.queue.maxsize
        snapshot["overload_protection"] = {
            "queue_processor_active": self._queue_worker_task is not None,
            "per_agent_limits": self.rate_limiter.agent_limits,
        }
        return snapshot

    def _generate_request_id(self) -> str:
        """Create a simple request id for tracking."""

        return hashlib.sha256(f"{time.time()}".encode("utf-8")).hexdigest()[:12]


__all__ = [
    "CacheManager",
    "MetricsCollector",
    "QueueItem",
    "RateLimiter",
    "RequestContext",
    "RequestManager",
]


async def _demo() -> None:
    """Small demonstration so the file can be run directly during the hackathon."""

    manager = RequestManager(cache_ttl_seconds=60, rate_limit_max_requests=2)
    await manager.start()

    # First request goes to the mock agent.
    first = await manager.handle_request(
        agent_id="translator",
        payload={"prompt": "translate hello to spanish"},
    )

    # Second request with the same payload should hit the cache.
    second = await manager.handle_request(
        agent_id="translator",
        payload={"prompt": "translate hello to spanish"},
    )

    print(
        json.dumps(
            {"first": first, "second": second, "metrics": manager.get_metrics()},
            indent=2,
        )
    )

    await manager.stop()


if __name__ == "__main__":
    asyncio.run(_demo())
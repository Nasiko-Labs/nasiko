"""
Per-agent rate limiter with async request queue.

Design
------
- Token-bucket algorithm: each agent gets a bucket of `capacity` tokens that
  refills at `rate` tokens/second.
- Requests that arrive when the bucket is empty are placed in an asyncio queue
  (up to `queue_size` items) and retried after a short wait.
- Requests that cannot be queued (queue full) are rejected with a 429 response.
- All state is in-process (no Redis dependency) so it works even without Redis.
  For multi-process deployments, Redis-backed counters can be added later.

Usage
-----
    limiter = RateLimiter()
    async with limiter.acquire("my-agent"):
        response = await call_agent(...)
"""

import asyncio
import logging
import time
from contextlib import asynccontextmanager
from dataclasses import dataclass, field
from typing import Any, AsyncGenerator, Dict, Optional

from router.src.config import settings

logger = logging.getLogger(__name__)


# ---------------------------------------------------------------------------
# Data structures
# ---------------------------------------------------------------------------

@dataclass
class AgentBucket:
    """Token-bucket state for a single agent."""

    agent_name: str
    capacity: float          # max tokens (burst size)
    rate: float              # tokens added per second
    tokens: float            # current token count
    last_refill: float       # epoch seconds of last refill
    queue: asyncio.Queue     # pending requests waiting for a token
    # Metrics
    total_requests: int = 0
    accepted_requests: int = 0
    queued_requests: int = 0
    rejected_requests: int = 0
    total_wait_ms: float = 0.0

    def refill(self) -> None:
        """Add tokens based on elapsed time since last refill."""
        now = time.monotonic()
        elapsed = now - self.last_refill
        self.tokens = min(self.capacity, self.tokens + elapsed * self.rate)
        self.last_refill = now

    def try_consume(self) -> bool:
        """Attempt to consume one token. Returns True on success."""
        self.refill()
        if self.tokens >= 1.0:
            self.tokens -= 1.0
            return True
        return False

    @property
    def queue_depth(self) -> int:
        return self.queue.qsize()

    @property
    def avg_wait_ms(self) -> float:
        if self.queued_requests == 0:
            return 0.0
        return round(self.total_wait_ms / self.queued_requests, 2)


@dataclass
class RateLimitConfig:
    """Per-agent rate limit configuration."""
    requests_per_second: float
    burst_capacity: int
    queue_size: int


# ---------------------------------------------------------------------------
# RateLimiter
# ---------------------------------------------------------------------------

class RateLimitExceeded(Exception):
    """Raised when a request cannot be queued (queue full)."""

    def __init__(self, agent_name: str, queue_depth: int):
        self.agent_name = agent_name
        self.queue_depth = queue_depth
        super().__init__(
            f"Rate limit exceeded for agent '{agent_name}'. "
            f"Queue is full ({queue_depth} items). Try again later."
        )


class RateLimiter:
    """
    Per-agent token-bucket rate limiter with async queuing.

    Configuration is read from settings at construction time but can be
    overridden per-agent via `configure_agent()`.
    """

    def __init__(self):
        self._buckets: Dict[str, AgentBucket] = {}
        self._lock = asyncio.Lock()
        # Per-agent overrides: agent_name → RateLimitConfig
        self._overrides: Dict[str, RateLimitConfig] = {}

    # ------------------------------------------------------------------
    # Public API
    # ------------------------------------------------------------------

    @asynccontextmanager
    async def acquire(self, agent_name: str) -> AsyncGenerator[None, None]:
        """
        Async context manager that blocks until a token is available.

        Raises RateLimitExceeded if the queue is full or the wait times out.
        """
        bucket = await self._get_or_create_bucket(agent_name)
        bucket.total_requests += 1
        enqueue_time = time.monotonic()

        # Fast path: token available immediately
        got_token = False
        async with self._lock:
            if bucket.try_consume():
                got_token = True

        if got_token:
            bucket.accepted_requests += 1
            logger.debug(
                f"Rate limiter: immediate token for agent={agent_name}, "
                f"tokens_left={bucket.tokens:.2f}"
            )
            yield
            return

        # Slow path: queue the request
        if bucket.queue.full():
            bucket.rejected_requests += 1
            logger.warning(
                f"Rate limiter: queue full for agent={agent_name} "
                f"(depth={bucket.queue_depth})"
            )
            raise RateLimitExceeded(agent_name, bucket.queue_depth)

        # Put a future in the queue; the background drain worker will resolve it
        loop = asyncio.get_running_loop()
        fut: asyncio.Future = loop.create_future()
        await bucket.queue.put(fut)
        bucket.queued_requests += 1

        logger.debug(
            f"Rate limiter: queued request for agent={agent_name}, "
            f"queue_depth={bucket.queue_depth}"
        )

        # Ensure the drain task is running
        asyncio.ensure_future(self._drain_queue(agent_name))

        try:
            await asyncio.wait_for(fut, timeout=settings.RATE_LIMIT_QUEUE_TIMEOUT)
        except asyncio.TimeoutError:
            bucket.rejected_requests += 1
            logger.warning(
                f"Rate limiter: queue timeout for agent={agent_name} "
                f"after {settings.RATE_LIMIT_QUEUE_TIMEOUT}s"
            )
            raise RateLimitExceeded(agent_name, bucket.queue_depth)

        wait_ms = (time.monotonic() - enqueue_time) * 1000
        bucket.total_wait_ms += wait_ms
        bucket.accepted_requests += 1
        logger.debug(
            f"Rate limiter: queued request granted for agent={agent_name}, "
            f"wait={wait_ms:.1f}ms"
        )
        yield

    def configure_agent(
        self,
        agent_name: str,
        requests_per_second: float,
        burst_capacity: int,
        queue_size: int,
    ) -> None:
        """
        Set or update rate limit configuration for a specific agent.
        The new config takes effect on the next request.
        """
        self._overrides[agent_name] = RateLimitConfig(
            requests_per_second=requests_per_second,
            burst_capacity=burst_capacity,
            queue_size=queue_size,
        )
        # Invalidate existing bucket so it's recreated with new config
        if agent_name in self._buckets:
            del self._buckets[agent_name]
        logger.info(
            f"Rate limit configured for agent={agent_name}: "
            f"{requests_per_second} req/s, burst={burst_capacity}, queue={queue_size}"
        )

    def remove_agent_config(self, agent_name: str) -> bool:
        """Remove per-agent override, reverting to global defaults."""
        removed = agent_name in self._overrides
        self._overrides.pop(agent_name, None)
        self._buckets.pop(agent_name, None)
        return removed

    def get_stats(self) -> Dict[str, Any]:
        """Return rate limiter statistics for all agents."""
        return {
            "agents": {
                name: self._bucket_stats(bucket)
                for name, bucket in self._buckets.items()
            },
            "global_defaults": {
                "requests_per_second": settings.RATE_LIMIT_REQUESTS_PER_SECOND,
                "burst_capacity": settings.RATE_LIMIT_BURST_CAPACITY,
                "queue_size": settings.RATE_LIMIT_QUEUE_SIZE,
                "queue_timeout_seconds": settings.RATE_LIMIT_QUEUE_TIMEOUT,
            },
        }

    def get_agent_stats(self, agent_name: str) -> Optional[Dict[str, Any]]:
        """Return stats for a single agent, or None if not seen yet."""
        bucket = self._buckets.get(agent_name)
        if bucket is None:
            return None
        return self._bucket_stats(bucket)

    def list_configured_agents(self) -> Dict[str, Any]:
        """List agents with custom rate limit configurations."""
        return {
            name: {
                "requests_per_second": cfg.requests_per_second,
                "burst_capacity": cfg.burst_capacity,
                "queue_size": cfg.queue_size,
            }
            for name, cfg in self._overrides.items()
        }

    # ------------------------------------------------------------------
    # Internal helpers
    # ------------------------------------------------------------------

    async def _get_or_create_bucket(self, agent_name: str) -> AgentBucket:
        """Get existing bucket or create a new one with appropriate config."""
        if agent_name in self._buckets:
            return self._buckets[agent_name]

        async with self._lock:
            # Double-check after acquiring lock
            if agent_name in self._buckets:
                return self._buckets[agent_name]

            cfg = self._overrides.get(agent_name)
            if cfg:
                rps = cfg.requests_per_second
                burst = cfg.burst_capacity
                qsize = cfg.queue_size
            else:
                rps = settings.RATE_LIMIT_REQUESTS_PER_SECOND
                burst = settings.RATE_LIMIT_BURST_CAPACITY
                qsize = settings.RATE_LIMIT_QUEUE_SIZE

            bucket = AgentBucket(
                agent_name=agent_name,
                capacity=float(burst),
                rate=rps,
                tokens=float(burst),  # Start full
                last_refill=time.monotonic(),
                queue=asyncio.Queue(maxsize=qsize),
            )
            self._buckets[agent_name] = bucket
            logger.info(
                f"Rate limiter: created bucket for agent={agent_name} "
                f"({rps} req/s, burst={burst}, queue={qsize})"
            )
            return bucket

    async def _drain_queue(self, agent_name: str) -> None:
        """
        Background coroutine that drains the queue for an agent.
        Runs until the queue is empty.
        """
        bucket = self._buckets.get(agent_name)
        if bucket is None:
            return

        while not bucket.queue.empty():
            # Wait until a token is available
            while True:
                async with self._lock:
                    if bucket.try_consume():
                        break
                # Sleep for the time needed to accumulate one token
                wait = max(0.01, 1.0 / bucket.rate)
                await asyncio.sleep(wait)

            # Grant the next queued request
            try:
                fut: asyncio.Future = bucket.queue.get_nowait()
                if not fut.done():
                    fut.set_result(None)
            except asyncio.QueueEmpty:
                break

    @staticmethod
    def _bucket_stats(bucket: AgentBucket) -> Dict[str, Any]:
        total = bucket.total_requests
        rejection_rate = (
            round(bucket.rejected_requests / total * 100, 2) if total > 0 else 0.0
        )
        return {
            "capacity": bucket.capacity,
            "rate_per_second": bucket.rate,
            "tokens_available": round(bucket.tokens, 3),
            "queue_depth": bucket.queue_depth,
            "total_requests": total,
            "accepted_requests": bucket.accepted_requests,
            "queued_requests": bucket.queued_requests,
            "rejected_requests": bucket.rejected_requests,
            "rejection_rate_pct": rejection_rate,
            "avg_queue_wait_ms": bucket.avg_wait_ms,
        }

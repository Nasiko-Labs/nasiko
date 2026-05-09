"""
RAL Adaptive Rate Limiter
=========================
Token-bucket rate limiter implemented in pure asyncio — no Redis round-trips
on the hot path.  All agents share the same global defaults.

Token bucket algorithm
----------------------
* Bucket capacity = RAL_RATE_LIMIT_BURST tokens (max burst).
* Tokens refill at RAL_RATE_LIMIT_RPS tokens/second.
* Each request consumes 1 token.
* Concurrent active requests are also capped at RAL_MAX_CONCURRENT per agent.
* When the bucket is empty or the concurrency cap is reached, the request
  is queued (not immediately rejected) and retried when capacity is available.

Design notes
------------
* In-process state is sufficient because all Kong-proxied requests for one
  router instance land on the same asyncio event loop.
* A Redis-backed distributed variant (via INCR + EXPIRE) can be added later
  for multi-replica deployments; the interface remains identical.
"""

from __future__ import annotations

import asyncio
import logging
import time
from dataclasses import dataclass, field
from typing import Dict

from .config import ral_settings

logger = logging.getLogger(__name__)


@dataclass
class _BucketState:
    tokens: float
    last_refill: float
    active: int = 0
    throttle_count: int = 0

    @classmethod
    def new(cls, capacity: float) -> "_BucketState":
        return cls(tokens=capacity, last_refill=time.monotonic())


class AdaptiveRateLimiter:
    """
    Per-agent token-bucket rate limiter with concurrency cap.

    All agents share the same configured RPS / burst / concurrency limits.
    Buckets are created lazily on first use and live as long as the
    `RouterOrchestrator` instance.
    """

    def __init__(self) -> None:
        self._rps: float      = ral_settings.RAL_RATE_LIMIT_RPS
        self._burst: int      = ral_settings.RAL_RATE_LIMIT_BURST
        self._max_conc: int   = ral_settings.RAL_MAX_CONCURRENT
        self._buckets: Dict[str, _BucketState] = {}
        self._lock = asyncio.Lock()

    # ------------------------------------------------------------------
    # Core API
    # ------------------------------------------------------------------

    async def acquire(self, agent_id: str, timeout: float = 10.0) -> bool:
        """
        Acquire a token for *agent_id*.

        Blocks up to *timeout* seconds waiting for a token to become
        available.  Returns True on success, False on timeout.
        """
        deadline = time.monotonic() + timeout
        while True:
            async with self._lock:
                bucket = self._get_or_create(agent_id)
                self._refill(bucket)

                if bucket.tokens >= 1.0 and bucket.active < self._max_conc:
                    bucket.tokens -= 1.0
                    bucket.active += 1
                    logger.debug(
                        "RAL limiter: ACQUIRED agent=%s tokens=%.1f active=%d",
                        agent_id, bucket.tokens, bucket.active,
                    )
                    return True
                else:
                    bucket.throttle_count += 1
                    logger.debug(
                        "RAL limiter: WAITING  agent=%s tokens=%.1f active=%d",
                        agent_id, bucket.tokens, bucket.active,
                    )

            remaining = deadline - time.monotonic()
            if remaining <= 0:
                logger.warning(
                    "RAL limiter: TIMEOUT  agent=%s after %.1fs", agent_id, timeout
                )
                return False

            # Sleep for the time needed to accumulate one token, capped by
            # remaining deadline.
            sleep_for = min(1.0 / self._rps, remaining)
            await asyncio.sleep(sleep_for)

    def release(self, agent_id: str) -> None:
        """Release the concurrency slot held by one completed request."""
        bucket = self._buckets.get(agent_id)
        if bucket and bucket.active > 0:
            bucket.active -= 1
            logger.debug(
                "RAL limiter: RELEASED agent=%s active=%d", agent_id, bucket.active
            )

    # ------------------------------------------------------------------
    # Stats
    # ------------------------------------------------------------------

    def get_stats(self) -> Dict[str, Dict]:
        """Return a snapshot of bucket states for all known agents."""
        snapshot = {}
        now = time.monotonic()
        for agent_id, bucket in self._buckets.items():
            refill_elapsed = now - bucket.last_refill
            projected_tokens = min(
                self._burst,
                bucket.tokens + refill_elapsed * self._rps,
            )
            snapshot[agent_id] = {
                "tokens_available": round(projected_tokens, 2),
                "burst_capacity": self._burst,
                "active_requests": bucket.active,
                "max_concurrent": self._max_conc,
                "throttle_count": bucket.throttle_count,
                "utilisation_pct": round(bucket.active / self._max_conc * 100, 1),
            }
        return snapshot

    # ------------------------------------------------------------------
    # Internal helpers
    # ------------------------------------------------------------------

    def _get_or_create(self, agent_id: str) -> _BucketState:
        if agent_id not in self._buckets:
            self._buckets[agent_id] = _BucketState.new(self._burst)
        return self._buckets[agent_id]

    def _refill(self, bucket: _BucketState) -> None:
        now = time.monotonic()
        elapsed = now - bucket.last_refill
        new_tokens = elapsed * self._rps
        bucket.tokens = min(self._burst, bucket.tokens + new_tokens)
        bucket.last_refill = now

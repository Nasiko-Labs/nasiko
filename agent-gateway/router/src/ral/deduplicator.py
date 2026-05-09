"""
RAL Request Deduplicator
========================
Prevents duplicate expensive LLM calls when identical requests arrive
concurrently (e.g. double-click, retry storms, fan-out test suites).

Algorithm
---------
* In-process `asyncio.Event` map keyed by the same cache-key used by
  `ResponseCache`.
* First request: creates the Event and executes the pipeline.
* Subsequent identical requests: suspend on the Event, then read the
  result from the shared cache once the first request completes.
* Cleanup: Event is removed from the map as soon as the first request
  resolves (success or failure), avoiding memory leaks.

Why in-process rather than Redis pub/sub?
  - The router service is stateless — all concurrent requests for a
    given session land on the same pod. In-process Events have zero
    network round-trip overhead.
  - Redis is still the durable cache; the deduplicator is the fast
    "in-flight guard" layer on top of it.
"""

from __future__ import annotations

import asyncio
import logging
from contextlib import asynccontextmanager
from typing import Dict, Optional, Any, AsyncGenerator

logger = logging.getLogger(__name__)


class _InFlightEntry:
    """Shared state for a single in-flight request."""

    __slots__ = ("event", "result", "error", "waiters")

    def __init__(self) -> None:
        self.event: asyncio.Event = asyncio.Event()
        self.result: Optional[str] = None
        self.error: Optional[Exception] = None
        self.waiters: int = 0


class RequestDeduplicator:
    """
    In-process deduplicator: identical concurrent requests share one result.

    Usage
    -----
    ```python
    dedup = RequestDeduplicator()

    async with dedup.guard(cache_key) as (is_leader, get_shared_result):
        if is_leader:
            result = await compute(...)
            dedup.resolve(cache_key, result)
        else:
            result = await get_shared_result()
    ```
    """

    def __init__(self) -> None:
        self._in_flight: Dict[str, _InFlightEntry] = {}
        self._lock = asyncio.Lock()

    # ------------------------------------------------------------------
    # Public API
    # ------------------------------------------------------------------

    def is_in_flight(self, key: str) -> bool:
        """Return True if a request with *key* is currently being processed."""
        return key in self._in_flight

    @asynccontextmanager
    async def guard(
        self, key: str
    ) -> AsyncGenerator[tuple[bool, "asyncio.coroutine"], None]:
        """
        Async context manager that coordinates first-caller vs waiters.

        Yields
        ------
        (is_leader, wait_fn)
            is_leader : bool — True if this coroutine should compute the result.
            wait_fn   : coroutine — call this to wait for the result when not leader.
        """
        async with self._lock:
            if key in self._in_flight:
                entry = self._in_flight[key]
                entry.waiters += 1
                is_leader = False
                logger.debug("RAL dedup: waiter for key=%s (total=%d)", key[:16], entry.waiters)
            else:
                entry = _InFlightEntry()
                self._in_flight[key] = entry
                is_leader = True
                logger.debug("RAL dedup: leader for key=%s", key[:16])

        async def _wait() -> Optional[str]:
            await entry.event.wait()
            if entry.error is not None:
                raise entry.error
            return entry.result

        try:
            yield is_leader, _wait
        finally:
            if is_leader:
                # Clean up only when leader exits (waiters have already been served)
                async with self._lock:
                    self._in_flight.pop(key, None)

    def resolve(self, key: str, result: str) -> None:
        """
        Signal all waiters that the result is ready.

        Call this from the leader coroutine after the result has been
        stored in the cache.
        """
        entry = self._in_flight.get(key)
        if entry:
            entry.result = result
            entry.event.set()
            logger.debug(
                "RAL dedup: resolved key=%s (notified %d waiter(s))",
                key[:16],
                entry.waiters,
            )

    def reject(self, key: str, error: Exception) -> None:
        """
        Signal all waiters that the request failed.

        Waiters will re-raise the exception when they call `wait_fn()`.
        """
        entry = self._in_flight.get(key)
        if entry:
            entry.error = error
            entry.event.set()
            logger.debug(
                "RAL dedup: rejected key=%s error=%s (notified %d waiter(s))",
                key[:16],
                type(error).__name__,
                entry.waiters,
            )

    @property
    def in_flight_count(self) -> int:
        """Number of unique requests currently being deduplicated."""
        return len(self._in_flight)

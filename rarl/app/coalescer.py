import asyncio
from typing import Any, Awaitable, Callable


class SingleFlight:
    """Collapses concurrent identical requests into a single upstream call."""

    def __init__(self) -> None:
        self._inflight: dict[str, asyncio.Future] = {}
        self._lock = asyncio.Lock()
        self.coalesced_count = 0

    async def do(
        self, key: str, fn: Callable[[], Awaitable[Any]]
    ) -> tuple[Any, bool]:
        """Returns (result, was_leader). was_leader=False means this call was coalesced."""
        async with self._lock:
            existing = self._inflight.get(key)
            if existing is not None:
                self.coalesced_count += 1
                fut = existing
                leader = False
            else:
                fut = asyncio.get_running_loop().create_future()
                self._inflight[key] = fut
                leader = True

        if not leader:
            return await fut, False

        try:
            result = await fn()
            if not fut.done():
                fut.set_result(result)
            return result, True
        except Exception as e:
            if not fut.done():
                fut.set_exception(e)
            raise
        finally:
            async with self._lock:
                self._inflight.pop(key, None)

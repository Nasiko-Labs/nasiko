"""Pytest fixtures for the Request layer test suite.

Tests run against a tiny in-memory async Redis fake so they can execute
without a running stack. The fake covers the subset of Redis operations
the unit tests exercise; integration tests are kept separate.
"""

import asyncio
from typing import Any

import pytest


class FakeRedis:
    """Very small async Redis fake.

    Only covers the subset of operations that the unit tests exercise:
    GET, SET (with EX), DELETE, INCRBYFLOAT, EXPIRE, HGET, HSET, HDEL,
    LPUSH, RPOPLPUSH, LREM, LLEN, SCAN_ITER, PUBSUB.
    """

    def __init__(self) -> None:
        self.store: dict[str, Any] = {}
        self.expirations: dict[str, float] = {}
        self.lists: dict[str, list[bytes]] = {}
        self.hashes: dict[str, dict[str, Any]] = {}
        self.subscribers: dict[str, list[asyncio.Queue]] = {}

    # ----- key/value ----------------------------------------------------

    async def get(self, key: str) -> Any:
        if key not in self.store:
            return None
        return self.store[key]

    async def set(
        self,
        key: str,
        value: Any,
        *,
        ex: int | None = None,
        nx: bool = False,
    ) -> bool:
        if nx and key in self.store:
            return False
        self.store[key] = value if isinstance(value, bytes) else str(value).encode()
        return True

    async def delete(self, *keys: str) -> int:
        deleted = 0
        for key in keys:
            if key in self.store:
                del self.store[key]
                deleted += 1
        return deleted

    async def incrbyfloat(self, key: str, amount: float) -> float:
        current = float(self.store.get(key, 0) or 0)
        current += amount
        self.store[key] = str(current).encode()
        return current

    async def expire(self, key: str, seconds: int) -> int:
        return 1 if key in self.store else 0

    async def ping(self) -> bool:
        return True

    async def aclose(self) -> None:  # pragma: no cover
        pass

    # ----- hashes -------------------------------------------------------

    async def hget(self, key: str, field: str) -> Any:
        return self.hashes.get(key, {}).get(field)

    async def hset(self, key: str, field: str | None = None, value: Any = None, *, mapping: dict | None = None) -> int:
        if mapping:
            self.hashes.setdefault(key, {}).update(mapping)
            return len(mapping)
        self.hashes.setdefault(key, {})[field] = value
        return 1

    async def hdel(self, key: str, field: str) -> int:
        return 1 if self.hashes.get(key, {}).pop(field, None) is not None else 0

    # ----- lists --------------------------------------------------------

    async def lpush(self, key: str, *values: Any) -> int:
        for value in values:
            self.lists.setdefault(key, []).insert(0, value if isinstance(value, bytes) else str(value).encode())
        return len(self.lists[key])

    async def rpoplpush(self, src: str, dest: str) -> bytes | None:
        if not self.lists.get(src):
            return None
        item = self.lists[src].pop()
        self.lists.setdefault(dest, []).insert(0, item)
        return item

    async def lrem(self, key: str, count: int, value: Any) -> int:
        target = value if isinstance(value, bytes) else str(value).encode()
        if key not in self.lists:
            return 0
        before = len(self.lists[key])
        self.lists[key] = [x for x in self.lists[key] if x != target]
        return before - len(self.lists[key])

    async def llen(self, key: str) -> int:
        return len(self.lists.get(key, []))

    # ----- pipeline (no-op transactional shim) -------------------------

    def pipeline(self) -> "FakePipeline":
        return FakePipeline(self)

    # ----- scan ---------------------------------------------------------

    async def scan_iter(self, match: str = "*"):
        import fnmatch

        for key in list(self.store.keys()):
            if fnmatch.fnmatchcase(key, match):
                yield key

    # ----- pubsub -------------------------------------------------------

    async def publish(self, channel: str, message: Any) -> int:
        subs = self.subscribers.get(channel, [])
        payload = message if isinstance(message, bytes) else str(message).encode()
        for queue in list(subs):
            queue.put_nowait(payload)
        return len(subs)

    def pubsub(self) -> "FakePubSub":
        return FakePubSub(self)


class FakePipeline:
    def __init__(self, parent: FakeRedis) -> None:
        self._parent = parent
        self._ops: list[tuple] = []

    def hset(self, key: str, *, mapping: dict) -> None:
        self._ops.append(("hset", key, mapping))

    def expire(self, key: str, seconds: int) -> None:
        self._ops.append(("expire", key, seconds))

    def incrbyfloat(self, key: str, amount: float) -> None:
        self._ops.append(("incrbyfloat", key, amount))

    def lpush(self, key: str, *values: Any) -> None:
        self._ops.append(("lpush", key, values))

    def ltrim(self, key: str, start: int, stop: int) -> None:  # pragma: no cover
        self._ops.append(("ltrim", key, start, stop))

    async def execute(self) -> list:
        results = []
        for op in self._ops:
            if op[0] == "hset":
                results.append(await self._parent.hset(op[1], mapping=op[2]))
            elif op[0] == "expire":
                results.append(await self._parent.expire(op[1], op[2]))
            elif op[0] == "incrbyfloat":
                results.append(await self._parent.incrbyfloat(op[1], op[2]))
            elif op[0] == "lpush":
                results.append(await self._parent.lpush(op[1], *op[2]))
        return results


class FakePubSub:
    def __init__(self, parent: FakeRedis) -> None:
        self._parent = parent
        self._channel: str | None = None
        self._queue: asyncio.Queue[bytes] = asyncio.Queue()

    async def subscribe(self, channel: str) -> None:
        self._channel = channel
        self._parent.subscribers.setdefault(channel, []).append(self._queue)

    async def unsubscribe(self, channel: str) -> None:
        subs = self._parent.subscribers.get(channel, [])
        if self._queue in subs:
            subs.remove(self._queue)

    async def get_message(
        self,
        ignore_subscribe_messages: bool = True,
        timeout: float | None = None,
    ) -> dict | None:
        try:
            payload = await asyncio.wait_for(self._queue.get(), timeout=timeout or 0.1)
        except asyncio.TimeoutError:
            return None
        return {"data": payload, "channel": self._channel}

    async def aclose(self) -> None:  # pragma: no cover
        pass


@pytest.fixture
def fake_redis() -> FakeRedis:
    return FakeRedis()


@pytest.fixture(autouse=True)
def _ensure_module_path(monkeypatch):
    """Make ``import request_layer.src...`` work when pytest is invoked from repo root."""

    import sys
    from pathlib import Path

    repo_root = Path(__file__).resolve().parents[2]
    if str(repo_root) not in sys.path:
        sys.path.insert(0, str(repo_root))
    yield

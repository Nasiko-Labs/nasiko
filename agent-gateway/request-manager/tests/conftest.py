from __future__ import annotations

import asyncio
from collections import defaultdict, deque
from typing import Any

import pytest


class FakeRedis:
    def __init__(self) -> None:
        self.values: dict[str, Any] = {}
        self.hashes: dict[str, dict[str, str]] = defaultdict(dict)
        self.sets: dict[str, set[str]] = defaultdict(set)
        self.lists: dict[str, deque[str]] = defaultdict(deque)
        self.expiry: dict[str, int] = {}

    async def ping(self) -> bool:
        return True

    async def get(self, key: str) -> Any:
        return self.values.get(key)

    async def set(self, key: str, value: Any, ex: int | None = None, px: int | None = None, nx: bool = False) -> bool:
        if nx and key in self.values:
            return False
        self.values[key] = value
        if ex is not None:
            self.expiry[key] = ex * 1000
        if px is not None:
            self.expiry[key] = px
        return True

    async def delete(self, *keys: str) -> int:
        removed = 0
        for key in keys:
            if key in self.values:
                removed += 1
                self.values.pop(key, None)
            self.hashes.pop(key, None)
            self.sets.pop(key, None)
            self.lists.pop(key, None)
        return removed

    async def hset(self, key: str, mapping: dict[str, Any]) -> int:
        for field, value in mapping.items():
            self.hashes[key][field] = str(value)
        return len(mapping)

    async def hgetall(self, key: str) -> dict[str, str]:
        return dict(self.hashes.get(key, {}))

    async def hincrby(self, key: str, field: str, amount: int = 1) -> int:
        current = int(self.hashes[key].get(field, "0"))
        updated = current + amount
        self.hashes[key][field] = str(updated)
        return updated

    async def sadd(self, key: str, *values: str) -> int:
        before = len(self.sets[key])
        self.sets[key].update(values)
        return len(self.sets[key]) - before

    async def smembers(self, key: str) -> set[str]:
        return set(self.sets.get(key, set()))

    async def srem(self, key: str, *values: str) -> int:
        removed = 0
        for value in values:
            if value in self.sets[key]:
                self.sets[key].remove(value)
                removed += 1
        return removed

    async def incr(self, key: str) -> int:
        current = int(self.values.get(key, "0")) + 1
        self.values[key] = str(current)
        return current

    async def decr(self, key: str) -> int:
        current = int(self.values.get(key, "0")) - 1
        self.values[key] = str(max(current, 0))
        return int(self.values[key])

    async def rpush(self, key: str, value: str) -> int:
        self.lists[key].append(value)
        return len(self.lists[key])

    async def lpop(self, key: str) -> str | None:
        if not self.lists[key]:
            return None
        return self.lists[key].popleft()

    async def lindex(self, key: str, index: int) -> str | None:
        try:
            return list(self.lists[key])[index]
        except IndexError:
            return None

    async def llen(self, key: str) -> int:
        return len(self.lists[key])

    async def lrem(self, key: str, count: int, value: str) -> int:
        original = list(self.lists[key])
        kept = deque(item for item in original if item != value)
        removed = len(original) - len(kept)
        self.lists[key] = kept
        return removed

    async def expire(self, key: str, seconds: int) -> bool:
        self.expiry[key] = seconds * 1000
        return True


@pytest.fixture
def fake_redis() -> FakeRedis:
    return FakeRedis()


@pytest.fixture
def event_loop():
    loop = asyncio.new_event_loop()
    yield loop
    loop.close()

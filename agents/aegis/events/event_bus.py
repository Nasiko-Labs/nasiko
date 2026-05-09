import asyncio
from collections import defaultdict
from typing import Any, Callable, Awaitable


class EventBus:
    def __init__(self):
        self._listeners: dict[str, list[Callable]] = defaultdict(list)

    def subscribe(self, event: str, handler: Callable[[Any], Awaitable[None]]) -> None:
        self._listeners[event].append(handler)

    async def publish(self, event: str, data: Any) -> None:
        for handler in self._listeners[event]:
            asyncio.create_task(handler(data))


bus = EventBus()

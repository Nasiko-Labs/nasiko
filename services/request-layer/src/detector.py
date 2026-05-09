import asyncio
import json
import logging
import time

import redis.asyncio as aioredis

from src.config import settings
from src.queue import get_queue_depth, get_slot_status

logger = logging.getLogger(__name__)

_OVERLOADED_THRESHOLD = 1.5
_IDLE_THRESHOLD = 0.1


def _pressure(slots_used: int, max_slots: int, queue_depth: int) -> float:
    utilisation = slots_used / max_slots if max_slots > 0 else 0.0
    return utilisation + queue_depth * 0.5


def _classify(score: float) -> str:
    if score > _OVERLOADED_THRESHOLD:
        return "overloaded"
    if score < _IDLE_THRESHOLD:
        return "idle"
    return "healthy"


async def _build_agent_state(redis: aioredis.Redis, agent_name: str) -> dict:
    status = await get_slot_status(redis, agent_name)
    depth = await get_queue_depth(redis, agent_name)
    hibernated = await redis.exists(f"fleet:hibernated:{agent_name}")
    score = _pressure(status["used"], status["max"], depth)
    return {
        "agent": agent_name,
        "slots_used": status["used"],
        "slots_max": status["max"],
        "slots_available": status["available"],
        "queue_depth": depth,
        "pressure": round(score, 3),
        "status": "hibernated" if hibernated else _classify(score),
        "timestamp": int(time.time()),
    }


async def _known_agents(redis: aioredis.Redis) -> list[str]:
    agents = set()
    async for key in redis.scan_iter(match="config:*:max_slots"):
        parts = key.decode().split(":")
        if len(parts) == 3:
            agents.add(parts[1])
    return list(agents)


async def _hibernation_safe(redis: aioredis.Redis, agent_name: str) -> bool:
    status = await get_slot_status(redis, agent_name)
    depth = await get_queue_depth(redis, agent_name)
    return status["used"] == 0 and depth == 0


async def _generate_recommendation(fleet: list[dict]) -> dict | None:
    overloaded = [a for a in fleet if a["status"] == "overloaded"]
    idle = [a for a in fleet if a["status"] == "idle"]
    if not overloaded or not idle:
        return None
    target = max(overloaded, key=lambda a: a["pressure"])
    candidate = idle[0]
    return {
        "action": "reallocation",
        "hibernate": candidate["agent"],
        "spawn_replica_for": target["agent"],
        "reason": f"{target['agent']} pressure={target['pressure']} queue={target['queue_depth']}",
        "timestamp": int(time.time()),
    }


class DetectorLoop:
    def __init__(self, redis: aioredis.Redis) -> None:
        self._redis = redis
        self._task: asyncio.Task | None = None

    def start(self) -> None:
        self._task = asyncio.create_task(self._loop(), name="detector-loop")

    def stop(self) -> None:
        if self._task:
            self._task.cancel()

    async def _loop(self) -> None:
        logger.info("detector loop started")
        while True:
            try:
                await self._tick()
            except asyncio.CancelledError:
                break
            except Exception as exc:
                logger.error(f"detector error: {exc}")
            await asyncio.sleep(settings.DETECTOR_INTERVAL_SECONDS)

    async def _tick(self) -> None:
        agents = await _known_agents(self._redis)
        if not agents:
            return

        fleet = [await _build_agent_state(self._redis, a) for a in agents]
        await self._redis.set("fleet:state", json.dumps(fleet), ex=60)

        recommendation = await _generate_recommendation(fleet)
        if recommendation:
            candidate = recommendation["hibernate"]
            if await _hibernation_safe(self._redis, candidate):
                await self._redis.set("fleet:recommendation", json.dumps(recommendation), ex=60)
            else:
                await self._redis.delete("fleet:recommendation")
        else:
            await self._redis.delete("fleet:recommendation")

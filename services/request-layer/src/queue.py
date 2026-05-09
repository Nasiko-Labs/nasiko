import asyncio
import json
import logging
import time
import uuid
from typing import Any, Callable, Awaitable

import redis.asyncio as aioredis

logger = logging.getLogger(__name__)

_DEFAULT_MAX_SLOTS = 5
_DRAIN_POLL_INTERVAL = 0.1


async def init_slots(redis: aioredis.Redis, agent_name: str, max_slots: int = _DEFAULT_MAX_SLOTS) -> None:
    slot_key = f"slots:{agent_name}"
    config_key = f"config:{agent_name}:max_slots"
    existing = await redis.get(config_key)
    if existing is None:
        await redis.set(config_key, max_slots)
        await redis.set(slot_key, max_slots)
        logger.info(f"slots init: {agent_name} max={max_slots}")


async def claim_slot(redis: aioredis.Redis, agent_name: str) -> bool:
    slot_key = f"slots:{agent_name}"
    new_val = await redis.decr(slot_key)
    if new_val < 0:
        await redis.incr(slot_key)
        return False
    return True


async def release_slot(redis: aioredis.Redis, agent_name: str) -> None:
    await redis.incr(f"slots:{agent_name}")


async def get_slot_status(redis: aioredis.Redis, agent_name: str) -> dict[str, int]:
    config_key = f"config:{agent_name}:max_slots"
    slot_key = f"slots:{agent_name}"
    max_slots = int(await redis.get(config_key) or _DEFAULT_MAX_SLOTS)
    available = int(await redis.get(slot_key) or max_slots)
    available = max(0, available)
    return {"max": max_slots, "available": available, "used": max_slots - available}


async def enqueue_request(
    redis: aioredis.Redis,
    agent_name: str,
    job_id: str,
    method: str,
    url: str,
    headers: dict,
    body: bytes,
) -> None:
    payload = json.dumps({
        "job_id": job_id,
        "method": method,
        "url": url,
        "headers": headers,
        "body": body.decode(errors="replace"),
        "enqueued_at": time.time(),
    })
    await redis.rpush(f"queue:{agent_name}", payload)


async def get_queue_depth(redis: aioredis.Redis, agent_name: str) -> int:
    return int(await redis.llen(f"queue:{agent_name}"))


async def store_job_result(
    redis: aioredis.Redis,
    job_id: str,
    status_code: int,
    body: bytes,
    headers: dict,
) -> None:
    payload = json.dumps({
        "status": "done",
        "status_code": status_code,
        "body": body.decode(errors="replace"),
        "headers": headers,
    })
    await redis.set(f"job:{job_id}:result", payload, ex=300)


async def get_job_result(redis: aioredis.Redis, job_id: str) -> dict | None:
    raw = await redis.get(f"job:{job_id}:result")
    if raw is None:
        return None
    return json.loads(raw)


async def estimate_wait_seconds(redis: aioredis.Redis, agent_name: str) -> float:
    depth = await get_queue_depth(redis, agent_name)
    avg_key = f"config:{agent_name}:avg_processing_ms"
    avg_ms = float(await redis.get(avg_key) or 500)
    status = await get_slot_status(redis, agent_name)
    workers = max(1, status["max"])
    return round((depth * avg_ms / 1000) / workers, 1)


def generate_job_id() -> str:
    return str(uuid.uuid4())


class DrainWorker:
    def __init__(
        self,
        redis: aioredis.Redis,
        agent_name: str,
        forward_fn: Callable[..., Awaitable[tuple[int, bytes, dict]]],
    ) -> None:
        self._redis = redis
        self._agent_name = agent_name
        self._forward = forward_fn
        self._task: asyncio.Task | None = None

    def start(self) -> None:
        self._task = asyncio.create_task(self._loop(), name=f"drain-{self._agent_name}")

    def stop(self) -> None:
        if self._task:
            self._task.cancel()

    async def _loop(self) -> None:
        logger.info(f"drain worker started for {self._agent_name}")
        while True:
            try:
                await self._tick()
            except asyncio.CancelledError:
                break
            except Exception as exc:
                logger.error(f"drain worker error ({self._agent_name}): {exc}")
            await asyncio.sleep(_DRAIN_POLL_INTERVAL)

    async def _tick(self) -> None:
        claimed = await claim_slot(self._redis, self._agent_name)
        if not claimed:
            return

        raw = await self._redis.lpop(f"queue:{self._agent_name}")
        if raw is None:
            await release_slot(self._redis, self._agent_name)
            return

        item = json.loads(raw)
        job_id = item["job_id"]
        start = time.monotonic()

        try:
            status_code, body, headers = await self._forward(
                method=item["method"],
                url=item["url"],
                headers=item["headers"],
                body=item["body"].encode(),
            )
            elapsed_ms = (time.monotonic() - start) * 1000
            await self._redis.set(
                f"config:{self._agent_name}:avg_processing_ms",
                int(elapsed_ms),
                ex=3600,
            )
            await store_job_result(self._redis, job_id, status_code, body, headers)
        except Exception as exc:
            logger.error(f"drain forward error job={job_id}: {exc}")
            await store_job_result(self._redis, job_id, 502, b'{"detail":"upstream error"}', {})
        finally:
            await release_slot(self._redis, self._agent_name)

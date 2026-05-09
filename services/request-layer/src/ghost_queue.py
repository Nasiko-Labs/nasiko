import asyncio
import json
import logging
import time

import redis.asyncio as aioredis

from src.queue import store_job_result, generate_job_id

logger = logging.getLogger(__name__)

_GHOST_POLL_INTERVAL = 1.0


async def enqueue_ghost(
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
    await redis.rpush(f"ghost_queue:{agent_name}", payload)
    logger.info(f"ghost queued job={job_id} for {agent_name}")


class GhostQueueDrainWorker:
    def __init__(
        self,
        redis: aioredis.Redis,
        agent_name: str,
        forward_fn,
    ) -> None:
        self._redis = redis
        self._agent_name = agent_name
        self._forward = forward_fn
        self._task: asyncio.Task | None = None

    def start(self) -> None:
        self._task = asyncio.create_task(
            self._loop(), name=f"ghost-drain-{self._agent_name}"
        )

    def stop(self) -> None:
        if self._task:
            self._task.cancel()

    async def _loop(self) -> None:
        logger.info(f"ghost drain worker started for {self._agent_name}")
        while True:
            try:
                hibernated = await self._redis.exists(f"fleet:hibernated:{self._agent_name}")
                if not hibernated:
                    await self._drain_all()
            except asyncio.CancelledError:
                break
            except Exception as exc:
                logger.error(f"ghost drain error ({self._agent_name}): {exc}")
            await asyncio.sleep(_GHOST_POLL_INTERVAL)

    async def _drain_all(self) -> None:
        while True:
            raw = await self._redis.lpop(f"ghost_queue:{self._agent_name}")
            if raw is None:
                break
            item = json.loads(raw)
            job_id = item["job_id"]
            try:
                status_code, body, headers = await self._forward(
                    method=item["method"],
                    url=item["url"],
                    headers=item["headers"],
                    body=item["body"].encode(),
                )
                await store_job_result(self._redis, job_id, status_code, body, headers)
            except Exception as exc:
                logger.error(f"ghost drain forward error job={job_id}: {exc}")
                await store_job_result(
                    self._redis, job_id, 502, b'{"detail":"upstream error"}', {}
                )

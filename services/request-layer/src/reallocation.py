import asyncio
import json
import logging
import time

import redis.asyncio as aioredis

from src import docker_client
from src import kong_client
from src.config import settings

logger = logging.getLogger(__name__)


def _replica_name(agent_name: str) -> str:
    return f"{agent_name}-replica-1"


def _container_name(agent_name: str) -> str:
    return agent_name


async def hibernate_agent(redis: aioredis.Redis, agent_name: str) -> None:
    from src.queue import get_slot_status, get_queue_depth

    status = await get_slot_status(redis, agent_name)
    depth = await get_queue_depth(redis, agent_name)
    if status["used"] > 0 or depth > 0:
        raise ValueError(f"cannot hibernate {agent_name}: slots_used={status['used']} queue={depth}")

    await asyncio.to_thread(docker_client.pause_container, _container_name(agent_name))
    await redis.set(f"fleet:hibernated:{agent_name}", "1")
    await _log_event(redis, "hibernate", agent_name, before_depth=depth, after_depth=depth)
    logger.info(f"hibernated agent: {agent_name}")


async def spawn_replica(redis: aioredis.Redis, agent_name: str) -> str:
    replica = _replica_name(agent_name)
    container_id = await asyncio.to_thread(
        docker_client.run_replica,
        _container_name(agent_name),
        replica,
        settings.AGENTS_NETWORK,
    )
    healthy = await asyncio.to_thread(docker_client.wait_for_health, replica, 60)
    if not healthy:
        logger.warning(f"replica {replica} did not become healthy within 60s — registering anyway")

    kong_client.add_upstream_target(agent_name, f"{replica}:5000")

    from src.queue import get_queue_depth
    depth = await get_queue_depth(redis, agent_name)
    await _log_event(redis, "spawn_replica", agent_name, before_depth=depth, after_depth=depth)
    logger.info(f"replica spawned and registered: {replica}")
    return container_id


async def restore_agent(redis: aioredis.Redis, agent_name: str) -> None:
    from src.queue import get_queue_depth
    before_depth = await get_queue_depth(redis, agent_name)

    replica = _replica_name(agent_name)
    kong_client.remove_upstream_target(agent_name, f"{replica}:5000")
    await asyncio.to_thread(docker_client.stop_container, replica)

    await asyncio.to_thread(docker_client.unpause_container, _container_name(agent_name))
    await redis.delete(f"fleet:hibernated:{agent_name}")

    after_depth = await get_queue_depth(redis, agent_name)
    await _log_event(redis, "restore", agent_name, before_depth=before_depth, after_depth=after_depth)
    logger.info(f"restored agent: {agent_name}")


async def _log_event(
    redis: aioredis.Redis,
    action: str,
    agent_name: str,
    before_depth: int,
    after_depth: int,
) -> None:
    entry = json.dumps({
        "action": action,
        "agent": agent_name,
        "before_queue_depth": before_depth,
        "after_queue_depth": after_depth,
        "timestamp": int(time.time()),
    })
    await redis.lpush("fleet:history", entry)
    await redis.ltrim("fleet:history", 0, 99)

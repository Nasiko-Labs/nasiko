import json
import logging

import redis.asyncio as aioredis
from fastapi import APIRouter, Depends, HTTPException, Request
from pydantic import BaseModel

from src.queue import get_queue_depth, get_slot_status, estimate_wait_seconds

logger = logging.getLogger(__name__)

router = APIRouter(prefix="/monitor", tags=["monitor"])


def _redis(request: Request) -> aioredis.Redis:
    return request.app.state.redis


@router.get("/cache/stats")
async def cache_stats(redis: aioredis.Redis = Depends(_redis)):
    from src import cache as cache_module
    return await cache_module.get_cache_stats(redis)


@router.get("/cache/{agent_name}")
async def cache_agent(agent_name: str, redis: aioredis.Redis = Depends(_redis)):
    from src import cache as cache_module
    return await cache_module.get_agent_cache_entries(redis, agent_name)


@router.delete("/cache/{agent_name}")
async def cache_purge_agent(agent_name: str, redis: aioredis.Redis = Depends(_redis)):
    from src import cache as cache_module
    deleted = await cache_module.purge_agent_cache(redis, agent_name)
    return {"deleted": deleted}


@router.delete("/cache")
async def cache_purge_all(redis: aioredis.Redis = Depends(_redis)):
    from src import cache as cache_module
    deleted = await cache_module.purge_all_cache(redis)
    return {"deleted": deleted}


@router.get("/queue/stats")
async def queue_stats(redis: aioredis.Redis = Depends(_redis)):
    agents = await _known_agents(redis)
    result = []
    for agent in agents:
        depth = await get_queue_depth(redis, agent)
        wait = await estimate_wait_seconds(redis, agent)
        result.append({"agent": agent, "queue_depth": depth, "estimated_wait_s": wait})
    return result


@router.get("/slots")
async def slots(redis: aioredis.Redis = Depends(_redis)):
    agents = await _known_agents(redis)
    return [{"agent": a, **(await get_slot_status(redis, a))} for a in agents]


class SlotsUpdate(BaseModel):
    max_slots: int


@router.put("/slots/{agent_name}")
async def update_slots(
    agent_name: str, body: SlotsUpdate, redis: aioredis.Redis = Depends(_redis)
):
    if body.max_slots < 1:
        raise HTTPException(status_code=400, detail="max_slots must be >= 1")
    old_max = int(await redis.get(f"config:{agent_name}:max_slots") or body.max_slots)
    old_available = int(await redis.get(f"slots:{agent_name}") or old_max)
    in_use = max(0, old_max - old_available)
    new_available = max(0, body.max_slots - in_use)
    await redis.set(f"config:{agent_name}:max_slots", body.max_slots)
    await redis.set(f"slots:{agent_name}", new_available)
    return {"agent": agent_name, "max_slots": body.max_slots, "available": new_available, "in_use": in_use}


@router.get("/fleet/state")
async def fleet_state(redis: aioredis.Redis = Depends(_redis)):
    raw = await redis.get("fleet:state")
    if raw is None:
        return []
    return json.loads(raw)


@router.get("/fleet/recommendation")
async def fleet_recommendation(redis: aioredis.Redis = Depends(_redis)):
    raw = await redis.get("fleet:recommendation")
    if raw is None:
        return None
    return json.loads(raw)


@router.delete("/fleet/recommendation")
async def fleet_reject_recommendation(redis: aioredis.Redis = Depends(_redis)):
    await redis.delete("fleet:recommendation")
    return {"status": "rejected"}


@router.post("/fleet/hibernate/{agent_name}")
async def fleet_hibernate(agent_name: str, redis: aioredis.Redis = Depends(_redis)):
    from src import reallocation
    try:
        await reallocation.hibernate_agent(redis, agent_name)
    except ValueError as exc:
        raise HTTPException(status_code=409, detail=str(exc))
    except RuntimeError as exc:
        raise HTTPException(status_code=500, detail=str(exc))
    return {"status": "hibernated", "agent": agent_name}


@router.post("/fleet/spawn/{agent_name}")
async def fleet_spawn(agent_name: str, redis: aioredis.Redis = Depends(_redis)):
    from src import reallocation
    try:
        container_id = await reallocation.spawn_replica(redis, agent_name)
    except (ValueError, RuntimeError) as exc:
        raise HTTPException(status_code=500, detail=str(exc))
    return {"status": "spawned", "agent": agent_name, "container_id": container_id}


@router.post("/fleet/restore/{agent_name}")
async def fleet_restore(agent_name: str, redis: aioredis.Redis = Depends(_redis)):
    from src import reallocation
    try:
        await reallocation.restore_agent(redis, agent_name)
    except (ValueError, RuntimeError) as exc:
        raise HTTPException(status_code=500, detail=str(exc))
    return {"status": "restored", "agent": agent_name}


@router.get("/fleet/history")
async def fleet_history(redis: aioredis.Redis = Depends(_redis)):
    raw_list = await redis.lrange("fleet:history", 0, 49)
    return [json.loads(r) for r in raw_list]


async def _known_agents(redis: aioredis.Redis) -> list[str]:
    agents = []
    async for key in redis.scan_iter(match="config:*:max_slots"):
        parts = key.decode().split(":")
        if len(parts) == 3:
            agents.append(parts[1])
    return agents

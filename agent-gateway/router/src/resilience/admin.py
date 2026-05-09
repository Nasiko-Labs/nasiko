from __future__ import annotations

from fastapi import APIRouter, Depends, Header, HTTPException
from pydantic import BaseModel, Field

from router.src.config import settings
from router.src.resilience.executor import ResilientAgentExecutor


class CacheClearRequest(BaseModel):
    agent_id: str | None = None


class CacheConfigRequest(BaseModel):
    ttl_seconds: float | None = Field(default=None, ge=0)
    enabled: bool | None = None
    semantic_enabled: bool | None = None
    semantic_threshold: float | None = Field(default=None, ge=0, le=1)


class LimitConfigRequest(BaseModel):
    base_rps: float | None = Field(default=None, gt=0)
    min_rps: float | None = Field(default=None, gt=0)
    burst: int | None = Field(default=None, ge=1)
    max_queue_depth: int | None = Field(default=None, ge=0)
    max_queue_wait_seconds: float | None = Field(default=None, ge=0)
    target_latency_seconds: float | None = Field(default=None, gt=0)


def build_admin_router(executor: ResilientAgentExecutor) -> APIRouter:
    router = APIRouter(prefix="/admin", tags=["resilience-admin"])

    async def require_admin_key(
        x_admin_api_key: str | None = Header(default=None),
    ) -> None:
        if not settings.RESILIENCE_ADMIN_API_KEY:
            return
        if x_admin_api_key != settings.RESILIENCE_ADMIN_API_KEY:
            raise HTTPException(status_code=401, detail="Invalid admin API key")

    @router.get("/stats/runtime", dependencies=[Depends(require_admin_key)])
    async def runtime_stats():
        return executor.runtime_snapshot().model_dump()

    @router.post("/cache/clear", dependencies=[Depends(require_admin_key)])
    async def clear_cache(payload: CacheClearRequest | None = None):
        agent_id = payload.agent_id if payload else None
        deleted = executor.cache.clear(agent_id=agent_id)
        return {"deleted": deleted}

    @router.put("/cache/config", dependencies=[Depends(require_admin_key)])
    async def update_cache_config(payload: CacheConfigRequest):
        config = executor.cache.update_config(**payload.model_dump())
        return config.model_dump()

    @router.put("/limits/{agent_id}", dependencies=[Depends(require_admin_key)])
    async def update_limit(agent_id: str, payload: LimitConfigRequest):
        config = executor.limiter.update_config(agent_id, **payload.model_dump())
        return config.model_dump()

    return router

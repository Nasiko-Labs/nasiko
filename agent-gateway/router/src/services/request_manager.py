"""
Unified Request Management Layer.

This service sits between the FastAPI endpoint and the RouterOrchestrator.
It enforces:
  1. Per-agent rate limiting (token bucket + async queue)
  2. Response caching (Redis-backed, LRU fallback) — injected into the
     orchestrator so the cache check happens at the correct point: AFTER
     the LLM selects the agent but BEFORE the request is forwarded to it.

Request flow
------------
  POST /router
      │
      ▼
  RequestManager.handle_request()
      │
      ├─ [1] Rate limit on "default" bucket (pre-agent, protects the router itself)
      │       ├─ Token available → proceed
      │       ├─ Queue not full  → wait in queue, then proceed
      │       └─ Queue full      → 429 immediately
      │
      └─ [2] RouterOrchestrator.process_request()
                  │
                  ├─ fetch agent cards
                  ├─ LLM route selection  → agent_name known here
                  ├─ [CACHE CHECK] get(agent_name, query)
                  │       ├─ HIT  → stream cached lines, skip agent call
                  │       └─ MISS → call agent, cache response, stream lines
                  └─ stream to caller
"""

import logging
from collections.abc import AsyncGenerator
from typing import List, Optional, Tuple

from router.src.core.cache_service import CacheService
from router.src.core.rate_limiter import RateLimiter, RateLimitExceeded
from router.src.entities import RouterResponse, UserRequest
from router.src.services.router_orchestrator import RouterOrchestrator

logger = logging.getLogger(__name__)


class RequestManager:
    """
    Unified request management layer combining caching and rate limiting.

    Intended to be instantiated once at application startup (singleton).
    The CacheService is shared with the RouterOrchestrator so the cache
    check happens at the correct interception point inside the pipeline.
    """

    def __init__(self):
        self._cache = CacheService()
        # Inject the shared cache so the orchestrator can check/store responses
        # at the right point (after agent selection, before agent call).
        self._orchestrator = RouterOrchestrator(cache=self._cache)
        self._rate_limiter = RateLimiter()

    # ------------------------------------------------------------------
    # Lifecycle
    # ------------------------------------------------------------------

    async def startup(self) -> None:
        """Connect to Redis cache on application startup."""
        await self._cache.connect()
        logger.info("RequestManager started (cache + rate limiter ready)")

    async def shutdown(self) -> None:
        """Gracefully close Redis connection."""
        await self._cache.close()
        logger.info("RequestManager shut down")

    # ------------------------------------------------------------------
    # Main entry point
    # ------------------------------------------------------------------

    async def handle_request(
        self,
        request: UserRequest,
        files: List[Tuple[str, Tuple[str, bytes, str]]],
        token: str,
    ) -> AsyncGenerator[str, None]:
        """
        Process a user request through the full management pipeline.

        Yields JSON-serialised RouterResponse lines (same contract as
        RouterOrchestrator.process_request).
        """
        # ----------------------------------------------------------------
        # Rate limiting on a per-request basis.
        # We use "router" as the bucket name here — this protects the router
        # service itself from being overwhelmed before it even starts routing.
        # Per-agent rate limiting is enforced by configuring individual agents
        # via PUT /monitor/rate-limits/{agent_name}.
        # ----------------------------------------------------------------
        rate_limit_key = request.route if request.route else "router"

        try:
            async with self._rate_limiter.acquire(rate_limit_key):
                # Record total request for per-agent cache stats
                await self._cache.record_agent_request(rate_limit_key)

                # Forward to orchestrator — cache check happens inside it
                # at the correct point (after agent selection, before agent call)
                async for line in self._orchestrator.process_request(
                    request, files, token
                ):
                    yield line

        except RateLimitExceeded as exc:
            logger.warning(str(exc))
            yield self._error_response(
                f"The router is currently overloaded. "
                f"Your request could not be queued "
                f"(queue depth: {exc.queue_depth}). Please retry in a moment.",
                agent_id=rate_limit_key,
            )

    # ------------------------------------------------------------------
    # Operational controls (exposed via monitoring endpoints)
    # ------------------------------------------------------------------

    async def get_cache_stats(self):
        return await self._cache.get_stats()

    async def get_agent_cache_stats(self, agent_name: str):
        return await self._cache.get_agent_stats(agent_name)

    async def clear_cache(self) -> dict:
        count = await self._cache.clear_all()
        return {"cleared_keys": count, "status": "ok"}

    async def clear_agent_cache(self, agent_name: str) -> dict:
        count = await self._cache.clear_agent(agent_name)
        return {"agent": agent_name, "cleared_keys": count, "status": "ok"}

    def get_rate_limit_stats(self) -> dict:
        return self._rate_limiter.get_stats()

    def get_agent_rate_limit_stats(self, agent_name: str):
        return self._rate_limiter.get_agent_stats(agent_name)

    def configure_rate_limit(
        self,
        agent_name: str,
        requests_per_second: float,
        burst_capacity: int,
        queue_size: int,
    ) -> dict:
        self._rate_limiter.configure_agent(
            agent_name, requests_per_second, burst_capacity, queue_size
        )
        return {
            "agent": agent_name,
            "requests_per_second": requests_per_second,
            "burst_capacity": burst_capacity,
            "queue_size": queue_size,
            "status": "configured",
        }

    def remove_rate_limit_config(self, agent_name: str) -> dict:
        removed = self._rate_limiter.remove_agent_config(agent_name)
        return {
            "agent": agent_name,
            "status": "removed" if removed else "not_found",
        }

    def list_rate_limit_configs(self) -> dict:
        return self._rate_limiter.list_configured_agents()

    async def health_check(self) -> dict:
        """Aggregate health check including cache and rate limiter."""
        base = await self._orchestrator.health_check()
        cache_stats = await self._cache.get_stats()
        base["components"]["cache"] = {
            "status": (
                "healthy"
                if cache_stats["connected"] or cache_stats["backend"] == "lru"
                else "degraded"
            ),
            "backend": cache_stats["backend"],
            "hit_rate_pct": cache_stats["hit_rate_pct"],
        }
        base["components"]["rate_limiter"] = {
            "status": "healthy",
            "active_buckets": len(self._rate_limiter.get_stats()["agents"]),
        }
        return base

    @staticmethod
    def _error_response(message: str, agent_id: str = "") -> str:
        return (
            RouterResponse(
                message=message,
                is_int_response=False,
                agent_id=agent_id,
                url="",
            ).model_dump_json()
            + "\n"
        )

from __future__ import annotations

import asyncio
import time
from collections import defaultdict
from collections.abc import Awaitable, Callable

from router.src.entities import UserRequest
from router.src.resilience.cache import SemanticResponseCache
from router.src.resilience.limiter import AdaptiveRateLimiter
from router.src.resilience.models import (
    CacheConfig,
    CacheLookup,
    LimitConfig,
    ResilienceError,
)
from router.src.resilience.stats import RuntimeStats


class ResilientAgentExecutor:
    def __init__(
        self,
        *,
        cache_config: CacheConfig | None = None,
        limit_config: LimitConfig | None = None,
        cache: SemanticResponseCache | None = None,
        limiter: AdaptiveRateLimiter | None = None,
        stats: RuntimeStats | None = None,
    ):
        self.cache = cache or SemanticResponseCache(cache_config)
        self.limiter = limiter or AdaptiveRateLimiter(limit_config)
        self.stats = stats or RuntimeStats()
        self._inflight: dict[str, int] = defaultdict(int)
        self._queue_depths: dict[str, int] = defaultdict(int)
        self._locks: dict[str, asyncio.Lock] = defaultdict(asyncio.Lock)

    async def execute(
        self,
        agent_id: str,
        request: UserRequest,
        files: list[tuple],
        token: str,
        call_agent: Callable[[], Awaitable[str]],
    ) -> str:
        lookup = CacheLookup(
            agent_id=agent_id,
            query=request.query,
            auth_scope=self._auth_scope(token),
            route=request.route,
            has_files=bool(files),
        )

        cached = self.cache.get(lookup)
        if cached is not None:
            self.stats.record_cache_hit()
            return cached
        self.stats.record_cache_miss()

        await self._await_admission(agent_id)

        started = time.monotonic()
        success = False
        try:
            response = await call_agent()
            success = True
            self.cache.set(lookup, response)
            if not lookup.has_files:
                self.stats.record_cache_store()
            return response
        except Exception:
            self.stats.record_agent_error(agent_id)
            raise
        finally:
            elapsed = time.monotonic() - started
            self.stats.record_agent_latency(agent_id, elapsed)
            self.limiter.record_result(
                agent_id, latency_seconds=elapsed, success=success
            )
            async with self._locks[agent_id]:
                self._inflight[agent_id] = max(0, self._inflight[agent_id] - 1)

    def runtime_snapshot(self):
        for agent_id, limit in self.limiter.current_limits().items():
            self.stats.set_current_limit(agent_id, limit)
        return self.stats.snapshot()

    def metrics_text(self) -> str:
        self.runtime_snapshot()
        return self.stats.prometheus_text()

    async def _await_admission(self, agent_id: str) -> None:
        queued = False
        queue_started = time.monotonic()

        while True:
            async with self._locks[agent_id]:
                queue_depth = self._queue_depths[agent_id] + self._inflight[agent_id]
                decision = self.limiter.check(agent_id, queue_depth=queue_depth)
                self.stats.set_current_limit(agent_id, decision.effective_rps)

                if decision.action == "allow":
                    if queued:
                        self._queue_depths[agent_id] = max(
                            0, self._queue_depths[agent_id] - 1
                        )
                    self._inflight[agent_id] += 1
                    self.stats.set_queue_depth(agent_id, self._queue_depths[agent_id])
                    if queued:
                        self.stats.record_queue_wait(
                            agent_id, time.monotonic() - queue_started
                        )
                    return

                if decision.action == "reject":
                    self.stats.record_rate_limit_rejection()
                    raise ResilienceError(
                        "Agent request queue is full",
                        status_code=429,
                        retry_after_seconds=decision.retry_after_seconds,
                    )

                if not queued:
                    self._queue_depths[agent_id] += 1
                    queued = True
                    self.stats.set_queue_depth(agent_id, self._queue_depths[agent_id])

            config = self.limiter.get_config(agent_id)
            if time.monotonic() - queue_started >= config.max_queue_wait_seconds:
                async with self._locks[agent_id]:
                    if queued:
                        self._queue_depths[agent_id] = max(
                            0, self._queue_depths[agent_id] - 1
                        )
                        self.stats.set_queue_depth(
                            agent_id, self._queue_depths[agent_id]
                        )
                    self.stats.record_rate_limit_rejection()
                raise ResilienceError(
                    "Agent request queue wait exceeded",
                    status_code=503,
                    retry_after_seconds=config.max_queue_wait_seconds,
                )

            await asyncio.sleep(max(0.001, min(decision.retry_after_seconds, 0.05)))

    def _auth_scope(self, token: str) -> str:
        return token.strip() if token else "anonymous"

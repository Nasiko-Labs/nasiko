from __future__ import annotations

from statistics import median

from request_manager import redis_keys
from request_manager.models import AgentLimits, AgentStats, GlobalStats


class MetricsRecorder:
    def __init__(self, redis_client) -> None:
        self.redis = redis_client

    async def increment(self, agent_id: str, field: str, amount: int = 1) -> None:
        try:
            await self.redis.hincrby(redis_keys.metrics_agent(agent_id), field, amount)
            await self.redis.hincrby(redis_keys.metrics_global(), field, amount)
        except Exception:
            return

    async def record_latency(self, agent_id: str, latency_ms: int) -> None:
        await self._record_sample(redis_keys.latency(agent_id), latency_ms)

    async def record_queue_wait(self, agent_id: str, wait_ms: int) -> None:
        await self._record_sample(redis_keys.queue_wait(agent_id), wait_ms)

    async def _record_sample(self, key: str, value: int) -> None:
        try:
            await self.redis.rpush(key, str(value))
            if hasattr(self.redis, "ltrim"):
                await self.redis.ltrim(key, -200, -1)
            else:
                values = getattr(self.redis, "lists", {}).get(key)
                while values is not None and len(values) > 200:
                    values.popleft()
        except Exception:
            return

    async def agent_stats(self, agent_id: str, limits: AgentLimits, circuit_state: str) -> AgentStats:
        try:
            metrics = await self.redis.hgetall(redis_keys.metrics_agent(agent_id))
            active_requests = int(await self.redis.get(redis_keys.active(agent_id)) or "0")
            queued_requests = await self.redis.llen(redis_keys.queue(agent_id))
            latency_samples = await self._samples(redis_keys.latency(agent_id))
            queue_samples = await self._samples(redis_keys.queue_wait(agent_id))
        except Exception:
            metrics = {}
            active_requests = 0
            queued_requests = 0
            latency_samples = []
            queue_samples = []

        return AgentStats(
            agent_id=agent_id,
            active_requests=active_requests,
            queued_requests=queued_requests,
            cache_hits=int(metrics.get("cache_hits", "0")),
            cache_misses=int(metrics.get("cache_misses", "0")),
            cache_bypasses=int(metrics.get("cache_bypasses", "0")),
            singleflight_waiters=int(metrics.get("singleflight_waiters", "0")),
            upstream_requests=int(metrics.get("upstream_requests", "0")),
            upstream_errors=int(metrics.get("upstream_errors", "0")),
            queue_timeouts=int(metrics.get("queue_timeouts", "0")),
            circuit_state=circuit_state,
            p50_latency_ms=percentile(latency_samples, 50),
            p95_latency_ms=percentile(latency_samples, 95),
            p95_queue_wait_ms=percentile(queue_samples, 95),
            limits=limits,
        )

    async def global_stats(self, redis_available: bool, agents: list[AgentStats]) -> GlobalStats:
        try:
            metrics = await self.redis.hgetall(redis_keys.metrics_global())
            active_requests = int(await self.redis.get(redis_keys.active_global()) or "0")
        except Exception:
            metrics = {}
            active_requests = 0
        return GlobalStats(
            status="healthy" if redis_available else "degraded",
            redis_available=redis_available,
            active_requests=active_requests,
            cache_hits=int(metrics.get("cache_hits", "0")),
            cache_misses=int(metrics.get("cache_misses", "0")),
            cache_bypasses=int(metrics.get("cache_bypasses", "0")),
            upstream_requests=int(metrics.get("upstream_requests", "0")),
            upstream_errors=int(metrics.get("upstream_errors", "0")),
            queue_timeouts=int(metrics.get("queue_timeouts", "0")),
            agents=agents,
        )

    async def _samples(self, key: str) -> list[float]:
        if hasattr(self.redis, "lrange"):
            raw = await self.redis.lrange(key, 0, -1)
        else:
            raw = list(getattr(self.redis, "lists", {}).get(key, []))
        return [float(value.decode("utf-8") if isinstance(value, bytes) else value) for value in raw]


def percentile(values: list[float], pct: int) -> float:
    if not values:
        return 0.0
    if pct == 50:
        return float(median(values))
    ordered = sorted(values)
    index = int(round((pct / 100) * (len(ordered) - 1)))
    return float(ordered[index])

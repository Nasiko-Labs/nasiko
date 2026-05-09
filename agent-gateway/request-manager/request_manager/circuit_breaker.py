from __future__ import annotations

import time

from request_manager import redis_keys
from request_manager.models import CircuitDecision


class CircuitBreaker:
    def __init__(
        self,
        redis_client,
        window_size: int,
        min_failures: int,
        failure_ratio: float,
        open_seconds: int,
    ) -> None:
        self.redis = redis_client
        self.window_size = window_size
        self.min_failures = min_failures
        self.failure_ratio = failure_ratio
        self.open_seconds = open_seconds

    async def before_request(self, agent_id: str) -> CircuitDecision:
        try:
            state = await self.redis.hgetall(redis_keys.circuit(agent_id))
        except Exception:
            return CircuitDecision(allowed=True, state="degraded")

        now = time.time()
        open_until = float(state.get("open_until", "0") or "0")
        if open_until > now:
            return CircuitDecision(
                allowed=False,
                state="open",
                retry_after_seconds=max(1, int(open_until - now)),
            )
        if state.get("state") == "open":
            await self.redis.hset(
                redis_keys.circuit(agent_id),
                mapping={"state": "half-open", "open_until": "0"},
            )
            return CircuitDecision(allowed=True, state="half-open")
        return CircuitDecision(allowed=True, state=state.get("state", "closed"))

    async def record_result(self, agent_id: str, success: bool) -> None:
        try:
            await self._record_result(agent_id, success)
        except Exception:
            return

    async def _record_result(self, agent_id: str, success: bool) -> None:
        if success:
            state = await self.redis.hgetall(redis_keys.circuit(agent_id))
            if state.get("state") == "half-open":
                await self.redis.hset(
                    redis_keys.circuit(agent_id),
                    mapping={"state": "closed", "open_until": "0"},
                )
            await self._append_outcome(agent_id, "1")
            return

        await self._append_outcome(agent_id, "0")
        outcomes = await self._recent_outcomes(agent_id)
        failures = outcomes.count("0")
        if len(outcomes) >= self.min_failures and failures >= self.min_failures:
            if failures / len(outcomes) >= self.failure_ratio:
                await self.redis.hset(
                    redis_keys.circuit(agent_id),
                    mapping={
                        "state": "open",
                        "open_until": str(time.time() + self.open_seconds),
                    },
                )

    async def _append_outcome(self, agent_id: str, outcome: str) -> None:
        key = redis_keys.outcomes(agent_id)
        await self.redis.rpush(key, outcome)
        if hasattr(self.redis, "ltrim"):
            await self.redis.ltrim(key, -self.window_size, -1)
        else:
            values = getattr(self.redis, "lists", {}).get(key)
            while values is not None and len(values) > self.window_size:
                values.popleft()

    async def _recent_outcomes(self, agent_id: str) -> list[str]:
        key = redis_keys.outcomes(agent_id)
        if hasattr(self.redis, "lrange"):
            return [self._decode(value) for value in await self.redis.lrange(key, 0, -1)]
        return [self._decode(value) for value in getattr(self.redis, "lists", {}).get(key, [])]

    def _decode(self, value) -> str:
        if isinstance(value, bytes):
            return value.decode("utf-8")
        return str(value)

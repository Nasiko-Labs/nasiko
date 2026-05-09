from __future__ import annotations

from request_manager import redis_keys
from request_manager.models import AgentLimits, AgentTarget
from request_manager.settings import RequestManagerSettings


class TargetResolver:
    def __init__(self, redis_client) -> None:
        self.redis = redis_client
        self.memory: dict[str, AgentTarget] = {}

    async def resolve(self, agent_id: str) -> AgentTarget | None:
        try:
            payload = await self.redis.hgetall(redis_keys.target(agent_id))
        except Exception:
            return self.memory.get(agent_id)
        if not payload:
            self.memory.pop(agent_id, None)
            return None
        try:
            target = AgentTarget(
                agent_id=payload["agent_id"],
                public_path=payload["public_path"],
                upstream_url=payload["upstream_url"].rstrip("/"),
                target_revision=payload["target_revision"],
                source=payload["source"],
                namespace=payload["namespace"],
                updated_at=float(payload["updated_at"]),
            )
        except Exception:
            return self.memory.get(agent_id)
        self.memory[agent_id] = target
        return target


class LimitResolver:
    def __init__(self, redis_client, settings: RequestManagerSettings) -> None:
        self.redis = redis_client
        self.settings = settings

    async def resolve(self, agent_id: str) -> AgentLimits:
        defaults = AgentLimits(
            cache_ttl_seconds=self.settings.cache_ttl_seconds,
            max_concurrency=self.settings.max_concurrency_per_agent,
            sustained_rps=self.settings.sustained_rps_per_agent,
            burst_capacity=self.settings.burst_capacity_per_agent,
            max_queue_depth=self.settings.max_queue_depth_per_agent,
            max_queue_wait_ms=self.settings.max_queue_wait_ms,
            cache_enabled=True,
        )
        try:
            override = await self.redis.hgetall(redis_keys.limits(agent_id))
        except Exception:
            return defaults
        if not override:
            return defaults

        data = defaults.model_dump()
        for field in data:
            if field not in override:
                continue
            try:
                if isinstance(data[field], bool):
                    token = override[field].lower()
                    if token in {"1", "true", "yes", "on"}:
                        value = True
                    elif token in {"0", "false", "no", "off"}:
                        value = False
                    else:
                        raise ValueError(f"invalid boolean override for {field}")
                elif isinstance(data[field], int):
                    value = int(float(override[field]))
                elif isinstance(data[field], float):
                    value = float(override[field])
                else:
                    value = override[field]
                AgentLimits(**{**data, field: value})
            except Exception:
                continue
            data[field] = value
        try:
            return AgentLimits(**data)
        except Exception:
            return defaults

    async def update(self, agent_id: str, limits: AgentLimits) -> AgentLimits:
        mapping = {}
        for field, value in limits.model_dump().items():
            if isinstance(value, bool):
                mapping[field] = "true" if value else "false"
            else:
                mapping[field] = str(value)
        await self.redis.hset(redis_keys.limits(agent_id), mapping=mapping)
        return limits

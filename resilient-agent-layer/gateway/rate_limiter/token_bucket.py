import time
import asyncio
from typing import Dict, Optional
import redis.asyncio as aioredis

from gateway.config import (
    REDIS_URL,
    DEFAULT_RATE_LIMIT_RPS,
    DEFAULT_BURST_CAPACITY,
)

# ─── Lua script: atomic token bucket refill + consume ─────────────────────────
TOKEN_BUCKET_LUA = """
local key        = KEYS[1]
local now        = tonumber(ARGV[1])      -- current time in seconds (float)
local rate       = tonumber(ARGV[2])      -- tokens per second
local capacity   = tonumber(ARGV[3])      -- max burst capacity
local ttl        = tonumber(ARGV[4])      -- key TTL in seconds

local data = redis.call('HMGET', key, 'tokens', 'last_refill')
local tokens      = tonumber(data[1]) or capacity
local last_refill = tonumber(data[2]) or now

local elapsed = now - last_refill
local new_tokens = math.min(capacity, tokens + elapsed * rate)

if new_tokens >= 1 then
    redis.call('HMSET', key, 'tokens', new_tokens - 1, 'last_refill', now)
    redis.call('EXPIRE', key, ttl)
    return 1   -- allowed
else
    redis.call('HMSET', key, 'tokens', new_tokens, 'last_refill', now)
    redis.call('EXPIRE', key, ttl)
    return 0   -- denied
end
"""


class RateLimiterConfig:
    """Mutable per-agent rate limit configuration."""

    def __init__(self, rps: float = DEFAULT_RATE_LIMIT_RPS, burst: int = DEFAULT_BURST_CAPACITY):
        self.rps = rps
        self.burst = burst


class RateLimiter:
    """
    Per-agent token bucket rate limiter backed by Redis.
    Falls back to in-process tracking if Redis is unavailable.
    """

    def __init__(self):
        self._redis: Optional[aioredis.Redis] = None
        self._script_sha: Optional[str] = None
        self._configs: Dict[str, RateLimiterConfig] = {}

        # In-process fallback state
        self._local_tokens: Dict[str, float] = {}
        self._local_last: Dict[str, float] = {}
        self._use_redis = True

        # Stats
        self._allowed: Dict[str, int] = {}
        self._denied: Dict[str, int] = {}

    async def connect(self, redis_client: aioredis.Redis):
        try:
            self._redis = redis_client
            self._script_sha = await self._redis.script_load(TOKEN_BUCKET_LUA)
            self._use_redis = True
        except Exception as e:
            print(f"[RateLimiter] Redis unavailable, using local fallback: {e}")
            self._use_redis = False

    def configure_agent(self, agent_id: str, rps: float, burst: int):
        self._configs[agent_id] = RateLimiterConfig(rps=rps, burst=burst)

    def get_config(self, agent_id: str) -> RateLimiterConfig:
        return self._configs.get(agent_id, RateLimiterConfig())

    def update_config(self, agent_id: str, rps: Optional[float] = None, burst: Optional[int] = None):
        cfg = self._configs.setdefault(agent_id, RateLimiterConfig())
        if rps is not None:
            cfg.rps = rps
        if burst is not None:
            cfg.burst = burst

    async def is_allowed(self, agent_id: str) -> bool:
        cfg = self.get_config(agent_id)
        allowed = False

        if self._use_redis and self._redis and self._script_sha:
            try:
                now = time.time()
                key = f"rl:{agent_id}"
                result = await self._redis.evalsha(
                    self._script_sha,
                    1, key,
                    str(now), str(cfg.rps), str(cfg.burst), "120"
                )
                allowed = bool(result)
            except Exception:
                allowed = self._local_check(agent_id, cfg)
        else:
            allowed = self._local_check(agent_id, cfg)

        if allowed:
            self._allowed[agent_id] = self._allowed.get(agent_id, 0) + 1
        else:
            self._denied[agent_id] = self._denied.get(agent_id, 0) + 1
        return allowed

    def _local_check(self, agent_id: str, cfg: RateLimiterConfig) -> bool:
        now = time.monotonic()
        last = self._local_last.get(agent_id, now)
        tokens = self._local_tokens.get(agent_id, float(cfg.burst))
        elapsed = now - last
        tokens = min(cfg.burst, tokens + elapsed * cfg.rps)
        self._local_last[agent_id] = now
        if tokens >= 1:
            self._local_tokens[agent_id] = tokens - 1
            return True
        self._local_tokens[agent_id] = tokens
        return False

    def get_stats(self, agent_id: Optional[str] = None) -> dict:
        if agent_id:
            cfg = self.get_config(agent_id)
            return {
                "agent_id": agent_id,
                "rps": cfg.rps,
                "burst": cfg.burst,
                "allowed": self._allowed.get(agent_id, 0),
                "denied": self._denied.get(agent_id, 0),
            }
        return {
            "backend": "redis" if self._use_redis else "local",
            "agents": {
                aid: self.get_stats(aid)
                for aid in self._configs
            },
            "total_allowed": sum(self._allowed.values()),
            "total_denied": sum(self._denied.values()),
        }

    def all_configs(self) -> dict:
        return {
            aid: {"rps": cfg.rps, "burst": cfg.burst}
            for aid, cfg in self._configs.items()
        }


# Singleton
rate_limiter = RateLimiter()

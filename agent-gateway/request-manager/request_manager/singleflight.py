from __future__ import annotations

import asyncio
import time
import uuid
from dataclasses import dataclass

from request_manager import redis_keys

_RELEASE_IF_OWNER_SCRIPT = """
if redis.call("GET", KEYS[1]) == ARGV[1] then
    redis.call("SET", KEYS[2], "1", "PX", 1000)
    redis.call("DEL", KEYS[1])
    return 1
end
return 0
"""


@dataclass(frozen=True)
class SingleFlightClaim:
    cache_key: str
    token: str
    owner: bool


class SingleFlight:
    def __init__(self, redis_client, wait_ms: int) -> None:
        self.redis = redis_client
        self.wait_ms = wait_ms

    async def claim(self, cache_key: str) -> SingleFlightClaim:
        token = str(uuid.uuid4())
        try:
            acquired = await self.redis.set(
                redis_keys.singleflight_lock(cache_key),
                token,
                px=max(self.wait_ms, 1000),
                nx=True,
            )
        except Exception:
            acquired = True
        return SingleFlightClaim(cache_key=cache_key, token=token, owner=bool(acquired))

    async def wait_until_ready(self, cache_key: str) -> bool:
        deadline = time.monotonic() + (self.wait_ms / 1000)
        while time.monotonic() < deadline:
            try:
                if await self.redis.get(redis_keys.singleflight_ready(cache_key)):
                    return True
            except Exception:
                return False
            await asyncio.sleep(0.05)
        return False

    async def release(self, claim: SingleFlightClaim) -> None:
        if not claim.owner:
            return
        lock_key = redis_keys.singleflight_lock(claim.cache_key)
        ready_key = redis_keys.singleflight_ready(claim.cache_key)

        eval_script = getattr(self.redis, "eval", None)
        if eval_script is not None:
            try:
                await eval_script(_RELEASE_IF_OWNER_SCRIPT, 2, lock_key, ready_key, claim.token)
                return
            except Exception:
                return

        try:
            if await self.redis.get(lock_key) != claim.token:
                return
            await self.redis.set(ready_key, "1", px=1000)
            await self.redis.delete(lock_key)
        except Exception:
            return

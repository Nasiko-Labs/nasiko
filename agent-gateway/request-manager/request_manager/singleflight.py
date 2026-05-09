from __future__ import annotations

import asyncio
import time
import uuid
from dataclasses import dataclass

from request_manager import redis_keys


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
                if not await self.redis.get(redis_keys.singleflight_lock(cache_key)):
                    return True
            except Exception:
                return True
            await asyncio.sleep(0.05)
        return False

    async def release(self, claim: SingleFlightClaim) -> None:
        if not claim.owner:
            return
        try:
            await self.redis.set(redis_keys.singleflight_ready(claim.cache_key), "1", px=1000)
            await self.redis.delete(redis_keys.singleflight_lock(claim.cache_key))
        except Exception:
            return

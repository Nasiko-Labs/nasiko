"""
Redis-backed agent response cache with stampede protection and versioned keys.
"""

import asyncio
import hashlib
import json
import logging
from typing import Any, Dict, Optional, Tuple

import redis.asyncio as aioredis

from router.src.core.events import EventEmitter

logger = logging.getLogger(__name__)


class AgentResponseCache:
    """
    Caches agent JSONRPC responses in Redis keyed by a versioned content hash.

    Key schema: cache:r:{agent_name}:{sha256[:24]}
    This namespace enables agent-scoped SCAN without scanning the full keyspace.

    Stampede protection: a SET NX lock ensures only one caller computes the
    response when multiple requests race on the same cache miss.
    """

    def __init__(
        self,
        redis: aioredis.Redis,
        default_ttl: int = 3600,
        model_version: str = "1",
        prompt_version: str = "1",
        emitter: EventEmitter = None,
    ):
        if emitter is None:
            raise TypeError("AgentResponseCache requires an EventEmitter — pass emitter=")
        self.redis = redis
        self.default_ttl = default_ttl
        self.model_version = model_version
        self.prompt_version = prompt_version
        self.emitter = emitter

    # -------------------------------------------------------------------------
    # Key helpers
    # -------------------------------------------------------------------------

    def _build_key(self, agent_name: str, query: str) -> str:
        raw = f"{agent_name}|{query.strip().lower()}|{self.model_version}|{self.prompt_version}"
        digest = hashlib.sha256(raw.encode()).hexdigest()[:24]
        return f"cache:r:{agent_name}:{digest}"

    def _lock_key(self, cache_key: str) -> str:
        return f"lock:{cache_key}"

    # -------------------------------------------------------------------------
    # Core get / set
    # -------------------------------------------------------------------------

    async def get(
        self, agent_name: str, query: str, has_files: bool
    ) -> Optional[Dict[str, Any]]:
        """Return cached response dict or None. Always None when files are attached."""
        if has_files:
            return None
        key = self._build_key(agent_name, query)
        try:
            raw = await self.redis.get(key)
            if raw:
                await self.redis.incr("cache:stats:hits")
                await self.redis.incr("cache:stats:total_requests")
                await self.emitter.emit("cache_hit", agent=agent_name)
                logger.debug(f"Cache HIT: {key}")
                return json.loads(raw)
            await self.redis.incr("cache:stats:misses")
            await self.redis.incr("cache:stats:total_requests")
            await self.emitter.emit("cache_miss", agent=agent_name)
            logger.debug(f"Cache MISS: {key}")
            return None
        except Exception as e:
            logger.warning(f"Cache GET error: {e}")
            return None

    async def set(
        self,
        agent_name: str,
        query: str,
        response: Dict[str, Any],
        ttl: Optional[int] = None,
    ) -> None:
        if not response:
            return
        key = self._build_key(agent_name, query)
        expiry = ttl if ttl is not None else self.default_ttl
        try:
            await self.redis.setex(key, expiry, json.dumps(response))
            await self.redis.incr(f"cache:stats:keys:{agent_name}")
            logger.debug(f"Cache SET: {key} (ttl={expiry}s)")
        except Exception as e:
            logger.warning(f"Cache SET error: {e}")

    # -------------------------------------------------------------------------
    # Stampede protection
    # -------------------------------------------------------------------------

    async def get_with_stampede_lock(
        self, agent_name: str, query: str, has_files: bool
    ) -> Tuple[Optional[Dict[str, Any]], bool]:
        """
        Returns (cached_response, lock_acquired).

        If cache is populated: (data, False) — caller should return cached data.
        If lock acquired:      (None, True)  — caller must compute + call release_lock.
        If lock not acquired:  waits 200ms, re-checks; if still empty (None, False).
        """
        if has_files:
            return None, False

        key = self._build_key(agent_name, query)
        lock = self._lock_key(key)

        # Fast path: already cached
        cached = await self.get(agent_name, query, has_files=False)
        if cached is not None:
            return cached, False

        # Try to acquire stampede lock (SET NX EX 10)
        try:
            acquired = await self.redis.set(lock, "1", nx=True, ex=10)
        except Exception:
            acquired = False

        if acquired:
            return None, True

        # Lock held by another request — wait briefly then re-check
        await asyncio.sleep(0.2)
        cached = await self.get(agent_name, query, has_files=False)
        return cached, False

    async def release_lock(self, agent_name: str, query: str) -> None:
        key = self._build_key(agent_name, query)
        lock = self._lock_key(key)
        try:
            await self.redis.delete(lock)
        except Exception as e:
            logger.warning(f"Lock release error: {e}")

    # -------------------------------------------------------------------------
    # Invalidation (SCAN-safe, never KEYS *)
    # -------------------------------------------------------------------------

    async def invalidate_agent(self, agent_name: str) -> int:
        """Delete all cached responses for a specific agent. Returns count deleted."""
        pattern = f"cache:r:{agent_name}:*"
        deleted = 0
        try:
            cursor = 0
            while True:
                cursor, keys = await self.redis.scan(cursor, match=pattern, count=100)
                if keys:
                    deleted += await self.redis.delete(*keys)
                if cursor == 0:
                    break
            # Reset key counter for this agent
            await self.redis.delete(f"cache:stats:keys:{agent_name}")
            logger.info(f"Invalidated {deleted} cache entries for agent '{agent_name}'")
        except Exception as e:
            logger.warning(f"Cache invalidate_agent error: {e}")
        return deleted

    async def invalidate_all(self) -> int:
        """Delete all agent response cache entries. Returns count deleted."""
        pattern = "cache:r:*"
        deleted = 0
        try:
            cursor = 0
            while True:
                cursor, keys = await self.redis.scan(cursor, match=pattern, count=100)
                if keys:
                    deleted += await self.redis.delete(*keys)
                if cursor == 0:
                    break
            # Reset global counters
            await self.redis.delete("cache:stats:hits", "cache:stats:misses")
            logger.info(f"Invalidated {deleted} total cache entries")
        except Exception as e:
            logger.warning(f"Cache invalidate_all error: {e}")
        return deleted

    # -------------------------------------------------------------------------
    # Stats
    # -------------------------------------------------------------------------

    async def get_stats(self) -> Dict[str, Any]:
        try:
            hits = int(await self.redis.get("cache:stats:hits") or 0)
            misses = int(await self.redis.get("cache:stats:misses") or 0)
            total = int(await self.redis.get("cache:stats:total_requests") or 0)
            hit_rate = round(hits / total, 4) if total > 0 else 0.0

            # Count live keys via SCAN
            key_count = 0
            cursor = 0
            while True:
                cursor, keys = await self.redis.scan(cursor, match="cache:r:*", count=200)
                key_count += len(keys)
                if cursor == 0:
                    break

            return {
                "hits": hits,
                "misses": misses,
                "total_requests": total,
                "hit_rate": hit_rate,
                "key_count": key_count,
                "ttl_default_seconds": self.default_ttl,
                "model_version": self.model_version,
                "prompt_version": self.prompt_version,
            }
        except Exception as e:
            logger.warning(f"Cache stats error: {e}")
            return {"error": str(e)}

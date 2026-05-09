"""
Resilient request control layer for router caching, queueing, and metrics.
"""

import asyncio
import hashlib
import json
import logging
import time
import uuid
from contextlib import asynccontextmanager
from io import BytesIO
from typing import Any, Dict, List, Tuple, Optional, AsyncIterator

from redis.asyncio import Redis

from router.src.config import settings
from router.src.entities import UserRequest

logger = logging.getLogger(__name__)


class RequestControlError(Exception):
    """Raised when request control operations fail."""


class AgentQueueTimeoutError(RequestControlError):
    """Raised when a request waits too long in an agent queue."""


class RequestControlService:
    """Coordinates cache, per-agent concurrency limits, and runtime stats."""

    # Lua script for atomic slot acquisition.
    # KEYS[1] = active_key, KEYS[2] = queue_key
    # ARGV[1] = queue_token, ARGV[2] = max_concurrent
    # Returns 1 if the slot was acquired, 0 otherwise.
    _LUA_ACQUIRE_SLOT = """
    local active = tonumber(redis.call('GET', KEYS[1]) or '0')
    local head = redis.call('LINDEX', KEYS[2], 0)
    if head == ARGV[1] and active < tonumber(ARGV[2]) then
        redis.call('INCR', KEYS[1])
        redis.call('LPOP', KEYS[2])
        return 1
    end
    return 0
    """

    def __init__(self):
        self.redis = Redis.from_url(settings.REDIS_URL, decode_responses=True)
        self._acquire_slot_script = self.redis.register_script(
            self._LUA_ACQUIRE_SLOT
        )
        self._started_at = time.time()
        self.cache_prefix = "router:cache"
        self.metrics_key = "router:traffic:metrics"
        self.agent_stats_prefix = "router:traffic:agent"
        self.agent_config_prefix = "router:traffic:config"

    async def health_check(self) -> Dict[str, Any]:
        """Check whether Redis-backed traffic control is healthy."""
        try:
            ping_result = await self.redis.ping()
            return {
                "status": "healthy" if ping_result else "unhealthy",
                "cache_enabled": settings.REQUEST_CACHE_ENABLED,
                "rate_limit_enabled": settings.AGENT_RATE_LIMIT_ENABLED,
            }
        except Exception as exc:
            logger.warning("Request control health check failed: %s", exc)
            return {
                "status": "unhealthy",
                "error": str(exc),
                "cache_enabled": settings.REQUEST_CACHE_ENABLED,
                "rate_limit_enabled": settings.AGENT_RATE_LIMIT_ENABLED,
            }

    async def record_request_received(self) -> None:
        """Increment the global request counter."""
        await self._hincr(self.metrics_key, "total_requests", 1)

    async def get_cached_response(
        self, request: UserRequest, files: List[Tuple[str, Tuple[str, bytes, str]]]
    ) -> Optional[Dict[str, Any]]:
        """Return a cached response payload for a repeated request if present."""
        if not settings.REQUEST_CACHE_ENABLED:
            return None

        cache_key = self._cache_key(request, files)
        try:
            payload = await self.redis.get(cache_key)
            if not payload:
                await self._hincr(self.metrics_key, "cache_misses", 1)
                return None

            await self._hincr(self.metrics_key, "cache_hits", 1)
            response = json.loads(payload)
            response["cache_key"] = cache_key
            return response
        except Exception as exc:
            logger.warning("Cache read failed for %s: %s", cache_key, exc)
            return None

    async def store_cached_response(
        self,
        request: UserRequest,
        files: List[Tuple[str, Tuple[str, bytes, str]]],
        response_payload: Dict[str, Any],
    ) -> None:
        """Store an agent response in the shared request cache."""
        if not settings.REQUEST_CACHE_ENABLED:
            return

        cache_key = self._cache_key(request, files)
        payload = dict(response_payload)
        payload["cached_at"] = time.time()
        try:
            await self.redis.setex(
                cache_key,
                settings.REQUEST_CACHE_TTL_SECONDS,
                json.dumps(payload),
            )
            await self._hincr(self.metrics_key, "cache_writes", 1)
        except Exception as exc:
            logger.warning("Cache write failed for %s: %s", cache_key, exc)

    async def clear_cache(self) -> int:
        """Clear all cached request responses."""
        deleted = 0
        async for key in self.redis.scan_iter(match=f"{self.cache_prefix}:*"):
            deleted += await self.redis.delete(key)
        await self.redis.hset(self.metrics_key, mapping={"last_cache_clear_at": time.time()})
        return deleted

    @asynccontextmanager
    async def acquire_agent_slot(self, agent_name: str) -> AsyncIterator[Dict[str, Any]]:
        """
        Acquire capacity for an agent, queueing when the agent is at its limit.
        """
        if not settings.AGENT_RATE_LIMIT_ENABLED:
            yield {"queued": False, "wait_time_ms": 0, "max_concurrent": None}
            return

        queue_key = self._agent_queue_key(agent_name)
        active_key = self._agent_active_key(agent_name)
        queue_token = str(uuid.uuid4())
        max_concurrent = await self.get_agent_limit(agent_name)
        wait_started = time.monotonic()
        acquired = False
        poll_cycles = 0

        try:
            await self.redis.rpush(queue_key, queue_token)
            while True:
                # Atomic check-and-acquire via Lua script.
                got_slot = await self._acquire_slot_script(
                    keys=[active_key, queue_key],
                    args=[queue_token, str(max_concurrent)],
                )

                if got_slot:
                    acquired = True

                    wait_time_ms = int((time.monotonic() - wait_started) * 1000)
                    queued = poll_cycles > 0
                    await self._record_agent_dispatch(
                        agent_name, queued=queued, wait_time_ms=wait_time_ms
                    )
                    yield {
                        "queued": queued,
                        "wait_time_ms": wait_time_ms,
                        "max_concurrent": max_concurrent,
                    }
                    return

                elapsed = time.monotonic() - wait_started
                if elapsed >= settings.AGENT_QUEUE_TIMEOUT_SECONDS:
                    await self.redis.lrem(queue_key, 1, queue_token)
                    await self._hincr(self.metrics_key, "queue_timeouts", 1)
                    await self._hincr(
                        self._agent_stats_key(agent_name), "queue_timeouts", 1
                    )
                    raise AgentQueueTimeoutError(
                        f"Timed out while waiting for agent capacity: {agent_name}"
                    )

                poll_cycles += 1
                await asyncio.sleep(settings.AGENT_QUEUE_POLL_INTERVAL_MS / 1000)

        finally:
            if acquired:
                await self._release_agent_slot(agent_name)

    async def get_agent_limit(self, agent_name: str) -> int:
        """Return the configured concurrency limit for an agent."""
        config = await self.redis.hgetall(self._agent_config_key(agent_name))
        raw_value = config.get("max_concurrent")
        if not raw_value:
            return settings.DEFAULT_AGENT_MAX_CONCURRENT
        return max(1, int(raw_value))

    async def set_agent_limit(self, agent_name: str, max_concurrent: int) -> Dict[str, Any]:
        """Update the concurrency limit for an agent."""
        max_concurrent = max(1, max_concurrent)
        config_key = self._agent_config_key(agent_name)
        await self.redis.hset(
            config_key,
            mapping={
                "max_concurrent": max_concurrent,
                "updated_at": time.time(),
            },
        )
        return await self.get_agent_controls(agent_name)

    async def get_agent_controls(self, agent_name: str) -> Dict[str, Any]:
        """Return live controls and queue state for one agent."""
        stats_key = self._agent_stats_key(agent_name)
        queue_key = self._agent_queue_key(agent_name)
        active_key = self._agent_active_key(agent_name)
        stats = await self.redis.hgetall(stats_key)
        queue_depth = await self.redis.llen(queue_key)
        active_requests = int(await self.redis.get(active_key) or 0)

        return {
            "agent_name": agent_name,
            "max_concurrent": await self.get_agent_limit(agent_name),
            "active_requests": active_requests,
            "queue_depth": queue_depth,
            "stats": self._normalize_stats(stats),
        }

    async def get_runtime_stats(self) -> Dict[str, Any]:
        """Return aggregated runtime metrics for the resilient request layer."""
        metrics = self._normalize_stats(await self.redis.hgetall(self.metrics_key))
        agent_names = set()

        async for key in self.redis.scan_iter(match=f"{self.agent_stats_prefix}:*:stats"):
            try:
                agent_names.add(key.split(":")[-2])
            except IndexError:
                continue

        agents = []
        for agent_name in sorted(agent_names):
            agents.append(await self.get_agent_controls(agent_name))

        queue_depth_total = sum(agent["queue_depth"] for agent in agents)
        active_total = sum(agent["active_requests"] for agent in agents)

        return {
            "stats_since": self._started_at,
            "cache": {
                "enabled": settings.REQUEST_CACHE_ENABLED,
                "ttl_seconds": settings.REQUEST_CACHE_TTL_SECONDS,
                "hits": metrics.get("cache_hits", 0),
                "misses": metrics.get("cache_misses", 0),
                "writes": metrics.get("cache_writes", 0),
                "hit_rate": self._safe_ratio(
                    metrics.get("cache_hits", 0),
                    metrics.get("cache_hits", 0) + metrics.get("cache_misses", 0),
                ),
                "last_clear_at": metrics.get("last_cache_clear_at"),
            },
            "rate_limits": {
                "enabled": settings.AGENT_RATE_LIMIT_ENABLED,
                "default_max_concurrent": settings.DEFAULT_AGENT_MAX_CONCURRENT,
                "queue_timeout_seconds": settings.AGENT_QUEUE_TIMEOUT_SECONDS,
            },
            "traffic": {
                "total_requests": metrics.get("total_requests", 0),
                "queued_requests": metrics.get("queued_requests", 0),
                "queue_timeouts": metrics.get("queue_timeouts", 0),
                "active_requests": active_total,
                "queue_depth": queue_depth_total,
            },
            "agents": agents,
        }

    async def _record_agent_dispatch(
        self, agent_name: str, queued: bool, wait_time_ms: int
    ) -> None:
        """Record queueing and dispatch metrics for an agent."""
        stats_key = self._agent_stats_key(agent_name)
        await self._hincr(stats_key, "requests", 1)
        if queued:
            await self._hincr(self.metrics_key, "queued_requests", 1)
            await self._hincr(stats_key, "queued_requests", 1)
        await self.redis.hset(
            stats_key,
            mapping={
                "last_wait_time_ms": wait_time_ms,
                "last_dispatch_at": time.time(),
            },
        )

    async def record_agent_success(
        self, agent_name: str, response_latency_ms: int, cache_stored: bool
    ) -> None:
        """Record a successful upstream agent response."""
        stats_key = self._agent_stats_key(agent_name)
        await self._hincr(stats_key, "successes", 1)
        await self.redis.hset(
            stats_key,
            mapping={
                "last_response_latency_ms": response_latency_ms,
                "last_success_at": time.time(),
                "cache_stored": int(cache_stored),
            },
        )

    async def record_agent_failure(self, agent_name: str) -> None:
        """Record an upstream failure for an agent."""
        stats_key = self._agent_stats_key(agent_name)
        await self._hincr(stats_key, "failures", 1)
        await self.redis.hset(stats_key, mapping={"last_failure_at": time.time()})

    async def _release_agent_slot(self, agent_name: str) -> None:
        """Release one active slot for an agent after the upstream request completes."""
        if not settings.AGENT_RATE_LIMIT_ENABLED:
            return

        active_key = self._agent_active_key(agent_name)
        active = await self.redis.decr(active_key)
        if active < 0:
            await self.redis.set(active_key, 0)

    async def _hincr(self, key: str, field: str, amount: int) -> None:
        """Increment a Redis hash field with soft-failure logging."""
        try:
            await self.redis.hincrby(key, field, amount)
        except Exception as exc:
            logger.warning("Metric increment failed for %s[%s]: %s", key, field, exc)

    def _cache_key(
        self, request: UserRequest, files: List[Tuple[str, Tuple[str, bytes, str]]]
    ) -> str:
        """Build a stable hash for request-level response caching."""
        payload = {
            "query": request.query.strip(),
            "route": request.route or "",
            "files": self._fingerprint_files(files),
        }
        digest = hashlib.sha256(
            json.dumps(payload, sort_keys=True, ensure_ascii=True).encode("utf-8")
        ).hexdigest()
        return f"{self.cache_prefix}:{digest}"

    def _fingerprint_files(
        self, files: List[Tuple[str, Tuple[str, bytes, str]]]
    ) -> List[Dict[str, str]]:
        """Create content fingerprints for uploaded files."""
        fingerprints = []
        for _, (filename, file_obj, content_type) in files:
            if isinstance(file_obj, BytesIO):
                file_bytes = file_obj.getvalue()
            elif isinstance(file_obj, bytes):
                file_bytes = file_obj
            else:
                try:
                    position = file_obj.tell()
                    file_bytes = file_obj.read()
                    file_obj.seek(position)
                except Exception:
                    file_bytes = b""

            fingerprints.append(
                {
                    "filename": filename,
                    "content_type": content_type,
                    "sha256": hashlib.sha256(file_bytes).hexdigest(),
                }
            )
        return fingerprints

    def _agent_queue_key(self, agent_name: str) -> str:
        return f"{self.agent_stats_prefix}:{agent_name}:queue"

    def _agent_active_key(self, agent_name: str) -> str:
        return f"{self.agent_stats_prefix}:{agent_name}:active"

    def _agent_stats_key(self, agent_name: str) -> str:
        return f"{self.agent_stats_prefix}:{agent_name}:stats"

    def _agent_config_key(self, agent_name: str) -> str:
        return f"{self.agent_config_prefix}:{agent_name}"

    def _normalize_stats(self, stats: Dict[str, str]) -> Dict[str, Any]:
        normalized: Dict[str, Any] = {}
        for key, value in stats.items():
            if value is None:
                normalized[key] = value
                continue
            try:
                if "." in value:
                    normalized[key] = float(value)
                else:
                    normalized[key] = int(value)
            except ValueError:
                normalized[key] = value
        return normalized

    def _safe_ratio(self, numerator: int, denominator: int) -> float:
        if denominator <= 0:
            return 0.0
        return round(numerator / denominator, 4)

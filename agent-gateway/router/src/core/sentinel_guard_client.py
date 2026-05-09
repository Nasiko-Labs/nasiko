"""
Sentinel Guard client for the router orchestrator.

Provides cache-check, rate-limit, queue, and cache-store operations
by communicating with the sentinel-guard agent via HTTP.

Designed with fail-open semantics: if sentinel-guard is unreachable,
all requests are transparently forwarded without caching or rate limiting.
"""

import logging
from typing import Any, Optional

import httpx

logger = logging.getLogger(__name__)

# Default URL — overridden by env or constructor arg
_DEFAULT_URL = "http://sentinel-guard:8000"


class SentinelGuardClient:
    """HTTP client for communicating with the Sentinel Guard agent."""

    def __init__(self, base_url: Optional[str] = None) -> None:
        import os

        self.base_url = (
            base_url
            or os.getenv("SENTINEL_GUARD_URL", _DEFAULT_URL)
        ).rstrip("/")
        self.timeout = httpx.Timeout(10.0, connect=3.0)
        self._available: Optional[bool] = None
        logger.info(f"SentinelGuardClient targeting {self.base_url}")

    async def is_healthy(self) -> bool:
        """Check if sentinel-guard is reachable."""
        try:
            async with httpx.AsyncClient(timeout=self.timeout) as client:
                resp = await client.get(f"{self.base_url}/health")
                self._available = resp.status_code == 200
                return self._available
        except Exception:
            self._available = False
            return False

    async def check_cache(self, query: str, agent_name: str) -> Optional[Any]:
        """
        Check if a response is cached for this query + agent.

        Returns the cached response dict on hit, or None on miss / error.
        """
        try:
            async with httpx.AsyncClient(timeout=self.timeout) as client:
                resp = await client.get(
                    f"{self.base_url}/cache/check",
                    params={"query": query, "agent": agent_name},
                )
                if resp.status_code == 200:
                    data = resp.json()
                    if data.get("hit"):
                        logger.info(f"Sentinel cache HIT for agent={agent_name}")
                        return data.get("result")
        except Exception as exc:
            logger.debug(f"Sentinel cache check failed (fail-open): {exc}")
        return None

    async def check_rate(self, agent_name: str) -> dict:
        """
        Check rate limit for an agent.

        Returns rate limit status dict. On error, returns allowed=True (fail-open).
        """
        try:
            async with httpx.AsyncClient(timeout=self.timeout) as client:
                resp = await client.get(
                    f"{self.base_url}/rate/check/{agent_name}"
                )
                if resp.status_code == 200:
                    return resp.json()
        except Exception as exc:
            logger.debug(f"Sentinel rate check failed (fail-open): {exc}")
        # Fail open
        return {"allowed": True, "remaining": 999, "limit": 999}

    async def enqueue(self, query: str, agent_name: str, payload: Optional[dict] = None) -> dict:
        """
        Enqueue a request when rate-limited.

        Returns queue position info or an error indicator.
        """
        try:
            async with httpx.AsyncClient(timeout=self.timeout) as client:
                resp = await client.post(
                    f"{self.base_url}/proxy",
                    json={
                        "agent": agent_name,
                        "query": query,
                        "payload": payload or {},
                    },
                )
                return resp.json()
        except Exception as exc:
            logger.debug(f"Sentinel enqueue failed: {exc}")
            return {"queued": False, "reason": "sentinel_unreachable"}

    async def store_cache(self, query: str, response: Any, agent_name: str) -> None:
        """
        Store a query-response pair in sentinel-guard's cache.

        Fire-and-forget — errors are silently logged.
        """
        try:
            async with httpx.AsyncClient(timeout=self.timeout) as client:
                await client.post(
                    f"{self.base_url}/cache/store",
                    json={
                        "agent": agent_name,
                        "query": query,
                        "payload": response if isinstance(response, dict) else {"result": response},
                    },
                )
        except Exception as exc:
            logger.debug(f"Sentinel cache store failed (non-critical): {exc}")

    async def get_stats(self) -> Optional[dict]:
        """Fetch runtime stats from sentinel-guard."""
        try:
            async with httpx.AsyncClient(timeout=self.timeout) as client:
                resp = await client.get(f"{self.base_url}/stats")
                if resp.status_code == 200:
                    return resp.json()
        except Exception:
            pass
        return None

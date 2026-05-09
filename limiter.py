"""
In-memory per-agent rate limiting infrastructure.

The limiter protects fake AI agents from burst traffic using a timestamp-based
sliding window. It is route-agnostic and exposes admission helpers that can be
called by routes, middleware, or queue workers. A future Redis-backed limiter
can preserve this API while making admission distributed across processes.
"""

import time
from threading import RLock
from typing import Dict, List, Optional, Any
from dataclasses import asdict, dataclass


@dataclass
class AgentRateLimitConfig:
    """Configuration for an individual agent's rate limits."""
    limit: int
    window_seconds: int


@dataclass
class AgentRateLimitStatus:
    """Current limit status for an agent."""
    agent_name: str
    limit: int
    window_seconds: int
    requests_in_window: int
    remaining_requests: int
    retry_after: float
    allowed: bool

    def to_dict(self) -> Dict[str, Any]:
        """Convert status to a stable JSON-serializable payload."""
        payload = asdict(self)
        payload["agent"] = payload.pop("agent_name")
        payload["retry_after_seconds"] = self.retry_after
        return payload


class RateLimiter:
    """
    Lightweight in-memory per-agent rate limiter.

    Uses a simple timestamp-based sliding window strategy. Each agent tracks
    recent request timestamps and automatically prunes expired entries. A small
    lock protects shared in-memory state when tests or ASGI servers use threads.
    """

    def __init__(self, limits: Optional[Dict[str, AgentRateLimitConfig]] = None):
        """Initialize the rate limiter with per-agent configurations."""
        self.limits: Dict[str, AgentRateLimitConfig] = limits or {
            "translator": AgentRateLimitConfig(limit=5, window_seconds=10),
            "coder": AgentRateLimitConfig(limit=3, window_seconds=10),
            "search": AgentRateLimitConfig(limit=8, window_seconds=10),
            "math": AgentRateLimitConfig(limit=10, window_seconds=10),
        }

        # Each agent owns an independent sliding window. This isolation keeps a
        # noisy agent from consuming capacity reserved for other agents.
        self.request_timestamps: Dict[str, List[float]] = {
            agent: []
            for agent in self.limits
        }
        self.rate_limited_requests: int = 0
        self.per_agent_request_counts: Dict[str, int] = {
            agent: 0
            for agent in self.limits
        }
        self._lock = RLock()

    def _now(self) -> float:
        """Return the current monotonic timestamp."""
        return time.monotonic()

    def _prune_expired(self, agent_name: str) -> None:
        """Remove timestamps outside the agent's active sliding window."""
        if agent_name not in self.limits:
            return
        
        now = self._now()
        window = self.limits[agent_name].window_seconds
        self.request_timestamps[agent_name] = [
            timestamp
            for timestamp in self.request_timestamps.get(agent_name, [])
            if timestamp > now - window
        ]

    def is_allowed(self, agent_name: str) -> bool:
        """
        Check whether a request is allowed for the given agent.

        Args:
            agent_name: Agent identifier.

        Returns:
            True if within limit, False if the agent is overloaded.
        """
        agent_name = self._normalize_agent_name(agent_name)
        with self._lock:
            if agent_name not in self.limits:
                return True
            
            # Prune on every decision so inactive agents do not retain stale
            # capacity usage indefinitely.
            self._prune_expired(agent_name)
            current_requests = len(self.request_timestamps[agent_name])
            return current_requests < self.limits[agent_name].limit

    def check_and_record(
        self,
        agent_name: str,
        *,
        count_rejected: bool = True,
    ) -> Dict[str, Any]:
        """
        Atomically check capacity and record one allowed request.

        This keeps admission control reusable by routes, middleware, or a
        future queue manager. A Redis-backed distributed limiter could replace
        this method with an atomic script or token-bucket primitive later.
        Queue workers can disable rejected counting while polling already
        admitted work for capacity.
        """
        agent_name = self._normalize_agent_name(agent_name)
        with self._lock:
            if agent_name not in self.limits:
                self._ensure_agent_state(agent_name)

            self._prune_expired(agent_name)
            allowed = (
                len(self.request_timestamps[agent_name])
                < self.limits[agent_name].limit
            )

            if allowed:
                # Recording happens in the same critical section as the check,
                # preventing concurrent requests from overshooting the window.
                self.request_timestamps[agent_name].append(self._now())
                self.per_agent_request_counts[agent_name] = (
                    self.per_agent_request_counts.get(agent_name, 0) + 1
                )
            elif count_rejected:
                self.rate_limited_requests += 1

            status = self.get_limit_status(agent_name) or {}
            status["allowed"] = allowed
            return status

    def record_request(self, agent_name: str) -> None:
        """
        Record an allowed request for the agent.

        Args:
            agent_name: Agent identifier.
        """
        agent_name = self._normalize_agent_name(agent_name)
        with self._lock:
            if agent_name not in self.limits:
                self._ensure_agent_state(agent_name)
            
            self._prune_expired(agent_name)
            self.request_timestamps[agent_name].append(self._now())
            self.per_agent_request_counts[agent_name] = (
                self.per_agent_request_counts.get(agent_name, 0) + 1
            )

    def get_remaining_requests(self, agent_name: str) -> int:
        """
        Get the number of remaining requests in the current window.

        Args:
            agent_name: Agent identifier.

        Returns:
            Remaining request count.
        """
        agent_name = self._normalize_agent_name(agent_name)
        with self._lock:
            if agent_name not in self.limits:
                return 0
            
            self._prune_expired(agent_name)
            config = self.limits[agent_name]
            remaining = config.limit - len(self.request_timestamps[agent_name])
            return max(0, remaining)

    def get_limit_status(self, agent_name: str) -> Optional[Dict[str, Any]]:
        """
        Get current limit status for an agent.

        Args:
            agent_name: Agent identifier.

        Returns:
            Dictionary with limit details or None if agent is not configured.
        """
        agent_name = self._normalize_agent_name(agent_name)
        with self._lock:
            if agent_name not in self.limits:
                return None
            
            self._prune_expired(agent_name)
            config = self.limits[agent_name]
            requests_in_window = len(self.request_timestamps[agent_name])
            remaining = max(0, config.limit - requests_in_window)
            retry_after = 0.0
            
            if (
                requests_in_window >= config.limit
                and self.request_timestamps[agent_name]
            ):
                oldest = min(self.request_timestamps[agent_name])
                retry_after = max(0.0, config.window_seconds - (self._now() - oldest))
            
            return AgentRateLimitStatus(
                agent_name=agent_name,
                limit=config.limit,
                window_seconds=config.window_seconds,
                requests_in_window=requests_in_window,
                remaining_requests=remaining,
                retry_after=retry_after,
                allowed=requests_in_window < config.limit,
            ).to_dict()

    def increment_rate_limited_requests(self) -> None:
        """Increment the count of blocked requests."""
        with self._lock:
            self.rate_limited_requests += 1

    def reset_agent(self, agent_name: str) -> None:
        """
        Reset rate limit state for a specific agent.

        Args:
            agent_name: Agent identifier.
        """
        agent_name = self._normalize_agent_name(agent_name)
        with self._lock:
            if agent_name in self.request_timestamps:
                self.request_timestamps[agent_name] = []

    def get_stats(self) -> Dict[str, Any]:
        """
        Get current limiter statistics for debug and observability endpoints.

        The payload exposes both aggregate counters and active window sizes so
        reviewers can verify overload behavior without inspecting internals.
        """
        with self._lock:
            self._prune_all()
            return {
                "rate_limited_requests": self.rate_limited_requests,
                "per_agent_allowed_counts": self.per_agent_request_counts,
                "tracked_agents": list(self.limits.keys()),
                "active_request_timestamps": {
                    agent: len(self.request_timestamps[agent])
                    for agent in self.limits
                },
                "limit_status": {
                    agent: self.get_limit_status(agent)
                    for agent in self.limits
                },
            }

    def _prune_all(self) -> None:
        """Prune expired timestamps for all configured agents."""
        for agent_name in self.limits:
            self._prune_expired(agent_name)

    def get_limits(self) -> Dict[str, Dict[str, Any]]:
        """
        Get configured limits and current usage for all agents.
        """
        with self._lock:
            self._prune_all()
            return {
                agent: self.get_limit_status(agent)
                for agent in self.limits
            }

    def _ensure_agent_state(self, agent_name: str) -> None:
        """Ensure a validated agent has limiter state even if configured later."""
        self.limits.setdefault(
            agent_name,
            AgentRateLimitConfig(limit=10, window_seconds=10),
        )
        self.request_timestamps.setdefault(agent_name, [])
        self.per_agent_request_counts.setdefault(agent_name, 0)

    @staticmethod
    def _normalize_agent_name(agent_name: str) -> str:
        """Normalize agent identifiers for stable per-agent windows."""
        return agent_name.strip().lower()


# Global rate limiter instance
rate_limiter = RateLimiter()

"""
Rate limiter service for managing per-agent request limits and queuing.
"""

import asyncio
import logging
import time
from typing import Dict, Any, List, Optional
from pydantic import BaseModel
from router.src.config import settings

logger = logging.getLogger(__name__)


class AgentStats(BaseModel):
    """Statistics for a single agent's request queue."""
    agent_id: str
    active_requests: int
    queued_requests: int
    total_requests: int
    limit: int
    avg_wait_time_ms: float = 0.0
    avg_response_time_ms: float = 0.0
    error_count: int = 0
    error_rate: float = 0.0


class RateLimiterService:
    """
    Service that manages per-agent rate limits using semaphores for queuing.
    """

    def __init__(self, default_limit: int = 5):
        self.default_limit = default_limit
        self.agent_limits: Dict[str, int] = {}
        self.semaphores: Dict[str, asyncio.Semaphore] = {}
        self.active_counts: Dict[str, int] = {}
        self.queued_counts: Dict[str, int] = {}
        self.total_counts: Dict[str, int] = {}
        self.total_wait_time: Dict[str, float] = {}
        self.total_response_time: Dict[str, float] = {}
        self.completed_requests: Dict[str, int] = {}
        self.error_counts: Dict[str, int] = {}
        self.extra_permits_to_consume: Dict[str, int] = {}  # For decreasing limits
        self._lock = asyncio.Lock()

    async def _ensure_agent(self, agent_id: str):
        """Ensure agent structures are initialized."""
        if agent_id not in self.semaphores:
            async with self._lock:
                if agent_id not in self.semaphores:
                    limit = self.agent_limits.get(agent_id, self.default_limit)
                    self.semaphores[agent_id] = asyncio.Semaphore(limit)
                    self.active_counts[agent_id] = 0
                    self.queued_counts[agent_id] = 0
                    self.total_counts[agent_id] = 0
                    self.total_wait_time[agent_id] = 0.0
                    self.total_response_time[agent_id] = 0.0
                    self.completed_requests[agent_id] = 0
                    self.error_counts[agent_id] = 0
                    self.extra_permits_to_consume[agent_id] = 0
                    logger.info(f"Initialized rate limiter for agent {agent_id} with limit {limit}")

    async def acquire(self, agent_id: str):
        """
        Acquire a slot for the agent. If the limit is reached, this will wait (queue).
        """
        await self._ensure_agent(agent_id)
        
        sem = self.semaphores[agent_id]
        start_time = time.time()
        
        async with self._lock:
            self.total_counts[agent_id] += 1
            # Check if we would block (queue)
            if sem.locked():
                self.queued_counts[agent_id] += 1
                logger.info(f"Request for agent {agent_id} queued. Queue length: {self.queued_counts[agent_id]}")

        try:
            await sem.acquire()
        except Exception as e:
            # Handle potential cancellation or other errors
            async with self._lock:
                if sem.locked() and self.queued_counts[agent_id] > 0:
                     self.queued_counts[agent_id] -= 1
            raise e
        
        # Calculate wait time in milliseconds
        wait_time = (time.time() - start_time) * 1000
        
        async with self._lock:
            if self.queued_counts[agent_id] > 0:
                self.queued_counts[agent_id] -= 1
            self.active_counts[agent_id] += 1
            self.total_wait_time[agent_id] += wait_time

    async def release(self, agent_id: str):
        """Release a slot for the agent."""
        if agent_id in self.semaphores:
            async with self._lock:
                self.active_counts[agent_id] -= 1
                
                # If we have permits to consume (due to limit decrease), don't release to semaphore
                if self.extra_permits_to_consume.get(agent_id, 0) > 0:
                    self.extra_permits_to_consume[agent_id] -= 1
                    logger.info(f"Consuming extra permit for {agent_id} due to limit decrease. Remaining: {self.extra_permits_to_consume[agent_id]}")
                    return

            self.semaphores[agent_id].release()

    def track_completion(self, agent_id: str, response_time_ms: float, success: bool = True):
        """Track request completion, response time and success status."""
        if agent_id in self.total_counts:
            self.completed_requests[agent_id] += 1
            self.total_response_time[agent_id] += response_time_ms
            if not success:
                self.error_counts[agent_id] += 1

    async def set_limit(self, agent_id: str, limit: int):
        """
        Update the limit for an agent dynamically.
        """
        async with self._lock:
            old_limit = self.agent_limits.get(agent_id, self.default_limit)
            self.agent_limits[agent_id] = limit
            
            if agent_id in self.semaphores:
                sem = self.semaphores[agent_id]
                diff = limit - old_limit
                
                if diff > 0:
                    # Increase limit: release the semaphore N times
                    for _ in range(diff):
                        sem.release()
                    logger.info(f"Increased limit for agent {agent_id} from {old_limit} to {limit}")
                elif diff < 0:
                    # Decrease limit: we need to consume permits
                    to_consume = abs(diff)
                    
                    # Try to consume available permits immediately without blocking
                    while to_consume > 0 and not sem.locked():
                        await sem.acquire()
                        to_consume -= 1
                    
                    if to_consume > 0:
                        self.extra_permits_to_consume[agent_id] += to_consume
                        logger.info(f"Decreased limit for agent {agent_id} from {old_limit} to {limit}. Will consume {to_consume} more permits as they are released.")
                    else:
                        logger.info(f"Decreased limit for agent {agent_id} from {old_limit} to {limit} immediately.")
            else:
                # Agent not yet initialized, just setting the limit for future use
                logger.info(f"Set future limit for agent {agent_id} to {limit}")

    def _get_avg_wait_time(self, agent_id: str) -> float:
        """Calculate average wait time for an agent in milliseconds."""
        completed = self.completed_requests.get(agent_id, 0)
        if completed == 0:
            return 0.0
        return self.total_wait_time.get(agent_id, 0.0) / completed

    def _get_avg_response_time(self, agent_id: str) -> float:
        """Calculate average response time for an agent in milliseconds."""
        completed = self.completed_requests.get(agent_id, 0)
        if completed == 0:
            return 0.0
        return self.total_response_time.get(agent_id, 0.0) / completed

    def _get_error_rate(self, agent_id: str) -> float:
        """Calculate error rate for an agent."""
        completed = self.completed_requests.get(agent_id, 0)
        if completed == 0:
            return 0.0
        return self.error_counts.get(agent_id, 0) / completed

    def get_all_stats(self) -> List[AgentStats]:
        """Get statistics for all agents."""
        stats = []
        for agent_id in self.semaphores:
            stats.append(self._build_stats(agent_id))
        return stats

    def get_agent_stats(self, agent_id: str) -> Optional[AgentStats]:
        """Get statistics for a specific agent."""
        if agent_id not in self.semaphores:
            return None
        return self._build_stats(agent_id)

    def _build_stats(self, agent_id: str) -> AgentStats:
        """Build stats object for an agent."""
        return AgentStats(
            agent_id=agent_id,
            active_requests=self.active_counts[agent_id],
            queued_requests=self.queued_counts[agent_id],
            total_requests=self.total_counts[agent_id],
            limit=self.agent_limits.get(agent_id, self.default_limit),
            avg_wait_time_ms=self._get_avg_wait_time(agent_id),
            avg_response_time_ms=self._get_avg_response_time(agent_id),
            error_count=self.error_counts.get(agent_id, 0),
            error_rate=self._get_error_rate(agent_id)
        )


# Global rate limiter instance
agent_rate_limiter = RateLimiterService(default_limit=settings.MAX_CONCURRENT_REQUESTS)

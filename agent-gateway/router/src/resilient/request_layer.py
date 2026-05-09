"""
Resilient Request Layer Orchestrator.
Main component that coordinates caching, rate limiting, and queuing.
"""

import logging
import time
from typing import Optional, Dict, Any, Tuple

try:
    import redis
    from redis import Redis
except ImportError:
    Redis = None

from router.src.resilient.cache_manager import CacheManager
from router.src.resilient.rate_limiter import RateLimiter
from router.src.resilient.request_queue_manager import RequestQueueManager
from router.src.resilient.metrics_collector import MetricsCollector

logger = logging.getLogger(__name__)


class ResilientRequestLayer:
    """
    Orchestrates caching, rate limiting, and queuing for agent requests.
    
    Request flow:
    1. Check cache (return if hit)
    2. Check rate limit (proceed if available)
    3. Queue if rate-limited (if enabled)
    4. Reject if queue full
    5. Forward to agent
    6. Cache response and update metrics
    """

    def __init__(
        self,
        redis_client: Optional[Redis] = None,
        redis_db: int = 1,
        cache_ttl_seconds: int = 3600,
        default_rps: float = 10.0,
        default_burst: int = 50,
    ):
        """
        Initialize resilient request layer.
        
        Args:
            redis_client: Redis connection (creates new if None)
            redis_db: Redis database number
            cache_ttl_seconds: Default cache TTL
            default_rps: Default requests per second limit
            default_burst: Default burst capacity
        """
        self.redis_client = redis_client
        self.redis_db = redis_db
        self.cache_ttl_seconds = cache_ttl_seconds
        self.default_rps = default_rps
        self.default_burst = default_burst
        
        # Initialize components
        self.cache = CacheManager(redis_client, redis_db)
        self.rate_limiter = RateLimiter(redis_client, redis_db)
        self.queue = RequestQueueManager(redis_client, redis_db)
        self.metrics = MetricsCollector(redis_client, redis_db)
        
        logger.info("Resilient request layer initialized")

    def process_request(
        self,
        agent_id: str,
        request_data: Dict[str, Any],
        agent_func,  # Coroutine that processes the request
        ttl_seconds: Optional[int] = None,
    ) -> Tuple[Any, bool, str]:
        """
        Process a request through the resilient layer.
        
        Args:
            agent_id: Target agent identifier
            request_data: Request payload
            agent_func: Async function to call agent (should return response)
            ttl_seconds: Cache TTL (uses default if None)
            
        Returns:
            Tuple of (response, was_cached, status_message)
        """
        ttl = ttl_seconds or self.cache_ttl_seconds
        
        # Step 1: Check cache
        cached = self.cache.get(agent_id, request_data)
        if cached is not None:
            logger.info(f"Cache hit for {agent_id}")
            self.metrics.record_hit(agent_id)
            return cached["response"], True, "served_from_cache"
        
        # Step 2: Check rate limit
        if not self.rate_limiter.can_process(agent_id):
            # Rate limited - try to queue
            logger.warning(f"Rate limit exceeded for {agent_id}, queuing request")
            self.metrics.record_queued(agent_id)
            
            queued = self.queue.enqueue(agent_id, request_data)
            if queued:
                return None, False, f"queued_position_{queued['queue_position']}"
            
            # Queue full
            self.metrics.record_rejected(agent_id)
            return None, False, "rejected_queue_full"
        
        # Step 3: Acquire rate limit tokens
        if not self.rate_limiter.acquire(agent_id):
            logger.warning(f"Rate limit exceeded for {agent_id}")
            self.metrics.record_rejected(agent_id)
            return None, False, "rejected_rate_limit"
        
        # Step 4: Proceed to agent (should be called by wrapper)
        logger.info(f"Forwarding request to {agent_id}")
        return None, False, "forwarded_to_agent"

    def on_response_received(
        self,
        agent_id: str,
        request_data: Dict[str, Any],
        response: Any,
        response_time_ms: float = 0,
        cache: bool = True,
        ttl_seconds: Optional[int] = None,
    ) -> bool:
        """
        Called when agent response is received.
        Updates cache and metrics.
        
        Args:
            agent_id: Agent identifier
            request_data: Original request
            response: Agent response to cache
            response_time_ms: Response time in milliseconds
            cache: Whether to cache this response
            ttl_seconds: Cache TTL
            
        Returns:
            True if cached
        """
        ttl = ttl_seconds or self.cache_ttl_seconds
        
        if cache:
            cached = self.cache.set(agent_id, request_data, response, ttl)
            logger.info(f"Cached response for {agent_id}: {cached}")
        else:
            cached = False
        
        self.metrics.record_miss(agent_id, response_time_ms)
        return cached

    def process_queue(
        self,
        agent_id: str,
        max_requests: int = 10,
        agent_func=None,
    ) -> int:
        """
        Process queued requests for an agent.
        
        Args:
            agent_id: Agent identifier
            max_requests: Maximum number of requests to process
            agent_func: Optional async function to process queued requests
            
        Returns:
            Number of requests processed
        """
        processed = 0
        
        for _ in range(max_requests):
            if not self.queue.peek(agent_id):
                break
            
            # Acquire token for queued request
            if not self.rate_limiter.acquire(agent_id):
                logger.info(f"Rate limit hit while processing queue for {agent_id}")
                break
            
            # Dequeue and process
            dequeued = self.queue.dequeue(agent_id, count=1)
            if not dequeued:
                break
            
            request = dequeued[0]
            logger.info(
                f"Processing queued request for {agent_id}: "
                f"{request.get('request_id', 'unknown')}"
            )
            processed += 1
        
        return processed

    def configure_agent(
        self,
        agent_id: str,
        requests_per_second: float = None,
        burst_capacity: int = None,
        cache_ttl_seconds: int = None,
        max_queue_size: int = None,
    ) -> bool:
        """
        Configure rate limiting and caching for an agent.
        
        Args:
            agent_id: Agent identifier
            requests_per_second: Requests per second limit
            burst_capacity: Burst capacity
            cache_ttl_seconds: Cache TTL
            max_queue_size: Maximum queue size
            
        Returns:
            True if all configs set successfully
        """
        success = True
        
        if requests_per_second is not None or burst_capacity is not None:
            rps = requests_per_second or self.default_rps
            burst = burst_capacity or self.default_burst
            if not self.rate_limiter.set_default_limit(agent_id, rps, burst):
                success = False
        
        if max_queue_size is not None:
            if not self.queue.set_queue_config(agent_id, max_queue_size):
                success = False
        
        logger.info(
            f"Configured agent {agent_id}: "
            f"RPS={requests_per_second}, burst={burst_capacity}, "
            f"cache_ttl={cache_ttl_seconds}, queue_size={max_queue_size}"
        )
        
        return success

    def reset_agent(self, agent_id: str) -> Dict[str, bool]:
        """
        Reset all state for an agent (cache, rate limit, queue).
        
        Returns:
            Dict with reset status for each component
        """
        return {
            "cache_flushed": bool(self.cache.flush_agent(agent_id)),
            "rate_limit_reset": self.rate_limiter.reset_agent(agent_id),
            "queue_cleared": bool(self.queue.clear_queue(agent_id)),
        }

    def health(self) -> Dict[str, Dict[str, Any]]:
        """
        Check health of all components.
        
        Returns:
            Dict with health status of each component
        """
        return {
            "cache": self.cache.health(),
            "rate_limiter": self.rate_limiter.health(),
            "queue": self.queue.health(),
            "metrics": self.metrics.health(),
        }

    def get_comprehensive_stats(self) -> Dict[str, Any]:
        """
        Get comprehensive statistics for all agents and components.
        
        Returns:
            Combined stats from all components
        """
        return {
            "metrics": self.metrics.get_summary(),
            "rate_limits": self.rate_limiter.get_all_configs(),
            "queues": self.queue.get_all_queue_status(),
            "caches": self.cache.all_stats(),
        }

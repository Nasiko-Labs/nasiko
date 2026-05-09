"""
Metrics Collector for resilient request layer.
Tracks request statistics and performance metrics.
"""

import logging
import json
from typing import Optional, Dict, Any
from datetime import datetime

try:
    import redis
    from redis import Redis, RedisError
except ImportError:
    Redis = None
    RedisError = None

logger = logging.getLogger(__name__)


class MetricsCollector:
    """Collects and aggregates metrics for all agents."""

    def __init__(self, redis_client: Optional[Redis] = None, db: int = 1, prefix: str = "resilient"):
        """
        Initialize metrics collector.
        
        Args:
            redis_client: Redis connection
            db: Redis database number
            prefix: Key prefix for namespacing
        """
        self.redis_client = redis_client
        self.db = db
        self.prefix = prefix
        
        if redis_client is None:
            try:
                self.redis_client = Redis(
                    host="localhost",
                    port=6379,
                    db=db,
                    decode_responses=True,
                    socket_connect_timeout=5,
                )
                self.redis_client.ping()
                logger.info("Metrics collector initialized with Redis")
            except Exception as e:
                logger.error(f"Failed to initialize Redis for metrics: {e}")
                self.redis_client = None

    def _build_metrics_key(self, agent_id: str) -> str:
        """Build metrics key for an agent."""
        return f"{self.prefix}:metrics:{agent_id}"

    def record_hit(self, agent_id: str, response_time_ms: float = 0) -> None:
        """Record a cache hit."""
        if not self.redis_client:
            return

        try:
            key = self._build_metrics_key(agent_id)
            self.redis_client.hincrby(key, "cache_hits", 1)
            self.redis_client.hincrby(key, "total_requests", 1)
            
            # Update average response time (simple moving average)
            if response_time_ms > 0:
                self._update_avg_response_time(key, response_time_ms)
            
            logger.debug(f"Recorded cache hit for {agent_id}")
        except RedisError as e:
            logger.error(f"Redis error recording metric: {e}")

    def record_miss(self, agent_id: str, response_time_ms: float = 0) -> None:
        """Record a cache miss."""
        if not self.redis_client:
            return

        try:
            key = self._build_metrics_key(agent_id)
            self.redis_client.hincrby(key, "cache_misses", 1)
            self.redis_client.hincrby(key, "total_requests", 1)
            
            if response_time_ms > 0:
                self._update_avg_response_time(key, response_time_ms)
            
            logger.debug(f"Recorded cache miss for {agent_id}")
        except RedisError as e:
            logger.error(f"Redis error recording metric: {e}")

    def record_queued(self, agent_id: str) -> None:
        """Record a queued request."""
        if not self.redis_client:
            return

        try:
            key = self._build_metrics_key(agent_id)
            self.redis_client.hincrby(key, "requests_queued", 1)
            logger.debug(f"Recorded queued request for {agent_id}")
        except RedisError as e:
            logger.error(f"Redis error recording queued metric: {e}")

    def record_rejected(self, agent_id: str) -> None:
        """Record a rejected request."""
        if not self.redis_client:
            return

        try:
            key = self._build_metrics_key(agent_id)
            self.redis_client.hincrby(key, "requests_rejected", 1)
            logger.debug(f"Recorded rejected request for {agent_id}")
        except RedisError as e:
            logger.error(f"Redis error recording rejected metric: {e}")

    def record_error(self, agent_id: str, error_type: str = "unknown") -> None:
        """Record an error."""
        if not self.redis_client:
            return

        try:
            key = self._build_metrics_key(agent_id)
            error_key = f"error_{error_type}"
            self.redis_client.hincrby(key, error_key, 1)
            logger.debug(f"Recorded {error_type} error for {agent_id}")
        except RedisError as e:
            logger.error(f"Redis error recording error metric: {e}")

    def _update_avg_response_time(self, key: str, response_time_ms: float) -> None:
        """Update average response time using exponential moving average."""
        try:
            # Get current metrics
            metrics = self.redis_client.hgetall(key)
            current_avg = float(metrics.get("avg_response_time_ms", 0))
            request_count = int(metrics.get("total_requests", 1))
            
            # Simple moving average (can be enhanced with exponential moving average)
            if request_count > 1:
                new_avg = (current_avg * (request_count - 1) + response_time_ms) / request_count
            else:
                new_avg = response_time_ms
            
            self.redis_client.hset(key, "avg_response_time_ms", str(new_avg))
        except Exception as e:
            logger.debug(f"Error updating average response time: {e}")

    def get_stats(self, agent_id: str) -> Dict[str, Any]:
        """
        Get metrics for a specific agent.
        
        Returns:
            Dict with statistics for the agent
        """
        if not self.redis_client:
            return {"error": "Redis not available"}

        try:
            key = self._build_metrics_key(agent_id)
            metrics = self.redis_client.hgetall(key)
            
            cache_hits = int(metrics.get("cache_hits", 0))
            cache_misses = int(metrics.get("cache_misses", 0))
            total_requests = cache_hits + cache_misses
            
            hit_ratio = cache_hits / total_requests if total_requests > 0 else 0
            
            return {
                "total_requests": total_requests,
                "cache_hits": cache_hits,
                "cache_misses": cache_misses,
                "cache_hit_ratio": round(hit_ratio, 4),
                "requests_queued": int(metrics.get("requests_queued", 0)),
                "requests_rejected": int(metrics.get("requests_rejected", 0)),
                "avg_response_time_ms": float(metrics.get("avg_response_time_ms", 0)),
                "last_updated": datetime.utcnow().isoformat(),
            }
        except Exception as e:
            logger.error(f"Error getting stats: {e}")
            return {"error": str(e)}

    def get_all_stats(self) -> Dict[str, Dict[str, Any]]:
        """Get metrics for all agents."""
        if not self.redis_client:
            return {}

        try:
            stats = {}
            pattern = f"{self.prefix}:metrics:*"
            keys = self.redis_client.keys(pattern)
            
            for key in keys:
                # Extract agent_id from key
                parts = key.split(":")
                if len(parts) >= 3:
                    agent_id = ":".join(parts[2:])  # Handle agent_ids with colons
                    stats[agent_id] = self.get_stats(agent_id)
            
            return stats
        except Exception as e:
            logger.error(f"Error getting all stats: {e}")
            return {}

    def reset_stats(self, agent_id: str) -> bool:
        """Reset metrics for an agent."""
        if not self.redis_client:
            return False

        try:
            key = self._build_metrics_key(agent_id)
            self.redis_client.delete(key)
            logger.info(f"Reset metrics for {agent_id}")
            return True
        except Exception as e:
            logger.error(f"Error resetting stats: {e}")
            return False

    def reset_all_stats(self) -> int:
        """Reset metrics for all agents. Returns number of agents reset."""
        if not self.redis_client:
            return 0

        try:
            pattern = f"{self.prefix}:metrics:*"
            keys = self.redis_client.keys(pattern)
            
            if keys:
                self.redis_client.delete(*keys)
                logger.info(f"Reset metrics for {len(keys)} agents")
            
            return len(keys)
        except Exception as e:
            logger.error(f"Error resetting all stats: {e}")
            return 0

    def get_summary(self) -> Dict[str, Any]:
        """
        Get summary statistics across all agents.
        
        Returns:
            Overall metrics summary
        """
        try:
            all_stats = self.get_all_stats()
            
            total_requests = sum(s.get("total_requests", 0) for s in all_stats.values())
            total_cache_hits = sum(s.get("cache_hits", 0) for s in all_stats.values())
            total_cache_misses = sum(s.get("cache_misses", 0) for s in all_stats.values())
            total_queued = sum(s.get("requests_queued", 0) for s in all_stats.values())
            total_rejected = sum(s.get("requests_rejected", 0) for s in all_stats.values())
            
            overall_hit_ratio = (
                total_cache_hits / total_requests
                if total_requests > 0
                else 0
            )
            
            return {
                "timestamp": datetime.utcnow().isoformat(),
                "total_agents": len(all_stats),
                "total_requests": total_requests,
                "total_cache_hits": total_cache_hits,
                "total_cache_misses": total_cache_misses,
                "overall_cache_hit_ratio": round(overall_hit_ratio, 4),
                "total_requests_queued": total_queued,
                "total_requests_rejected": total_rejected,
                "agents": all_stats,
            }
        except Exception as e:
            logger.error(f"Error getting summary: {e}")
            return {"error": str(e)}

    def health(self) -> Dict[str, Any]:
        """Check metrics collector health status."""
        if not self.redis_client:
            return {"status": "unavailable", "reason": "Redis not connected"}

        try:
            self.redis_client.ping()
            return {"status": "healthy"}
        except Exception as e:
            return {"status": "unhealthy", "reason": str(e)}

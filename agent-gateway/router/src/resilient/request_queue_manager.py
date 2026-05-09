"""
Request Queue Manager for resilient request layer.
Handles queueing of requests when rate limits are exceeded.
"""

import json
import uuid
import logging
from typing import Optional, Dict, List, Any
from datetime import datetime

try:
    import redis
    from redis import Redis, RedisError
except ImportError:
    Redis = None
    RedisError = None

logger = logging.getLogger(__name__)


class RequestQueueManager:
    """Manages request queuing with priority support."""

    def __init__(self, redis_client: Optional[Redis] = None, db: int = 1, prefix: str = "resilient"):
        """
        Initialize request queue manager.
        
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
                logger.info("Request queue manager initialized with Redis")
            except Exception as e:
                logger.error(f"Failed to initialize Redis for queuing: {e}")
                self.redis_client = None

    def _build_queue_key(self, agent_id: str) -> str:
        """Build queue key for an agent."""
        return f"{self.prefix}:queue:{agent_id}"

    def _build_config_key(self, agent_id: str) -> str:
        """Build queue config key."""
        return f"{self.prefix}:queue_config:{agent_id}"

    def enqueue(
        self,
        agent_id: str,
        request_data: Dict[str, Any],
        priority: int = 5
    ) -> Optional[Dict[str, Any]]:
        """
        Enqueue a request for later processing.
        
        Args:
            agent_id: Agent identifier
            request_data: Request payload
            priority: Priority level (0-10, higher = more urgent)
            
        Returns:
            Enqueued request object with queue position, or None if failed
        """
        if not self.redis_client:
            return None

        try:
            queue_key = self._build_queue_key(agent_id)
            config_key = self._build_config_key(agent_id)
            
            # Check queue size limit
            config = self.redis_client.hgetall(config_key)
            max_queue_size = int(config.get("max_queue_size", 100))
            current_size = self.redis_client.llen(queue_key)
            
            if current_size >= max_queue_size:
                logger.warning(
                    f"Queue full for {agent_id}: {current_size}/{max_queue_size}"
                )
                return None
            
            # Create queued request object
            queued_request = {
                "request_id": str(uuid.uuid4()),
                "request_data": request_data,
                "queued_at": datetime.utcnow().isoformat(),
                "priority": max(0, min(10, priority)),  # Clamp to 0-10
            }
            
            # Store request with priority (higher priority = lower sort key for ZADD)
            score = -priority  # Negative so higher priority sorts first
            self.redis_client.zadd(
                queue_key,
                {json.dumps(queued_request, default=str): score}
            )
            
            logger.info(
                f"Queued request for {agent_id}: "
                f"position {current_size + 1}, priority {priority}"
            )
            
            return {
                **queued_request,
                "queue_position": current_size + 1,
                "max_queue_size": max_queue_size
            }
        except RedisError as e:
            logger.error(f"Redis error during enqueue: {e}")
            return None
        except Exception as e:
            logger.error(f"Error enqueuing request: {e}")
            return None

    def dequeue(
        self,
        agent_id: str,
        count: int = 1
    ) -> List[Dict[str, Any]]:
        """
        Dequeue requests for processing.
        
        Args:
            agent_id: Agent identifier
            count: Number of requests to dequeue
            
        Returns:
            List of dequeued requests
        """
        if not self.redis_client:
            return []

        try:
            queue_key = self._build_queue_key(agent_id)
            dequeued = []
            
            for _ in range(count):
                # Get highest priority (first element, highest score in descending order)
                items = self.redis_client.zrange(queue_key, 0, 0)
                
                if not items:
                    break
                
                item_json = items[0]
                self.redis_client.zrem(queue_key, item_json)
                
                try:
                    request = json.loads(item_json)
                    dequeued.append(request)
                except json.JSONDecodeError as e:
                    logger.error(f"Failed to deserialize queued request: {e}")
            
            if dequeued:
                logger.info(f"Dequeued {len(dequeued)} requests for {agent_id}")
            
            return dequeued
        except RedisError as e:
            logger.error(f"Redis error during dequeue: {e}")
            return []
        except Exception as e:
            logger.error(f"Error dequeuing requests: {e}")
            return []

    def peek(self, agent_id: str) -> Optional[Dict[str, Any]]:
        """
        Peek at next request without removing it.
        
        Returns:
            Next request or None
        """
        if not self.redis_client:
            return None

        try:
            queue_key = self._build_queue_key(agent_id)
            items = self.redis_client.zrange(queue_key, 0, 0)
            
            if items:
                try:
                    return json.loads(items[0])
                except json.JSONDecodeError:
                    return None
            return None
        except Exception as e:
            logger.error(f"Error peeking at queue: {e}")
            return None

    def get_queue_status(self, agent_id: str) -> Dict[str, Any]:
        """
        Get detailed queue status for an agent.
        
        Returns:
            Dict with queue size, oldest request age, etc.
        """
        if not self.redis_client:
            return {"error": "Redis not available"}

        try:
            queue_key = self._build_queue_key(agent_id)
            config_key = self._build_config_key(agent_id)
            
            queue_size = self.redis_client.zcard(queue_key)
            config = self.redis_client.hgetall(config_key)
            max_queue_size = int(config.get("max_queue_size", 100))
            
            # Get oldest request for age calculation
            oldest_request = None
            oldest_age_ms = 0
            
            if queue_size > 0:
                items = self.redis_client.zrange(queue_key, -1, -1)  # Get last item (oldest priority)
                if items:
                    try:
                        oldest_request = json.loads(items[0])
                        queued_at = datetime.fromisoformat(oldest_request["queued_at"])
                        oldest_age_ms = int((datetime.utcnow() - queued_at).total_seconds() * 1000)
                    except (json.JSONDecodeError, KeyError):
                        pass
            
            # Estimate wait time (assuming 1 request per second processing)
            estimated_wait_ms = queue_size * 1000  # Simple estimate
            
            return {
                "queue_size": queue_size,
                "max_queue_size": max_queue_size,
                "queue_utilization": round(queue_size / max_queue_size, 4) if max_queue_size > 0 else 0,
                "oldest_request_age_ms": oldest_age_ms,
                "estimated_wait_time_ms": estimated_wait_ms,
                "is_full": queue_size >= max_queue_size,
            }
        except Exception as e:
            logger.error(f"Error getting queue status: {e}")
            return {"error": str(e)}

    def get_all_queue_status(self) -> Dict[str, Dict[str, Any]]:
        """Get queue status for all agents."""
        if not self.redis_client:
            return {}

        try:
            status = {}
            pattern = f"{self.prefix}:queue_config:*"
            keys = self.redis_client.keys(pattern)
            
            for key in keys:
                # Extract agent_id
                parts = key.split(":")
                if len(parts) >= 4:
                    agent_id = ":".join(parts[3:])  # Handle agent_ids with colons
                    status[agent_id] = self.get_queue_status(agent_id)
            
            return status
        except Exception as e:
            logger.error(f"Error getting all queue status: {e}")
            return {}

    def clear_queue(self, agent_id: str) -> int:
        """
        Clear all queued requests for an agent.
        
        Returns:
            Number of requests removed
        """
        if not self.redis_client:
            return 0

        try:
            queue_key = self._build_queue_key(agent_id)
            count = self.redis_client.zcard(queue_key)
            self.redis_client.delete(queue_key)
            
            if count > 0:
                logger.info(f"Cleared {count} queued requests for {agent_id}")
            
            return count
        except Exception as e:
            logger.error(f"Error clearing queue: {e}")
            return 0

    def set_queue_config(
        self,
        agent_id: str,
        max_queue_size: int = 100,
        queue_enabled: bool = True
    ) -> bool:
        """
        Set queue configuration for an agent.
        
        Returns:
            True if successful
        """
        if not self.redis_client:
            return False

        try:
            config_key = self._build_config_key(agent_id)
            config = {
                "max_queue_size": str(max_queue_size),
                "queue_enabled": str(queue_enabled),
                "configured_at": datetime.utcnow().isoformat(),
            }
            
            self.redis_client.hset(config_key, mapping=config)
            logger.info(
                f"Set queue config for {agent_id}: "
                f"max_size={max_queue_size}, enabled={queue_enabled}"
            )
            return True
        except Exception as e:
            logger.error(f"Error setting queue config: {e}")
            return False

    def health(self) -> Dict[str, Any]:
        """Check queue manager health status."""
        if not self.redis_client:
            return {"status": "unavailable", "reason": "Redis not connected"}

        try:
            self.redis_client.ping()
            return {"status": "healthy"}
        except Exception as e:
            return {"status": "unhealthy", "reason": str(e)}

"""
Rate Limiter for resilient request layer.
Implements token bucket algorithm with per-agent configuration.
"""

import logging
import json
from typing import Optional, Dict, Tuple
from datetime import datetime, timedelta

try:
    import redis
    from redis import Redis, RedisError
except ImportError:
    Redis = None
    RedisError = None

logger = logging.getLogger(__name__)


class RateLimiter:
    """Token bucket rate limiter with per-agent configuration."""

    def __init__(self, redis_client: Optional[Redis] = None, db: int = 1, prefix: str = "resilient"):
        """
        Initialize rate limiter.
        
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
                logger.info("Rate limiter initialized with Redis")
            except Exception as e:
                logger.error(f"Failed to initialize Redis for rate limiting: {e}")
                self.redis_client = None

    def _build_limit_key(self, agent_id: str) -> str:
        """Build rate limit state key."""
        return f"{self.prefix}:rate_limit:{agent_id}"

    def _build_config_key(self, agent_id: str) -> str:
        """Build rate limit config key."""
        return f"{self.prefix}:rate_limit_config:{agent_id}"

    def set_default_limit(
        self,
        agent_id: str,
        requests_per_second: float = 10.0,
        burst_capacity: int = 50
    ) -> bool:
        """
        Set or update rate limit for an agent.
        
        Args:
            agent_id: Agent identifier
            requests_per_second: Refill rate (tokens/sec)
            burst_capacity: Maximum burst capacity
            
        Returns:
            True if successful
        """
        if not self.redis_client:
            return False

        try:
            config_key = self._build_config_key(agent_id)
            config = {
                "requests_per_second": float(requests_per_second),
                "burst_capacity": int(burst_capacity),
                "queue_enabled": True,
                "max_queue_size": burst_capacity * 2
            }
            
            self.redis_client.hset(
                config_key,
                mapping={k: str(v) for k, v in config.items()}
            )
            
            # Initialize tokens to burst capacity
            limit_key = self._build_limit_key(agent_id)
            self.redis_client.hset(
                limit_key,
                mapping={
                    "tokens": str(float(burst_capacity)),
                    "capacity": str(float(burst_capacity)),
                    "last_refill": str(datetime.utcnow().isoformat())
                }
            )
            
            logger.info(
                f"Set rate limit for {agent_id}: "
                f"{requests_per_second} RPS, {burst_capacity} burst"
            )
            return True
        except Exception as e:
            logger.error(f"Error setting rate limit: {e}")
            return False

    def can_process(self, agent_id: str, tokens_required: int = 1) -> bool:
        """
        Check if request can be processed (doesn't consume tokens).
        
        Args:
            agent_id: Agent identifier
            tokens_required: Tokens needed
            
        Returns:
            True if quota available
        """
        if not self.redis_client:
            return True  # Allow if Redis unavailable

        try:
            available = self._refill_tokens(agent_id)
            return available >= tokens_required
        except Exception as e:
            logger.error(f"Error checking rate limit: {e}")
            return True  # Fail open to avoid blocking traffic

    def acquire(self, agent_id: str, tokens_required: int = 1) -> bool:
        """
        Acquire tokens (consumes if successful).
        
        Args:
            agent_id: Agent identifier
            tokens_required: Tokens to consume
            
        Returns:
            True if tokens acquired
        """
        if not self.redis_client:
            return True  # Allow if Redis unavailable

        try:
            limit_key = self._build_limit_key(agent_id)
            current_tokens = self._refill_tokens(agent_id)
            
            if current_tokens >= tokens_required:
                # Consume tokens
                new_tokens = current_tokens - tokens_required
                self.redis_client.hset(limit_key, "tokens", str(new_tokens))
                logger.debug(f"Acquired {tokens_required} tokens for {agent_id}, remaining: {new_tokens}")
                return True
            
            logger.warning(
                f"Rate limit exceeded for {agent_id}: "
                f"required {tokens_required}, available {current_tokens}"
            )
            return False
        except Exception as e:
            logger.error(f"Error acquiring tokens: {e}")
            return True  # Fail open

    def _refill_tokens(self, agent_id: str) -> float:
        """
        Refill tokens based on elapsed time and refill rate.
        
        Returns:
            Current available tokens
        """
        limit_key = self._build_limit_key(agent_id)
        config_key = self._build_config_key(agent_id)
        
        try:
            # Get current state
            limit_state = self.redis_client.hgetall(limit_key)
            config = self.redis_client.hgetall(config_key)
            
            current_tokens = float(limit_state.get("tokens", 0))
            capacity = float(limit_state.get("capacity", 10))
            last_refill_str = limit_state.get("last_refill", datetime.utcnow().isoformat())
            
            refill_rate = float(config.get("requests_per_second", 10.0))
            
            # Parse last refill time
            try:
                last_refill = datetime.fromisoformat(last_refill_str)
            except (ValueError, TypeError):
                last_refill = datetime.utcnow()
            
            # Calculate elapsed time and new tokens
            elapsed = (datetime.utcnow() - last_refill).total_seconds()
            tokens_to_add = elapsed * refill_rate
            
            # Update if refill needed
            if tokens_to_add > 0:
                new_tokens = min(capacity, current_tokens + tokens_to_add)
                self.redis_client.hset(
                    limit_key,
                    mapping={
                        "tokens": str(new_tokens),
                        "last_refill": datetime.utcnow().isoformat()
                    }
                )
                logger.debug(
                    f"Refilled {tokens_to_add:.2f} tokens for {agent_id}, "
                    f"total: {new_tokens:.2f}"
                )
                return new_tokens
            
            return current_tokens
        except Exception as e:
            logger.error(f"Error during token refill: {e}")
            return 0

    def reset_agent(self, agent_id: str) -> bool:
        """Reset rate limiter for an agent (back to full capacity)."""
        if not self.redis_client:
            return False

        try:
            limit_key = self._build_limit_key(agent_id)
            config_key = self._build_config_key(agent_id)
            
            config = self.redis_client.hgetall(config_key)
            capacity = float(config.get("burst_capacity", 50))
            
            self.redis_client.hset(
                limit_key,
                mapping={
                    "tokens": str(capacity),
                    "last_refill": datetime.utcnow().isoformat()
                }
            )
            
            logger.info(f"Reset rate limit for {agent_id} to capacity: {capacity}")
            return True
        except Exception as e:
            logger.error(f"Error resetting rate limit: {e}")
            return False

    def get_current_state(self, agent_id: str) -> Dict[str, any]:
        """
        Get current rate limit state for an agent.
        
        Returns:
            Dict with current tokens, capacity, and config
        """
        if not self.redis_client:
            return {"error": "Redis not available"}

        try:
            limit_key = self._build_limit_key(agent_id)
            config_key = self._build_config_key(agent_id)
            
            limit_state = self.redis_client.hgetall(limit_key)
            config = self.redis_client.hgetall(config_key)
            
            return {
                "current_tokens": float(limit_state.get("tokens", 0)),
                "capacity": float(limit_state.get("capacity", 0)),
                "requests_per_second": float(config.get("requests_per_second", 0)),
                "burst_capacity": int(config.get("burst_capacity", 0)),
                "queue_enabled": config.get("queue_enabled", "True") == "True",
                "max_queue_size": int(config.get("max_queue_size", 0)),
            }
        except Exception as e:
            logger.error(f"Error getting rate limit state: {e}")
            return {"error": str(e)}

    def get_all_configs(self) -> Dict[str, Dict[str, any]]:
        """Get rate limit configuration for all agents."""
        if not self.redis_client:
            return {}

        try:
            configs = {}
            pattern = f"{self.prefix}:rate_limit_config:*"
            keys = self.redis_client.keys(pattern)
            
            for key in keys:
                # Extract agent_id
                parts = key.split(":")
                if len(parts) >= 3:
                    agent_id = ":".join(parts[3:])  # Handle agent_ids with colons
                    configs[agent_id] = self.get_current_state(agent_id)
            
            return configs
        except Exception as e:
            logger.error(f"Error getting all configs: {e}")
            return {}

    def health(self) -> Dict[str, any]:
        """Check rate limiter health status."""
        if not self.redis_client:
            return {"status": "unavailable", "reason": "Redis not connected"}

        try:
            self.redis_client.ping()
            return {"status": "healthy"}
        except Exception as e:
            return {"status": "unhealthy", "reason": str(e)}

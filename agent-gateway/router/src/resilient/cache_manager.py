"""
Cache Manager for resilient request layer.
Handles response caching with TTL and eviction policies.
"""

import json
import hashlib
import logging
from typing import Any, Optional, Dict
from datetime import datetime, timedelta

try:
    import redis
    from redis import Redis, RedisError
except ImportError:
    Redis = None
    RedisError = None

logger = logging.getLogger(__name__)


class CacheManager:
    """Manages response caching with TTL and statistics tracking."""

    def __init__(self, redis_client: Optional[Redis] = None, db: int = 1, prefix: str = "resilient"):
        """
        Initialize cache manager.
        
        Args:
            redis_client: Redis connection (creates new if None)
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
                # Test connection
                self.redis_client.ping()
                logger.info("Cache manager initialized with Redis")
            except Exception as e:
                logger.error(f"Failed to initialize Redis for caching: {e}")
                self.redis_client = None

    def _build_key(self, agent_id: str, request_hash: str) -> str:
        """Build cache key from agent ID and request hash."""
        return f"{self.prefix}:agent:{agent_id}:cache:{request_hash}"

    def _build_hash_key(self, agent_id: str) -> str:
        """Build hash stats key."""
        return f"{self.prefix}:agent:{agent_id}:hash_stats"

    def _calculate_request_hash(self, request_data: Dict[str, Any]) -> str:
        """
        Calculate SHA256 hash of request for deduplication.
        
        Excludes timestamps and request IDs for consistency.
        """
        try:
            # Create a sanitized copy excluding volatile fields
            sanitized = {
                k: v for k, v in request_data.items()
                if k not in ['timestamp', 'request_id', 'trace_id', 'span_id']
            }
            
            # Sort JSON for consistent hashing
            json_str = json.dumps(sanitized, sort_keys=True, default=str)
            return hashlib.sha256(json_str.encode()).hexdigest()
        except Exception as e:
            logger.warning(f"Failed to calculate request hash: {e}")
            # Return hash based on full data if sanitization fails
            return hashlib.sha256(
                json.dumps(request_data, sort_keys=True, default=str).encode()
            ).hexdigest()

    def get(self, agent_id: str, request_data: Dict[str, Any]) -> Optional[Dict[str, Any]]:
        """
        Retrieve cached response if available.
        
        Args:
            agent_id: Agent identifier
            request_data: Request data for hashing
            
        Returns:
            Cached response dict or None
        """
        if not self.redis_client:
            return None

        try:
            request_hash = self._calculate_request_hash(request_data)
            cache_key = self._build_key(agent_id, request_hash)
            
            cached = self.redis_client.get(cache_key)
            if cached:
                try:
                    response = json.loads(cached)
                    
                    # Update hit metrics
                    hash_key = self._build_hash_key(agent_id)
                    self.redis_client.hincrby(hash_key, "hits", 1)
                    
                    logger.info(f"Cache hit for {agent_id}: {request_hash[:8]}")
                    return response
                except json.JSONDecodeError as e:
                    logger.error(f"Failed to deserialize cached response: {e}")
                    self.redis_client.delete(cache_key)
                    return None
            
            # Update miss metrics
            hash_key = self._build_hash_key(agent_id)
            self.redis_client.hincrby(hash_key, "misses", 1)
            
            return None
        except RedisError as e:
            logger.error(f"Redis error during cache get: {e}")
            return None
        except Exception as e:
            logger.error(f"Error during cache retrieval: {e}")
            return None

    def set(
        self,
        agent_id: str,
        request_data: Dict[str, Any],
        response: Any,
        ttl_seconds: int = 3600
    ) -> bool:
        """
        Store response in cache.
        
        Args:
            agent_id: Agent identifier
            request_data: Request data for hashing
            response: Response to cache
            ttl_seconds: TTL in seconds
            
        Returns:
            True if cached successfully
        """
        if not self.redis_client:
            return False

        try:
            request_hash = self._calculate_request_hash(request_data)
            cache_key = self._build_key(agent_id, request_hash)
            
            cache_entry = {
                "response": response,
                "created_at": datetime.utcnow().isoformat(),
                "ttl_seconds": ttl_seconds,
            }
            
            # Store with expiration
            self.redis_client.setex(
                cache_key,
                ttl_seconds,
                json.dumps(cache_entry, default=str)
            )
            
            logger.info(f"Cached response for {agent_id}: {request_hash[:8]} (TTL: {ttl_seconds}s)")
            return True
        except RedisError as e:
            logger.error(f"Redis error during cache set: {e}")
            return False
        except Exception as e:
            logger.error(f"Error during cache storage: {e}")
            return False

    def delete(self, agent_id: str, request_hash: str) -> bool:
        """Delete specific cached response."""
        if not self.redis_client:
            return False

        try:
            cache_key = self._build_key(agent_id, request_hash)
            deleted = self.redis_client.delete(cache_key)
            if deleted:
                logger.info(f"Deleted cache for {agent_id}: {request_hash[:8]}")
            return bool(deleted)
        except RedisError as e:
            logger.error(f"Redis error during cache delete: {e}")
            return False

    def flush_agent(self, agent_id: str) -> int:
        """
        Clear all cached responses for an agent.
        
        Returns:
            Number of keys deleted
        """
        if not self.redis_client:
            return 0

        try:
            pattern = self._build_key(agent_id, "*")
            keys = self.redis_client.keys(pattern)
            if keys:
                deleted = self.redis_client.delete(*keys)
                logger.info(f"Flushed {deleted} cache entries for {agent_id}")
                return deleted
            return 0
        except RedisError as e:
            logger.error(f"Redis error during flush_agent: {e}")
            return 0

    def flush_all(self) -> int:
        """
        Clear all cached responses for all agents.
        
        Returns:
            Number of keys deleted
        """
        if not self.redis_client:
            return 0

        try:
            pattern = f"{self.prefix}:agent:*:cache:*"
            keys = self.redis_client.keys(pattern)
            if keys:
                deleted = self.redis_client.delete(*keys)
                logger.info(f"Flushed all cache entries: {deleted} keys removed")
                return deleted
            return 0
        except RedisError as e:
            logger.error(f"Redis error during flush_all: {e}")
            return 0

    def stats(self, agent_id: str) -> Dict[str, Any]:
        """
        Get cache statistics for an agent.
        
        Returns:
            Dict with hits, misses, hit_ratio, etc.
        """
        if not self.redis_client:
            return {"error": "Redis not available"}

        try:
            hash_key = self._build_hash_key(agent_id)
            stats = self.redis_client.hgetall(hash_key)
            
            hits = int(stats.get("hits", 0))
            misses = int(stats.get("misses", 0))
            total = hits + misses
            hit_ratio = hits / total if total > 0 else 0
            
            return {
                "cache_hits": hits,
                "cache_misses": misses,
                "total_requests": total,
                "cache_hit_ratio": round(hit_ratio, 4),
            }
        except Exception as e:
            logger.error(f"Error retrieving cache stats: {e}")
            return {"error": str(e)}

    def all_stats(self) -> Dict[str, Dict[str, Any]]:
        """Get cache statistics for all agents."""
        if not self.redis_client:
            return {}

        try:
            stats = {}
            # Find all agent hash keys
            pattern = f"{self.prefix}:agent:*:hash_stats"
            keys = self.redis_client.keys(pattern)
            
            for key in keys:
                # Extract agent_id from key
                parts = key.split(":")
                if len(parts) >= 3:
                    agent_id = parts[2]
                    stats[agent_id] = self.stats(agent_id)
            
            return stats
        except Exception as e:
            logger.error(f"Error retrieving all cache stats: {e}")
            return {}

    def ttl(self, agent_id: str, request_data: Dict[str, Any]) -> Dict[str, Any]:
        """
        Get remaining TTL for a cached request.

        Returns:
            Dict with cache_key, ttl_seconds, exists, and remaining_seconds.
        """
        if not self.redis_client:
            return {"error": "Redis not available"}

        try:
            request_hash = self._calculate_request_hash(request_data)
            cache_key = self._build_key(agent_id, request_hash)
            remaining = self.redis_client.ttl(cache_key)
            
            if remaining is None:
                remaining = -1

            return {
                "agent_id": agent_id,
                "request_hash": request_hash,
                "cache_key": cache_key,
                "ttl_seconds": remaining,
                "exists": remaining >= 0,
            }
        except Exception as e:
            logger.error(f"Error retrieving cache TTL: {e}")
            return {"error": str(e)}

    def health(self) -> Dict[str, Any]:
        """Check cache health status."""
        if not self.redis_client:
            return {"status": "unavailable", "reason": "Redis not connected"}

        try:
            self.redis_client.ping()
            info = self.redis_client.info("memory")
            return {
                "status": "healthy",
                "memory_used_bytes": info.get("used_memory", 0),
                "memory_peak_bytes": info.get("used_memory_peak", 0),
            }
        except Exception as e:
            return {"status": "unhealthy", "reason": str(e)}

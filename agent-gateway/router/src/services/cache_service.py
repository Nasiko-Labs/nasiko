import time
import logging
import hashlib
from threading import Lock
from typing import Optional, Dict, Tuple
from router.src.config import settings

logger = logging.getLogger(__name__)

class CacheService:
    """
    A simple in-memory LRU-like cache with TTL for agent responses.
    This ensures that redundant identical queries are not re-processed
    by the agents within the TTL window.
    """
    
    def __init__(self):
        self._cache: Dict[str, Tuple[float, str]] = {}
        self._lock = Lock()
        self.ttl = settings.AGENT_RESPONSE_CACHE_TTL
        self.max_size = settings.AGENT_RESPONSE_CACHE_MAX_SIZE
        
    def _generate_key(self, query: str, agent_url: str) -> str:
        """Generate a unique cache key based on query and agent URL."""
        # Normalize the query string
        normalized_query = query.strip().lower()
        key_content = f"{agent_url}::{normalized_query}".encode('utf-8')
        return hashlib.sha256(key_content).hexdigest()

    def get_cached_response(self, query: str, agent_url: str) -> Optional[str]:
        """
        Retrieve a cached response if it exists and is still valid.
        """
        key = self._generate_key(query, agent_url)
        
        with self._lock:
            if key in self._cache:
                timestamp, response = self._cache[key]
                # Check if it has expired
                if time.time() - timestamp <= self.ttl:
                    logger.info(f"Cache hit for query against agent: {agent_url}")
                    return response
                else:
                    # Expired, remove it
                    del self._cache[key]
        return None

    def set_cached_response(self, query: str, agent_url: str, response: str):
        """
        Store a response in the cache with the current timestamp.
        """
        key = self._generate_key(query, agent_url)
        
        with self._lock:
            # Simple eviction policy if cache gets too large
            if len(self._cache) >= self.max_size:
                # Remove the oldest entry
                oldest_key = min(self._cache.keys(), key=lambda k: self._cache[k][0])
                del self._cache[oldest_key]
                
            self._cache[key] = (time.time(), response)
            logger.debug(f"Cached response for query against agent: {agent_url}")

# Singleton instance
agent_response_cache = CacheService()

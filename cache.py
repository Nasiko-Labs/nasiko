"""
In-memory cache and request-coalescing utilities.

This module owns cache keys, cached response storage, and per-key async locks
that prevent cache stampedes. It is intentionally local and lightweight for the
MVP; Redis can later replace both the dictionary store and the per-key locking
strategy for multi-process deployments.
"""

import asyncio
import hashlib
from contextlib import asynccontextmanager
from typing import Optional, Dict, Any
from datetime import datetime
from dataclasses import dataclass


# Locks are keyed by the same hash used for cached responses. A waiting request
# does not re-run the agent; it waits for the owner request, re-checks cache,
# and returns the populated value.
cache_locks: Dict[str, asyncio.Lock] = {}
cache_lock_refs: Dict[str, int] = {}


def get_cache_lock(key: str) -> asyncio.Lock:
    """
    Return the per-cache-key lock used to coalesce duplicate cache misses.

    This is intentionally in-memory and event-loop friendly. A future Redis
    cache could replace this with distributed locking, but this keeps the MVP
    simple and prevents local cache stampedes.
    """
    if key not in cache_locks:
        cache_locks[key] = asyncio.Lock()
    return cache_locks[key]


@asynccontextmanager
async def cache_key_lock(key: str):
    """
    Acquire a per-cache-key lock and clean it up when the last waiter exits.

    The reference count includes tasks waiting on the lock, so cleanup will not
    remove a lock while another identical request is queued behind it.
    """
    lock = get_cache_lock(key)
    cache_lock_refs[key] = cache_lock_refs.get(key, 0) + 1

    try:
        async with lock:
            yield
    finally:
        remaining_refs = cache_lock_refs.get(key, 1) - 1
        if remaining_refs <= 0:
            cache_lock_refs.pop(key, None)
            if not lock.locked():
                cache_locks.pop(key, None)
        else:
            cache_lock_refs[key] = remaining_refs


@dataclass
class CacheEntry:
    """
    Individual cache entry with metadata.
    
    Stores the response along with metadata about when it was cached
    and how many times it's been accessed.
    """
    key: str
    value: str  # The cached AI response
    created_at: datetime
    hit_count: int = 0

    def to_dict(self) -> Dict[str, Any]:
        """Convert entry to dictionary for debugging."""
        return {
            "key": self.key,
            "created_at": self.created_at.isoformat(),
            "hit_count": self.hit_count,
            "value_length": len(self.value)
        }


def generate_cache_key(agent: str, query: str) -> str:
    """
    Generate a stable cache key from agent name and query.
    
    Uses SHA256 to create a cryptographically stable hash that uniquely
    identifies the (agent, query) combination.
    
    Args:
        agent: AI agent name
        query: User query
        
    Returns:
        Hex-encoded SHA256 hash as cache key
    """
    # Keep the key deterministic and opaque; the raw prompt is not exposed in
    # cache internals or debug output.
    combined = f"{agent}::{query}".encode("utf-8")
    return hashlib.sha256(combined).hexdigest()


class CacheManager:
    """
    In-memory cache management for AI request responses.
    
    EXTENSION POINT:
    - Replace in-memory dict with Redis backend
    - Add distributed cache invalidation
    - Add TTL/expiration support
    - Add cache eviction policy (LRU)
    - Add compression for large values
    - Add cache warming strategies
    """

    def __init__(self, max_size: int = 1000):
        """
        Initialize cache manager.
        
        Args:
            max_size: Maximum number of cached entries
                     (Phase 4: will be unlimited with Redis backend)
        """
        self.max_size = max_size
        # In-memory store is intentionally process-local. Redis can replace
        # this dictionary later without changing callers that use get/set.
        self.store: Dict[str, CacheEntry] = {}

    def get(self, key: str) -> Optional[str]:
        """
        Retrieve value from cache.
        
        Args:
            key: Cache key (typically from generate_cache_key)
            
        Returns:
            Cached response string or None if not found
        """
        if key not in self.store:
            return None
        
        entry = self.store[key]
        entry.hit_count += 1
        
        return entry.value

    def set(self, key: str, value: str) -> None:
        """
        Store value in cache.
        
        Args:
            key: Cache key (typically from generate_cache_key)
            value: Response string to cache
        """
        # Keep eviction simple for demos: remove the oldest entry when capacity
        # is reached. A production cache would likely use TTL/LRU semantics.
        if len(self.store) >= self.max_size:
            oldest_key = min(
                self.store.keys(),
                key=lambda k: self.store[k].created_at,
            )
            del self.store[oldest_key]
        
        entry = CacheEntry(
            key=key,
            value=value,
            created_at=datetime.utcnow(),
        )
        self.store[key] = entry

    def delete(self, key: str) -> bool:
        """
        Delete specific cache entry.
        
        Args:
            key: Cache key to delete
            
        Returns:
            True if deleted, False if not found
        """
        if key in self.store:
            del self.store[key]
            return True
        return False

    def clear(self) -> None:
        """Clear all cache entries."""
        self.store.clear()

    def get_stats(self) -> Dict[str, Any]:
        """
        Get cache statistics.
        
        Returns:
            Dictionary with cache performance metrics
        """
        total_hits = sum(entry.hit_count for entry in self.store.values())
        
        return {
            "size": len(self.store),
            "max_size": self.max_size,
            "utilization_percent": (
                len(self.store) / self.max_size * 100
                if self.max_size > 0
                else 0
            ),
            "total_hits": total_hits,
            "entries": [entry.to_dict() for entry in self.store.values()],
        }


# Global cache instance
cache_manager = CacheManager()

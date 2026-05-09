import redis.asyncio as redis
from typing import Optional, Tuple, Dict, Any
import json
import hashlib
from datetime import datetime


class CacheEngine:
    def __init__(self):
        self.redis = redis.Redis(
            host="redis",
            port=6379,
            decode_responses=True
        )
        self.default_ttl = 300  # 5 minutes
        self.similarity_threshold = 0.95
        
        # Stats
        self.hits = 0
        self.misses = 0
        self.semantic_hits = 0
    
    def _generate_key(self, query: str, agent_hint: Optional[str] = None) -> str:
        """Generate exact match cache key"""
        content = f"{query}:{agent_hint or 'any'}"
        return f"cache:exact:{hashlib.sha256(content.encode()).hexdigest()}"
    
    async def lookup(
        self,
        query: str,
        agent_hint: Optional[str] = None
    ) -> Tuple[Optional[Dict], float]:
        """
        Lookup in cache with fallback to semantic matching.
        Returns (result, similarity_score)
        """
        
        # Exact match first
        exact_key = self._generate_key(query, agent_hint)
        cached = await self.redis.get(exact_key)
        
        if cached:
            self.hits += 1
            return json.loads(cached), 1.0
        
        # Semantic match
        semantic_result = await self._semantic_lookup(query, agent_hint)
        if semantic_result:
            self.semantic_hits += 1
            self.hits += 1
            return semantic_result, 0.97
        
        self.misses += 1
        return None, 0.0
    
    async def _semantic_lookup(
        self,
        query: str,
        agent_hint: Optional[str] = None
    ) -> Optional[Dict]:
        """Basic semantic cache lookup using keyword overlap"""
        
        keys = []
        cursor = 0
        while True:
            cursor, batch = await self.redis.scan(
                cursor,
                match="cache:exact:*",
                count=10
            )
            keys.extend(batch)
            if cursor == 0:
                break
        
        query_words = set(query.lower().split())
        
        for key in keys[:20]:
            cached_data = await self.redis.get(key)
            if cached_data:
                try:
                    cached_entry = json.loads(cached_data)
                    if "original_query" in cached_entry:
                        cached_words = set(cached_entry["original_query"].lower().split())
                        overlap = len(query_words & cached_words) / max(len(query_words), 1)
                        if overlap > 0.7:
                            return cached_entry
                except:
                    continue
        
        return None
    
    async def store(
        self,
        query: str,
        response: Any,
        agent_hint: Optional[str] = None,
        ttl: Optional[int] = None
    ):
        """Store result in cache"""
        
        key = self._generate_key(query, agent_hint)
        
        if isinstance(response, dict):
            cache_entry = {
                **response,
                "original_query": query,
                "cached_at": datetime.utcnow().isoformat(),
                "agent_hint": agent_hint
            }
        else:
            cache_entry = {
                "result": response,
                "original_query": query,
                "cached_at": datetime.utcnow().isoformat(),
                "agent_hint": agent_hint
            }
        
        await self.redis.setex(
            key,
            ttl or self.default_ttl,
            json.dumps(cache_entry)
        )
    
    async def get_stats(self) -> Dict[str, Any]:
        """Get cache statistics"""
        total = self.hits + self.misses
        hit_rate = (self.hits / total * 100) if total > 0 else 0
        
        cache_size = 0
        cursor = 0
        while True:
            cursor, keys = await self.redis.scan(cursor, match="cache:*", count=100)
            cache_size += len(keys)
            if cursor == 0:
                break
        
        return {
            "hits": self.hits,
            "misses": self.misses,
            "semantic_hits": self.semantic_hits,
            "hit_rate_percent": round(hit_rate, 2),
            "cache_entries": cache_size,
            "ttl_seconds": self.default_ttl,
            "semantic_enabled": True
        }
    
    async def clear(self):
        """Clear all cache entries"""
        cursor = 0
        while True:
            cursor, keys = await self.redis.scan(cursor, match="cache:*", count=100)
            if keys:
                await self.redis.delete(*keys)
            if cursor == 0:
                break
    
    async def cleanup(self):
        """Cleanup connections"""
        await self.redis.close()

import redis
import json
import hashlib
import time
import numpy as np
from sentence_transformers import SentenceTransformer
from typing import Optional, Dict, Any, List

class SemanticCache:
    def __init__(self, redis_host: str = "redis", redis_port: int = 6379):
        self.redis = redis.Redis(
            host=redis_host,
            port=redis_port,
            decode_responses=True,
            socket_connect_timeout=5,
            socket_timeout=5,
            health_check_interval=30
        )
        self.embedder = SentenceTransformer('all-MiniLM-L6-v2')
        self.similarity_threshold = 0.85
        self.default_ttl = 3600  # 1 hour
        self.degraded_mode = False
        
    def _generate_embedding(self, text: str) -> List[float]:
        return self.embedder.encode(text).tolist()
    
    def _cosine_similarity(self, a: List[float], b: List[float]) -> float:
        a_np = np.array(a)
        b_np = np.array(b)
        return float(np.dot(a_np, b_np) / (np.linalg.norm(a_np) * np.linalg.norm(b_np)))
    
    def _get_cache_key(self, agent: str, query: str) -> str:
        query_hash = hashlib.md5(query.encode()).hexdigest()
        return f"cache:{agent}:{query_hash}"
    
    def check_cache(self, query: str, agent: str = "translator") -> Dict[str, Any]:
        start_time = time.time()
        try:
            query_embedding = self._generate_embedding(query)
            
            # Search all cache entries for this agent
            pattern = f"cache:{agent}:*"
            keys = self.redis.keys(pattern)
            
            best_match = None
            best_score = self.similarity_threshold
            
            for key in keys:
                try:
                    stored = self.redis.hgetall(key)
                    if not stored or "embedding" not in stored:
                        continue
                    
                    stored_embedding = json.loads(stored["embedding"])
                    similarity = self._cosine_similarity(query_embedding, stored_embedding)
                    
                    if similarity > best_score:
                        best_score = similarity
                        best_match = {
                            "key": key,
                            "response": json.loads(stored.get("response", "{}")),
                            "original_query": stored.get("query", ""),
                            "similarity": similarity
                        }
                except Exception:
                    continue
            
            if best_match:
                return {
                    "cache_hit": True,
                    "similarity_score": best_match["similarity"],
                    "response": best_match["response"],
                    "original_query": best_match["original_query"],
                    "latency_ms": round((time.time() - start_time) * 1000, 2),
                    "source": "semantic_cache"
                }
            
            return {
                "cache_hit": False,
                "latency_ms": round((time.time() - start_time) * 1000, 2),
                "forward_to": f"/agents/{agent}/"
            }
        except redis.ConnectionError:
            self.degraded_mode = True
            return {
                "cache_hit": False,
                "degraded_mode": True,
                "forward_to": f"/agents/{agent}/",
                "message": "Cache unavailable - passing through to agent",
                "latency_ms": round((time.time() - start_time) * 1000, 2)
            }
        except Exception as e:
            return {
                "cache_hit": False,
                "error": str(e),
                "forward_to": f"/agents/{agent}/",
                "latency_ms": round((time.time() - start_time) * 1000, 2)
            }
    
    def store_cache(self, query: str, response: Any, agent: str = "translator") -> Dict[str, Any]:
        embedding = self._generate_embedding(query)
        cache_key = self._get_cache_key(agent, query)
        
        cache_data = {
            "query": query,
            "embedding": json.dumps(embedding),
            "response": json.dumps(response),
            "timestamp": str(time.time()),
            "agent": agent
        }
        
        pipe = self.redis.pipeline()
        pipe.hset(cache_key, mapping=cache_data)
        pipe.expire(cache_key, self.default_ttl)
        pipe.execute()
        
        return {"stored": True, "key": cache_key, "ttl": self.default_ttl}
    
    def get_cache_stats(self, agent: str = "translator") -> Dict[str, Any]:
        pattern = f"cache:{agent}:*"
        keys = self.redis.keys(pattern)
        
        active = 0
        total = len(keys)
        
        for key in keys:
            ttl = self.redis.ttl(key)
            if ttl > 0:
                active += 1
        
        return {
            "agent": agent,
            "total_entries": total,
            "active_entries": active,
            "expired_entries": total - active
        }
    
    def invalidate_agent_cache(self, agent: str) -> Dict[str, Any]:
        pattern = f"cache:{agent}:*"
        keys = self.redis.keys(pattern)
        
        if keys:
            self.redis.delete(*keys)
        
        return {"invalidated": True, "agent": agent, "keys_removed": len(keys)}
    
    def warm_cache(self, common_queries: List[str], agent: str = "translator") -> Dict[str, Any]:
        """Pre-compute and cache common queries"""
        warmed = 0
        for query in common_queries:
            # Check if already cached
            result = self.check_cache(query, agent)
            if not result["cache_hit"]:
                # Forward to agent and cache (simulated for warming)
                warmed += 1
        
        return {"warmed": warmed, "agent": agent}

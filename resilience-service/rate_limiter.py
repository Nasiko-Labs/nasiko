import redis
import time
import json
from typing import Dict, Any, Optional
from enum import Enum

class Priority(Enum):
    P0 = "P0"  # Critical - can borrow from P2
    P1 = "P1"  # Standard
    P2 = "P2"  # Background - can be borrowed from

class AdaptiveRateLimiter:
    def __init__(self, redis_host: str = "redis", redis_port: int = 6379):
        self.redis = redis.Redis(
            host=redis_host,
            port=redis_port,
            decode_responses=True,
            socket_connect_timeout=5,
            socket_timeout=5
        )
        self.default_tokens = 100  # per minute
        self.token_refill_rate = 10  # every 10 seconds
        self.max_queue_wait = 30  # seconds
        
    def _get_bucket_key(self, agent: str) -> str:
        return f"ratelimit:{agent}:tokens"
    
    def _get_queue_key(self, agent: str, priority: Priority) -> str:
        return f"queue:{agent}:{priority.value}"
    
    def _get_error_key(self, agent: str) -> str:
        return f"errors:{agent}"
    
    def _get_total_key(self, agent: str) -> str:
        return f"total:{agent}"
    
    def check_rate_limit(self, agent: str, priority: str = "P1", request_id: str = None) -> Dict[str, Any]:
        if request_id is None:
            request_id = f"req_{time.time()}_{agent}"
        
        try:
            priority_enum = Priority(priority)
        except ValueError:
            priority_enum = Priority.P1
        
        bucket_key = self._get_bucket_key(agent)
        
        # Initialize bucket if not exists
        if not self.redis.exists(bucket_key):
            self.redis.setex(bucket_key, 60, self.default_tokens)
        
        current_tokens = int(self.redis.get(bucket_key) or 0)
        
        # Track total requests
        self.redis.incr(self._get_total_key(agent))
        
        if current_tokens > 0:
            # Allow request
            self.redis.decr(bucket_key)
            
            return {
                "allowed": True,
                "remaining_tokens": current_tokens - 1,
                "priority": priority_enum.value,
                "queued": False,
                "request_id": request_id,
                "agent": agent
            }
        else:
            # No tokens - queue the request
            queue_key = self._get_queue_key(agent, priority_enum)
            queue_position = self.redis.rpush(queue_key, request_id)
            queue_length = self.redis.llen(queue_key)
            
            # P0 priority boost: steal from P2 if available
            if priority_enum == Priority.P0:
                p2_queue = self._get_queue_key(agent, Priority.P2)
                if self.redis.llen(p2_queue) > 0:
                    # Remove oldest P2 request and allow P0
                    stolen = self.redis.lpop(p2_queue)
                    if stolen:
                        self.redis.decr(bucket_key)  # Use the token we just freed
                        # Remove P0 from queue since it's now being served
                        self.redis.lrem(queue_key, 0, request_id)
                        return {
                            "allowed": True,
                            "priority_boost": "P0 stole from P2 queue",
                            "stolen_request": stolen,
                            "queued": False,
                            "request_id": request_id,
                            "agent": agent
                        }
            
            estimated_wait = queue_length * 2  # Rough estimate: 2s per request
            
            return {
                "allowed": False,
                "queued": True,
                "queue_position": queue_position,
                "queue_length": queue_length,
                "estimated_wait_seconds": estimated_wait,
                "priority": priority_enum.value,
                "request_id": request_id,
                "agent": agent,
                "max_wait_sla": self.max_queue_wait
            }
    
    def refill_tokens(self, agent: str, amount: Optional[int] = None) -> Dict[str, Any]:
        if amount is None:
            amount = self.token_refill_rate
        
        bucket_key = self._get_bucket_key(agent)
        new_total = self.redis.incrby(bucket_key, amount)
        
        # Process queued requests after refill
        processed = self._process_queues(agent)
        
        return {
            "refilled": amount,
            "new_total": new_total,
            "agent": agent,
            "queued_processed": processed
        }
    
    def _process_queues(self, agent: str) -> Dict[str, int]:
        """Process queued requests after token refill"""
        bucket_key = self._get_bucket_key(agent)
        processed = {"P0": 0, "P1": 0, "P2": 0}
        
        for priority in [Priority.P0, Priority.P1, Priority.P2]:
            queue_key = self._get_queue_key(agent, priority)
            
            while self.redis.llen(queue_key) > 0:
                current_tokens = int(self.redis.get(bucket_key) or 0)
                if current_tokens <= 0:
                    break
                
                request_id = self.redis.lpop(queue_key)
                if request_id:
                    self.redis.decr(bucket_key)
                    processed[priority.value] += 1
                else:
                    break
        
        return processed
    
    def get_queue_status(self, agent: str) -> Dict[str, Any]:
        return {
            "agent": agent,
            "queues": {
                "P0": {
                    "length": self.redis.llen(self._get_queue_key(agent, Priority.P0)),
                    "items": self.redis.lrange(self._get_queue_key(agent, Priority.P0), 0, -1)
                },
                "P1": {
                    "length": self.redis.llen(self._get_queue_key(agent, Priority.P1)),
                    "items": self.redis.lrange(self._get_queue_key(agent, Priority.P1), 0, -1)
                },
                "P2": {
                    "length": self.redis.llen(self._get_queue_key(agent, Priority.P2)),
                    "items": self.redis.lrange(self._get_queue_key(agent, Priority.P2), 0, -1)
                }
            },
            "tokens_remaining": int(self.redis.get(self._get_bucket_key(agent)) or 0)
        }
    
    def update_rate_limit(self, agent: str, tokens_per_minute: int) -> Dict[str, Any]:
        bucket_key = self._get_bucket_key(agent)
        self.redis.setex(bucket_key, 60, tokens_per_minute)
        return {"updated": True, "agent": agent, "tokens_per_minute": tokens_per_minute}

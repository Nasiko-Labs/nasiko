import redis.asyncio as redis
import time
import asyncio
import json
from typing import Dict, Any


class TokenBucket:
    """Token bucket algorithm implementation using Redis"""
    
    def __init__(self, redis_client, key_prefix: str, rate: int, window: int):
        self.redis = redis_client
        self.key = f"rate_limit:{key_prefix}"
        self.rate = rate
        self.window = window
    
    async def acquire(self) -> bool:
        """Try to acquire a token. Returns True if allowed, False if rate limited."""
        
        now = time.time()
        window_start = now - self.window
        
        # Lua script for atomic token bucket operation
        lua_script = """
        local key = KEYS[1]
        local now = tonumber(ARGV[1])
        local window_start = tonumber(ARGV[2])
        local max_tokens = tonumber(ARGV[3])
        local window = tonumber(ARGV[4])
        
        redis.call('ZREMRANGEBYSCORE', key, 0, window_start)
        local current_count = redis.call('ZCARD', key)
        
        if current_count < max_tokens then
            local member = now .. '-' .. math.random()
            redis.call('ZADD', key, now, member)
            redis.call('EXPIRE', key, window * 2)
            return 1
        else
            return 0
        end
        """
        
        result = await self.redis.eval(
            lua_script,
            1,
            self.key,
            now,
            window_start,
            self.rate,
            self.window
        )
        
        return result == 1
    
    async def current_usage(self) -> int:
        """Get current token usage"""
        now = time.time()
        window_start = now - self.window
        await self.redis.zremrangebyscore(self.key, 0, window_start)
        return await self.redis.zcard(self.key)
    
    async def reset(self):
        """Reset rate limiter"""
        await self.redis.delete(self.key)


class RateLimiter:
    """Manages multiple token buckets for different agents"""
    
    def __init__(self):
        self.redis = redis.Redis(
            host="redis",
            port=6379,
            decode_responses=True
        )
        self.buckets: Dict[str, TokenBucket] = {}
    
    async def add_agent(self, agent_name: str, rate: int, window: int = 1):
        """Add rate limit configuration for an agent"""
        self.buckets[agent_name] = TokenBucket(
            self.redis,
            f"agent:{agent_name}",
            rate,
            window
        )
    
    async def acquire(self, agent_name: str) -> bool:
        """Check if request is allowed for this agent"""
        if agent_name not in self.buckets:
            return True
        return await self.buckets[agent_name].acquire()
    
    async def get_current_usage(self, agent_name: str) -> int:
        """Get current usage for an agent"""
        if agent_name not in self.buckets:
            return 0
        return await self.buckets[agent_name].current_usage()
    
    async def update_limit(self, agent_name: str, new_rate: int, window: int = 1):
        """Update rate limit dynamically"""
        self.buckets[agent_name] = TokenBucket(
            self.redis,
            f"agent:{agent_name}",
            new_rate,
            window
        )
    
    async def get_stats(self) -> Dict[str, Any]:
        """Get rate limiting statistics"""
        stats = {}
        for agent_name, bucket in self.buckets.items():
            usage = await bucket.current_usage()
            stats[agent_name] = {
                "rate_limit": bucket.rate,
                "window_seconds": bucket.window,
                "current_usage": usage,
                "remaining": max(bucket.rate - usage, 0),
                "utilization_percent": round((usage / bucket.rate) * 100, 2) if bucket.rate > 0 else 0
            }
        return stats

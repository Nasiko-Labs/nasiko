import redis.asyncio as redis
import json
import asyncio
from typing import Optional, Dict, Any, Callable
from datetime import datetime


class PriorityQueue:
    """Priority queue implementation using Redis sorted sets"""
    
    def __init__(self, redis_client, queue_name: str):
        self.redis = redis_client
        self.queue_name = f"queue:{queue_name}"
    
    async def enqueue(self, item: Dict, priority: int = 1) -> Dict:
        """Add item to queue with priority (lower = higher priority)"""
        
        timestamp = datetime.utcnow().timestamp()
        score = priority * 1000000 + timestamp
        
        item_with_meta = {
            **item,
            "enqueued_at": datetime.utcnow().isoformat(),
            "priority": priority
        }
        
        await self.redis.zadd(
            self.queue_name,
            {json.dumps(item_with_meta): score}
        )
        
        position = await self.redis.zrank(self.queue_name, json.dumps(item_with_meta))
        
        return {
            "position": position + 1 if position is not None else 0,
            "queue_size": await self.size()
        }
    
    async def dequeue(self) -> Optional[Dict]:
        """Get highest priority item"""
        
        items = await self.redis.zpopmin(self.queue_name, 1)
        
        if not items:
            return None
        
        item_data = items[0][0]
        return json.loads(item_data)
    
    async def size(self) -> int:
        """Get queue size"""
        return await self.redis.zcard(self.queue_name)


class QueueManager:
    """Manages queued requests and processes them in background"""
    
    def __init__(self):
        self.redis = redis.Redis(
            host="redis",
            port=6379,
            decode_responses=True
        )
        self.queue = PriorityQueue(self.redis, "request_queue")
        self.results = {}
        
        # Stats
        self.queued_count = 0
        self.processed_count = 0
        self.timeout_count = 0
    
    async def enqueue(
        self,
        query: str,
        agent_hint: Optional[str],
        priority: int = 1,
        pending_key: Optional[str] = None
    ) -> Dict:
        """Add request to queue"""
        
        self.queued_count += 1
        
        return await self.queue.enqueue(
            item={
                "query": query,
                "agent_hint": agent_hint,
                "pending_key": pending_key,
                "priority": priority
            },
            priority=priority
        )
    
    async def process_queue(self, processor: Callable):
        """Background worker processing queue"""
        
        while True:
            try:
                await self.process_next(processor)
                await asyncio.sleep(0.1)
            except Exception as e:
                print(f"Queue processing error: {e}")
                await asyncio.sleep(1)
    
    async def process_next(self, processor: Callable):
        """Process next item in queue"""
        
        item = await self.queue.dequeue()
        
        if not item:
            return
        
        try:
            result = await processor(
                item["query"],
                item.get("agent_hint")
            )
            
            if item.get("pending_key"):
                self.results[item["pending_key"]] = result
                
                await self.redis.setex(
                    f"coalesce:result:{item['pending_key']}",
                    30,
                    json.dumps(result)
                )
            
            self.processed_count += 1
            
        except Exception as e:
            if item.get("pending_key"):
                self.results[item["pending_key"]] = {"error": str(e)}
    
    async def wait_for_processing(
        self,
        pending_key: str,
        timeout: float = 30.0
    ) -> Optional[Dict]:
        """Wait for queued request to be processed"""
        
        start = datetime.utcnow()
        
        while (datetime.utcnow() - start).total_seconds() < timeout:
            if pending_key in self.results:
                return self.results.pop(pending_key)
            
            result = await self.redis.get(f"coalesce:result:{pending_key}")
            if result:
                return json.loads(result)
            
            await asyncio.sleep(0.5)
        
        self.timeout_count += 1
        return None
    
    async def get_stats(self) -> Dict[str, Any]:
        """Get queue statistics"""
        
        return {
            "queued_total": self.queued_count,
            "processed_total": self.processed_count,
            "timeouts": self.timeout_count,
            "current_queue_size": await self.queue.size(),
            "success_rate": round(
                (self.processed_count / max(self.queued_count, 1)) * 100,
                2
            )
        }

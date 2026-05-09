import redis
import time
from typing import Dict, Any

class QueueManager:
    def __init__(self, redis_host: str = "redis", redis_port: int = 6379):
        self.redis = redis.Redis(
            host=redis_host,
            port=redis_port,
            decode_responses=True,
            socket_connect_timeout=5,
            socket_timeout=5
        )

    def enqueue(self, queue_name: str, payload: str) -> Dict[str, Any]:
        key = f"queue:{queue_name}"
        position = self.redis.rpush(key, payload)
        return {"queued": True, "queue": queue_name, "position": position}

    def dequeue(self, queue_name: str) -> Dict[str, Any]:
        key = f"queue:{queue_name}"
        item = self.redis.lpop(key)
        return {"dequeued": bool(item), "queue": queue_name, "payload": item}

    def length(self, queue_name: str) -> Dict[str, Any]:
        key = f"queue:{queue_name}"
        return {"queue": queue_name, "length": self.redis.llen(key)}

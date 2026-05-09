import time
import hashlib
import asyncio
from collections import defaultdict, deque


class TrafficController:
    def __init__(self):
        self.cache = {}
        self.cache_ttl = 300

        self.rate_limits = defaultdict(lambda: {
            "limit": 5,
            "window": 10,
            "requests": deque()
        })

        self.queues = defaultdict(deque)

        self.metrics = {
            "cache_hits": 0,
            "cache_misses": 0,
            "queued_requests": 0,
            "processed_requests": 0,
            "rate_limited": 0,
        }

    def make_cache_key(self, query: str):
        return hashlib.sha256(query.encode()).hexdigest()

    def get_cached(self, query: str):
        key = self.make_cache_key(query)

        if key in self.cache:
            data, ts = self.cache[key]
            if time.time() - ts < self.cache_ttl:
                self.metrics["cache_hits"] += 1
                return data

            del self.cache[key]

        self.metrics["cache_misses"] += 1
        return None

    def set_cache(self, query: str, response: str):
        key = self.make_cache_key(query)
        self.cache[key] = (response, time.time())

    def allow_request(self, agent_name: str):
        limiter = self.rate_limits[agent_name]
        now = time.time()

        while limiter["requests"] and now - limiter["requests"][0] > limiter["window"]:
            limiter["requests"].popleft()

        if len(limiter["requests"]) >= limiter["limit"]:
            self.metrics["rate_limited"] += 1
            return False

        limiter["requests"].append(now)
        return True

    def queue_request(self, agent_name: str, request):
        self.queues[agent_name].append(request)
        self.metrics["queued_requests"] += 1

    def queue_size(self, agent_name: str):
        return len(self.queues[agent_name])

    def stats(self):
        return {
            "metrics": self.metrics,
            "cache_entries": len(self.cache),
            "active_queues": {
                agent: len(q) for agent, q in self.queues.items()
            }
        }


traffic_controller = TrafficController()
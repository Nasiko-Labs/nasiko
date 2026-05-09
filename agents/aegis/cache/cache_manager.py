import asyncio
import hashlib
import json
import time

CACHE_TTL = 300  # 5 minutes

class CacheManager:
    def __init__(self):
        self.cache = {}
        self.pending = {}

        self.stats = {
            "hits": 0,
            "misses": 0,
            "deduped": 0,
        }

    def generate_key(self, agent_name, payload):
        raw = json.dumps({
            "agent": agent_name,
            "payload": payload
        }, sort_keys=True)

        return hashlib.sha256(raw.encode()).hexdigest()

    def get_cached(self, key):
        if key not in self.cache:
            self.stats["misses"] += 1
            return None

        item = self.cache[key]

        if time.time() > item["expiry"]:
            del self.cache[key]
            self.stats["misses"] += 1
            return None

        self.stats["hits"] += 1
        return item["response"]

    def set_cache(self, key, response):
        self.cache[key] = {
            "response": response,
            "expiry": time.time() + CACHE_TTL
        }

    async def execute(self, agent_name, payload, execution_fn):

        key = self.generate_key(agent_name, payload)

        # CACHE HIT
        cached = self.get_cached(key)

        if cached is not None:
            return {
                "type": "cache_hit",
                "data": cached
            }

        # REQUEST COALESCING
        if key in self.pending:
            self.stats["deduped"] += 1

            result = await self.pending[key]

            return {
                "type": "deduped",
                "data": result
            }

        # FIRST REQUEST
        future = asyncio.create_task(execution_fn())

        self.pending[key] = future

        try:
            result = await future

            self.set_cache(key, result)

            return {
                "type": "executed",
                "data": result
            }

        finally:
            del self.pending[key]


cache_manager = CacheManager()
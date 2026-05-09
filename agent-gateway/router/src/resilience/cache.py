from __future__ import annotations

import hashlib
import json
import re
import time

from router.src.resilience.models import CacheConfig, CacheEntry, CacheLookup, CacheStats


_WHITESPACE_RE = re.compile(r"\s+")
_PUNCTUATION_RE = re.compile(r"[^\w\s-]")


def normalize_query(query: str) -> str:
    without_punctuation = _PUNCTUATION_RE.sub("", query.strip().lower())
    return _WHITESPACE_RE.sub(" ", without_punctuation)


class SemanticResponseCache:
    """Safe response cache for selected-agent results.

    The always-on implementation is deterministic exact-normalized caching.
    Semantic/vector lookup can be layered behind this API without changing the
    router integration.
    """

    def __init__(self, config: CacheConfig | None = None):
        self.config = config or CacheConfig()
        self.stats = CacheStats()
        self._entries: dict[str, CacheEntry] = {}
        self._redis = self._connect_redis(self.config.redis_url)

    def get(self, lookup: CacheLookup) -> str | None:
        if not self._is_cacheable(lookup):
            self.stats.misses += 1
            return None

        key = self._key_for(lookup)
        redis_response = self._get_from_redis(key)
        if redis_response is not None:
            self.stats.hits += 1
            return redis_response

        entry = self._entries.get(key)
        if entry is None:
            self.stats.misses += 1
            return None

        if entry.expires_at <= time.monotonic():
            self._entries.pop(key, None)
            self.stats.evictions += 1
            self.stats.misses += 1
            return None

        self.stats.hits += 1
        return entry.response

    def set(self, lookup: CacheLookup, response: str) -> None:
        if not self._is_cacheable(lookup):
            return

        key = self._key_for(lookup)
        if self._set_in_redis(key, lookup.agent_id, response):
            self.stats.stores += 1
            return

        self._entries[key] = CacheEntry(
            key=key,
            agent_id=lookup.agent_id,
            response=response,
            expires_at=time.monotonic() + self.config.ttl_seconds,
        )
        self.stats.stores += 1

    def clear(self, agent_id: str | None = None) -> int:
        redis_deleted = self._clear_redis(agent_id)
        if redis_deleted is not None:
            return redis_deleted

        if agent_id is None:
            count = len(self._entries)
            self._entries.clear()
            return count

        keys = [
            key for key, entry in self._entries.items() if entry.agent_id == agent_id
        ]
        for key in keys:
            self._entries.pop(key, None)
        return len(keys)

    def update_config(self, **updates: object) -> CacheConfig:
        data = self.config.model_dump()
        data.update({key: value for key, value in updates.items() if value is not None})
        self.config = CacheConfig(**data)
        self._redis = self._connect_redis(self.config.redis_url)
        return self.config

    def _is_cacheable(self, lookup: CacheLookup) -> bool:
        return (
            self.config.enabled
            and not lookup.has_files
            and bool(lookup.agent_id.strip())
            and bool(lookup.query.strip())
            and bool(lookup.auth_scope.strip())
            and self.config.ttl_seconds > 0
        )

    def _key_for(self, lookup: CacheLookup) -> str:
        raw = "|".join(
            [
                self.config.namespace,
                lookup.agent_id.strip(),
                normalize_query(lookup.query),
                lookup.auth_scope.strip(),
                (lookup.route or "").strip(),
            ]
        )
        return hashlib.sha256(raw.encode("utf-8")).hexdigest()

    def _redis_key(self, key: str) -> str:
        return f"{self.config.namespace}:response:{key}"

    def _redis_agent_set_key(self, agent_id: str) -> str:
        agent_hash = hashlib.sha256(agent_id.encode("utf-8")).hexdigest()
        return f"{self.config.namespace}:agent:{agent_hash}"

    def _connect_redis(self, redis_url: str | None):
        if not redis_url:
            return None
        try:
            import redis

            client = redis.Redis.from_url(
                redis_url, decode_responses=True, socket_connect_timeout=0.2
            )
            client.ping()
            return client
        except Exception:
            self.stats.errors += 1
            return None

    def _get_from_redis(self, key: str) -> str | None:
        if self._redis is None:
            return None
        try:
            raw = self._redis.get(self._redis_key(key))
            if not raw:
                return None
            payload = json.loads(raw)
            return payload.get("response")
        except Exception:
            self.stats.errors += 1
            return None

    def _set_in_redis(self, key: str, agent_id: str, response: str) -> bool:
        if self._redis is None:
            return False
        try:
            redis_key = self._redis_key(key)
            payload = json.dumps({"agent_id": agent_id, "response": response})
            ttl = max(1, int(self.config.ttl_seconds))
            pipe = self._redis.pipeline(transaction=True)
            pipe.setex(redis_key, ttl, payload)
            pipe.sadd(self._redis_agent_set_key(agent_id), redis_key)
            pipe.expire(self._redis_agent_set_key(agent_id), ttl)
            pipe.execute()
            return True
        except Exception:
            self.stats.errors += 1
            return False

    def _clear_redis(self, agent_id: str | None) -> int | None:
        if self._redis is None:
            return None
        try:
            if agent_id is not None:
                set_key = self._redis_agent_set_key(agent_id)
                keys = list(self._redis.smembers(set_key))
                if keys:
                    self._redis.delete(*keys)
                self._redis.delete(set_key)
                return len(keys)

            pattern = f"{self.config.namespace}:*"
            keys = list(self._redis.scan_iter(match=pattern))
            if keys:
                self._redis.delete(*keys)
            return len(keys)
        except Exception:
            self.stats.errors += 1
            return None

"""
Two-tier caching layer for Sentinel Guard.

L1 — Redis exact-match (sub-millisecond)
L2 — MemPalace semantic search (< 50 ms)
"""

import hashlib
import json
import logging
import time
from typing import Any, Optional

import redis

from app.config import config
from app.store import (
    CacheEntry,
    Decision,
    cache_hits,
    cache_misses,
    cache_semantic_hits,
    increment_counter,
    record_decision,
)

logger = logging.getLogger("sentinel.cache")


class CacheLayer:
    """Two-tier caching: Redis (exact) + semantic (MemPalace/sentence-transformers)."""

    def __init__(self) -> None:
        # ── Redis L1 cache ─────────────────────────────────────────────────
        try:
            self._redis = redis.Redis(
                host=config.REDIS_HOST,
                port=config.REDIS_PORT,
                db=config.REDIS_DB,
                decode_responses=True,
                socket_connect_timeout=5,
            )
            self._redis.ping()
            self._redis_ok = True
            logger.info("Redis L1 cache connected")
        except Exception as exc:
            logger.warning(f"Redis unavailable – falling back to in-memory only: {exc}")
            self._redis_ok = False
            self._redis = None

        # ── Semantic L2 cache (sentence-transformers) ──────────────────────
        self._semantic_cache: dict[str, list[CacheEntry]] = {}
        self._model = None
        try:
            from sentence_transformers import SentenceTransformer

            self._model = SentenceTransformer(config.EMBEDDING_MODEL)
            logger.info(f"Loaded embedding model: {config.EMBEDDING_MODEL}")
        except Exception as exc:
            logger.warning(f"sentence-transformers unavailable: {exc}")

        # ── MemPalace L2+ integration ──────────────────────────────────────
        self._mempalace = None
        try:
            from app.mempalace_adapter import MemPalaceAdapter

            self._mempalace = MemPalaceAdapter()
            logger.info("MemPalace adapter initialised")
        except Exception as exc:
            logger.warning(f"MemPalace unavailable – semantic cache will use local embeddings only: {exc}")

    # ── Public API ─────────────────────────────────────────────────────────

    def lookup(self, query: str, agent: str) -> Optional[dict[str, Any]]:
        """
        Try to find a cached response. Returns the cached payload or None.
        Order: Redis exact → local semantic → MemPalace semantic.
        """
        start = time.time()

        # L1: Redis exact match
        result = self._redis_get(query, agent)
        if result is not None:
            latency = (time.time() - start) * 1000
            increment_counter(cache_hits, agent)
            record_decision(Decision(
                timestamp=time.time(), agent=agent, query=query,
                outcome="cache_hit_exact", latency_ms=latency, source="redis",
            ))
            logger.info(f"[L1 HIT] agent={agent} latency={latency:.1f}ms")
            return result

        # L2: Local semantic match
        result = self._semantic_lookup(query, agent)
        if result is not None:
            latency = (time.time() - start) * 1000
            increment_counter(cache_hits, agent)
            increment_counter(cache_semantic_hits, agent)
            record_decision(Decision(
                timestamp=time.time(), agent=agent, query=query,
                outcome="cache_hit_semantic", similarity=result.get("_similarity"),
                latency_ms=latency, source="semantic",
            ))
            logger.info(f"[L2 HIT] agent={agent} sim={result.get('_similarity', 0):.3f} latency={latency:.1f}ms")
            return result

        # L3: MemPalace deep semantic search
        if self._mempalace:
            result = self._mempalace_lookup(query, agent)
            if result is not None:
                latency = (time.time() - start) * 1000
                increment_counter(cache_hits, agent)
                increment_counter(cache_semantic_hits, agent)
                record_decision(Decision(
                    timestamp=time.time(), agent=agent, query=query,
                    outcome="cache_hit_semantic", similarity=result.get("_similarity"),
                    latency_ms=latency, source="mempalace",
                ))
                logger.info(f"[L3 HIT] agent={agent} MemPalace latency={latency:.1f}ms")
                return result

        # Cache miss
        latency = (time.time() - start) * 1000
        increment_counter(cache_misses, agent)
        record_decision(Decision(
            timestamp=time.time(), agent=agent, query=query,
            outcome="cache_miss", latency_ms=latency,
        ))
        return None

    def store(self, query: str, response: Any, agent: str) -> None:
        """Store a response in all cache tiers."""
        try:
            # L1: Redis exact match
            self._redis_set(query, response, agent)

            # L2: Local semantic index
            self._semantic_store(query, response, agent)

            # L3: MemPalace
            if self._mempalace:
                self._mempalace_store(query, response, agent)

        except Exception as exc:
            logger.error(f"Cache store error: {exc}")

    def flush(self, agent: Optional[str] = None) -> dict[str, int]:
        """Flush cache entries. If agent given, flush only that agent's cache."""
        flushed = {"redis": 0, "semantic": 0, "mempalace": 0}

        # Redis
        if self._redis_ok and self._redis:
            try:
                if agent:
                    pattern = f"sentinel:cache:{agent}:*"
                    keys = list(self._redis.scan_iter(pattern, count=500))
                    if keys:
                        flushed["redis"] = self._redis.delete(*keys)
                else:
                    keys = list(self._redis.scan_iter("sentinel:cache:*", count=5000))
                    if keys:
                        flushed["redis"] = self._redis.delete(*keys)
            except Exception as exc:
                logger.warning(f"Redis flush error: {exc}")

        # Local semantic
        if agent:
            flushed["semantic"] = len(self._semantic_cache.pop(agent, []))
        else:
            flushed["semantic"] = sum(len(v) for v in self._semantic_cache.values())
            self._semantic_cache.clear()

        # MemPalace
        if self._mempalace:
            try:
                flushed["mempalace"] = self._mempalace.flush(agent)
            except Exception:
                pass

        logger.info(f"Cache flushed: {flushed} (agent={agent})")
        return flushed

    def stats(self) -> dict[str, Any]:
        """Return cache statistics."""
        redis_size = 0
        if self._redis_ok and self._redis:
            try:
                redis_size = len(list(self._redis.scan_iter("sentinel:cache:*", count=5000)))
            except Exception:
                pass

        semantic_size = sum(len(v) for v in self._semantic_cache.values())

        return {
            "redis_entries": redis_size,
            "semantic_entries": semantic_size,
            "mempalace_available": self._mempalace is not None,
            "embedding_model": config.EMBEDDING_MODEL if self._model else None,
            "ttl_seconds": config.CACHE_TTL_SECONDS,
            "similarity_threshold": config.SIMILARITY_THRESHOLD,
        }

    # ── Redis L1 ───────────────────────────────────────────────────────────

    def _cache_key(self, query: str, agent: str) -> str:
        """Deterministic cache key from query + agent."""
        raw = f"{agent}:{query.strip().lower()}"
        return f"sentinel:cache:{agent}:{hashlib.sha256(raw.encode()).hexdigest()[:24]}"

    def _redis_get(self, query: str, agent: str) -> Optional[dict]:
        if not self._redis_ok or not self._redis:
            return None
        try:
            key = self._cache_key(query, agent)
            raw = self._redis.get(key)
            if raw:
                data = json.loads(raw)
                # Bump hit counter
                self._redis.hincrby(f"{key}:meta", "hits", 1)
                return data
        except Exception as exc:
            logger.debug(f"Redis GET error: {exc}")
        return None

    def _redis_set(self, query: str, response: Any, agent: str) -> None:
        if not self._redis_ok or not self._redis:
            return
        try:
            key = self._cache_key(query, agent)
            payload = json.dumps(response) if not isinstance(response, str) else response
            self._redis.setex(key, config.CACHE_TTL_SECONDS, payload)
            self._redis.hset(f"{key}:meta", mapping={
                "agent": agent,
                "query": query[:200],
                "created_at": str(time.time()),
                "hits": "0",
            })
            self._redis.expire(f"{key}:meta", config.CACHE_TTL_SECONDS)
        except Exception as exc:
            logger.debug(f"Redis SET error: {exc}")

    # ── Semantic L2 ────────────────────────────────────────────────────────

    def _encode(self, text: str) -> Optional[list[float]]:
        """Encode text to embedding vector."""
        if self._model is None:
            return None
        try:
            return self._model.encode(text, normalize_embeddings=True).tolist()
        except Exception:
            return None

    def _cosine_similarity(self, a: list[float], b: list[float]) -> float:
        """Compute cosine similarity between two normalised vectors."""
        import numpy as np

        a_arr, b_arr = np.array(a), np.array(b)
        dot = np.dot(a_arr, b_arr)
        norm = np.linalg.norm(a_arr) * np.linalg.norm(b_arr)
        return float(dot / norm) if norm > 0 else 0.0

    def _semantic_lookup(self, query: str, agent: str) -> Optional[dict]:
        """Search local semantic cache for a similar query."""
        if self._model is None:
            return None

        entries = self._semantic_cache.get(agent, [])
        if not entries:
            return None

        query_vec = self._encode(query)
        if query_vec is None:
            return None

        best_entry: Optional[CacheEntry] = None
        best_sim = 0.0

        for entry in entries:
            if hasattr(entry, "_vector") and entry._vector:
                sim = self._cosine_similarity(query_vec, entry._vector)
                if sim > best_sim:
                    best_sim = sim
                    best_entry = entry

        if best_entry and best_sim >= config.SIMILARITY_THRESHOLD:
            best_entry.hits += 1
            best_entry.last_hit = time.time()
            result = best_entry.response if isinstance(best_entry.response, dict) else {"result": best_entry.response}
            result["_similarity"] = best_sim
            result["_cache_source"] = "semantic"
            return result

        return None

    def _semantic_store(self, query: str, response: Any, agent: str) -> None:
        """Store a response in the local semantic cache."""
        if self._model is None:
            return

        if agent not in self._semantic_cache:
            self._semantic_cache[agent] = []

        # Encode the query
        vec = self._encode(query)
        if vec is None:
            return

        entry = CacheEntry(
            query=query,
            response=response,
            agent=agent,
            cache_key=self._cache_key(query, agent),
            ttl_seconds=config.CACHE_TTL_SECONDS,
        )
        # Attach vector (not part of dataclass to keep it clean)
        entry._vector = vec  # type: ignore[attr-defined]

        self._semantic_cache[agent].append(entry)

        # Evict oldest if over limit
        if len(self._semantic_cache[agent]) > config.MAX_CACHE_SIZE_PER_AGENT:
            self._semantic_cache[agent] = sorted(
                self._semantic_cache[agent], key=lambda e: e.last_hit, reverse=True
            )[:config.MAX_CACHE_SIZE_PER_AGENT]

    # ── MemPalace L3 ───────────────────────────────────────────────────────

    def _mempalace_lookup(self, query: str, agent: str) -> Optional[dict]:
        """Search MemPalace for a semantically similar cached response."""
        if not self._mempalace:
            return None
        try:
            return self._mempalace.search(query, agent, config.SIMILARITY_THRESHOLD)
        except Exception as exc:
            logger.debug(f"MemPalace lookup error: {exc}")
            return None

    def _mempalace_store(self, query: str, response: Any, agent: str) -> None:
        """Store a query-response pair in MemPalace."""
        if not self._mempalace:
            return
        try:
            self._mempalace.store(query, response, agent)
        except Exception as exc:
            logger.debug(f"MemPalace store error: {exc}")

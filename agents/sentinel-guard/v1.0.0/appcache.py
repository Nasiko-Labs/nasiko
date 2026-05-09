import time

import numpy as np
from sentence_transformers import SentenceTransformer
from typing import Any

from app.store import cache_store, cache_hits, cache_misses, CacheEntry

# ── Embedding model (loaded once at import time) ───────────────────
_model = SentenceTransformer("all-MiniLM-L6-v2")
_thresholds: dict[str, float] = {}
DEFAULT_THRESHOLD = 0.92


# ── Threshold helpers ──────────────────────────────────────────────
def set_threshold(agent: str, threshold: float):
    _thresholds[agent] = threshold


def get_threshold(agent: str) -> float:
    return _thresholds.get(agent, DEFAULT_THRESHOLD)


# ── Embedding + similarity ─────────────────────────────────────────
def _embed(text: str) -> np.ndarray:
    return _model.encode(text, normalize_embeddings=True)


def _cosine(a: np.ndarray, b: list[float]) -> float:
    return float(np.dot(a, np.array(b, dtype=np.float32)))


# ── Cache operations ───────────────────────────────────────────────
def lookup(agent: str, query: str) -> tuple[Any | None, float]:
    """Return (cached_response, similarity_score) or (None, best_score)."""
    if agent not in cache_store or not cache_store[agent]:
        _bump_miss(agent)
        return None, 0.0

    vec = _embed(query)
    threshold = get_threshold(agent)
    best_score, best_entry = 0.0, None

    for entry in cache_store[agent]:
        score = _cosine(vec, entry.vector)
        if score > best_score:
            best_score, best_entry = score, entry

    if best_score >= threshold and best_entry:
        best_entry.hits += 1
        best_entry.last_hit = time.time()
        _bump_hit(agent)
        return best_entry.response, best_score

    _bump_miss(agent)
    return None, best_score


def store(agent: str, query: str, response: Any):
    """Embed and store a new cache entry."""
    vec = _embed(query)
    entry = CacheEntry(
        query=query,
        vector=vec.tolist(),
        response=response,
        agent=agent,
    )
    if agent not in cache_store:
        cache_store[agent] = []
    cache_store[agent].append(entry)


def flush(agent: str) -> int:
    """Flush cache for a single agent, return count of cleared entries."""
    count = len(cache_store.get(agent, []))
    cache_store[agent] = []
    cache_hits[agent] = 0
    cache_misses[agent] = 0
    return count


def flush_all() -> int:
    """Flush all caches, return total cleared entries."""
    total = sum(len(v) for v in cache_store.values())
    cache_store.clear()
    cache_hits.clear()
    cache_misses.clear()
    return total


def get_stats(agent: str | None = None) -> dict:
    """Return cache statistics for one agent or all agents."""
    if agent:
        entries = cache_store.get(agent, [])
        hits = cache_hits.get(agent, 0)
        misses = cache_misses.get(agent, 0)
        total = hits + misses
        return {
            "agent": agent,
            "entries": len(entries),
            "hits": hits,
            "misses": misses,
            "hit_rate": round(hits / total, 4) if total > 0 else 0.0,
            "threshold": get_threshold(agent),
        }
    all_agents = set(list(cache_store.keys()) + list(cache_hits.keys()))
    return {ag: get_stats(ag) for ag in all_agents}


# ── Internal counters ──────────────────────────────────────────────
def _bump_hit(agent: str):
    cache_hits[agent] = cache_hits.get(agent, 0) + 1


def _bump_miss(agent: str):
    cache_misses[agent] = cache_misses.get(agent, 0) + 1

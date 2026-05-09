import hashlib
import json
import uuid
from typing import Optional, Tuple

import numpy as np
import redis.asyncio as redis

from . import stats

# Lazy-loaded model reference
_model = None
_dimension = None

def _get_model():
    """Load the sentence transformer model once (lazy singleton)."""
    global _model, _dimension
    if _model is None:
        from sentence_transformers import SentenceTransformer
        print("ARIA: Loading sentence-transformers model (one-time)...")
        _model = SentenceTransformer('all-MiniLM-L6-v2')
        try:
            _dimension = _model.get_embedding_dimension()
        except AttributeError:
            _dimension = _model.get_sentence_embedding_dimension()
        print(f"ARIA: Model loaded. Embedding dimension: {_dimension}")
    return _model, _dimension


async def get_exact_cache(
    redis_client: redis.Redis, agent_name: str, request_body: bytes
) -> Optional[dict]:
    """Checks for an exact match in the Redis cache."""
    key = f"exact:{agent_name}:{hashlib.md5(request_body).hexdigest()}"
    cached_response = await redis_client.get(key)
    if cached_response:
        stats.stats["exact_hits"] += 1
        stats.stats["per_agent_exact_hits"][agent_name] += 1
        stats.stats["per_agent_requests"][agent_name] += 1
        stats.record_request(agent_name)
        return json.loads(cached_response)
    return None


async def get_semantic_cache(
    redis_client: redis.Redis,
    agent_name: str,
    query: str,
    similarity_threshold: float = 0.92,
) -> Optional[Tuple[dict, float]]:
    """Finds a semantically similar request in the cache."""
    if not query or not query.strip():
        return None

    model, dimension = _get_model()
    query_embedding = model.encode(query, convert_to_numpy=True).astype(np.float32)

    # Fetch all cached vector keys for the agent
    vector_keys = []
    async for key in redis_client.scan_iter(f"semvec:{agent_name}:*"):
        vector_keys.append(key)

    if not vector_keys:
        return None

    # Fetch all vectors individually to handle expired keys
    best_similarity = -1.0
    best_key_idx = -1

    for idx, vk in enumerate(vector_keys):
        vec_bytes = await redis_client.get(vk)
        if vec_bytes is None:
            continue
        cached_vec = np.frombuffer(vec_bytes, dtype=np.float32)
        if cached_vec.shape[0] != dimension:
            continue
        # Cosine similarity
        dot = np.dot(cached_vec, query_embedding)
        norm = np.linalg.norm(cached_vec) * np.linalg.norm(query_embedding)
        if norm == 0:
            continue
        similarity = float(dot / norm)
        if similarity > best_similarity:
            best_similarity = similarity
            best_key_idx = idx

    if best_key_idx >= 0 and best_similarity >= similarity_threshold:
        # Extract UID from key like b"semvec:agent:uid"
        key_str = vector_keys[best_key_idx]
        if isinstance(key_str, bytes):
            key_str = key_str.decode()
        response_uid = key_str.split(":")[-1]
        response_key = f"semresp:{agent_name}:{response_uid}"
        cached_response = await redis_client.get(response_key)
        if cached_response:
            stats.stats["semantic_hits"] += 1
            stats.stats["per_agent_semantic_hits"][agent_name] += 1
            stats.stats["per_agent_requests"][agent_name] += 1
            stats.record_request(agent_name)
            return json.loads(cached_response), best_similarity

    return None


async def set_cache(
    redis_client: redis.Redis,
    agent_name: str,
    request_body: bytes,
    query: str,
    response: dict,
):
    """Stores a response in both exact and semantic caches."""
    response_json = json.dumps(response)

    # Exact cache
    exact_key = f"exact:{agent_name}:{hashlib.md5(request_body).hexdigest()}"
    await redis_client.set(exact_key, response_json, ex=300)  # 5-minute TTL

    # Semantic cache (only if query is non-empty)
    if query and query.strip():
        model, _ = _get_model()
        uid = str(uuid.uuid4())
        embedding = model.encode(query, convert_to_numpy=True).astype(np.float32)
        vector_key = f"semvec:{agent_name}:{uid}"
        response_key = f"semresp:{agent_name}:{uid}"

        async with redis_client.pipeline() as pipe:
            pipe.set(vector_key, embedding.tobytes(), ex=300)
            pipe.set(response_key, response_json, ex=300)
            await pipe.execute()


async def clear_agent_cache(redis_client: redis.Redis, agent_name: str):
    """Deletes all cache entries for a specific agent."""
    patterns = [
        f"exact:{agent_name}:*",
        f"semvec:{agent_name}:*",
        f"semresp:{agent_name}:*",
    ]
    for pattern in patterns:
        keys = []
        async for key in redis_client.scan_iter(pattern):
            keys.append(key)
        if keys:
            await redis_client.delete(*keys)

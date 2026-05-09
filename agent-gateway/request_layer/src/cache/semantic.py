"""Semantic similarity response cache (Redis HNSW)."""
import logging
from datetime import datetime, timezone

import numpy as np
from redis.asyncio import Redis
from redis.commands.search.field import TagField, TextField, VectorField
from redis.commands.search.indexDefinition import IndexDefinition, IndexType
from redis.commands.search.query import Query
from redis.exceptions import ResponseError

from request_layer.src.cache import exact as exact_cache
from request_layer.src.embedding import embed_one
from request_layer.src.types import CacheEntry

logger = logging.getLogger(__name__)

_INDEX_PREFIX = "request_layer:semantic"
_DOC_PREFIX = "request_layer:semdoc"


def _index_name(agent: str) -> str:
    return f"{_INDEX_PREFIX}:idx:{agent}"


def _doc_key(agent: str, query_hash: str) -> str:
    return f"{_DOC_PREFIX}:{agent}:{query_hash}"


def _vector_to_bytes(vector: list[float]) -> bytes:
    return np.asarray(vector, dtype=np.float32).tobytes()


async def ensure_index(redis: Redis, agent: str, dim: int) -> None:
    """Create the HNSW index for ``agent`` if it does not already exist.

    Idempotent: the FT.CREATE call is wrapped to ignore "Index already
    exists" responses, so this can be called from request handlers without
    a startup race.
    """

    name = _index_name(agent)
    try:
        await redis.ft(name).info()
        return  # index already exists
    except ResponseError:
        pass

    schema = (
        VectorField(
            "embedding",
            "HNSW",
            {
                "TYPE": "FLOAT32",
                "DIM": dim,
                "DISTANCE_METRIC": "COSINE",
                "M": 16,
                "EF_CONSTRUCTION": 200,
            },
        ),
        TextField("matched_query"),
        TagField("agent"),
    )
    definition = IndexDefinition(
        prefix=[f"{_DOC_PREFIX}:{agent}:"],
        index_type=IndexType.HASH,
    )
    try:
        await redis.ft(name).create_index(schema, definition=definition)
        logger.info("created semantic index %s (dim=%s)", name, dim)
    except ResponseError as exc:
        if "Index already exists" in str(exc):
            return
        raise


async def lookup(
    redis: Redis,
    agent: str,
    normalized_query: str,
    threshold: float,
) -> tuple[CacheEntry, float, str] | None:
    """Search the semantic index for ``normalized_query``.

    Returns:
        ``(cache_entry, similarity, original_matched_query)`` on hit,
        otherwise ``None``.
    """

    vector = await embed_one(normalized_query)
    return await lookup_with_vector(redis, agent, vector, threshold)


async def lookup_with_vector(
    redis: Redis,
    agent: str,
    vector: list[float],
    threshold: float,
) -> tuple[CacheEntry, float, str] | None:
    """Variant of :func:`lookup` for callers that already have an embedding."""

    name = _index_name(agent)
    blob = _vector_to_bytes(vector)
    query = (
        Query("*=>[KNN 1 @embedding $vec AS score]")
        .return_fields("score", "matched_query", "payload")
        .dialect(2)
    )
    try:
        result = await redis.ft(name).search(query, query_params={"vec": blob})
    except ResponseError as exc:
        if "no such index" in str(exc).lower():
            return None
        raise

    if not result.docs:
        return None

    doc = result.docs[0]
    # ``score`` from KNN with cosine is the cosine *distance*, i.e. 1 - sim.
    distance = float(getattr(doc, "score", 1.0))
    similarity = 1.0 - distance
    if similarity < threshold:
        return None

    payload = getattr(doc, "payload", None)
    if payload is None:
        return None
    try:
        entry = CacheEntry.model_validate_json(payload)
    except ValueError:
        logger.warning("dropping corrupt L2 entry at %s", doc.id)
        await redis.delete(doc.id)
        return None

    matched_query = getattr(doc, "matched_query", "") or ""
    return entry, similarity, matched_query


async def store(
    redis: Redis,
    agent: str,
    normalized_query: str,
    entry: CacheEntry,
    ttl_seconds: int,
) -> None:
    """Insert ``(embedding(normalized_query), entry)`` into the L2 index."""

    vector = await embed_one(normalized_query)
    await store_with_vector(redis, agent, normalized_query, vector, entry, ttl_seconds)


async def store_with_vector(
    redis: Redis,
    agent: str,
    normalized_query: str,
    vector: list[float],
    entry: CacheEntry,
    ttl_seconds: int,
) -> None:
    """Variant of :func:`store` for callers that already have an embedding."""

    query_hash = exact_cache.stable_hash(normalized_query)
    key = _doc_key(agent, query_hash)
    payload = entry.model_copy(
        update={
            "matched_query": normalized_query,
            "cached_at": datetime.now(timezone.utc),
        }
    )
    blob = _vector_to_bytes(vector)

    pipe = redis.pipeline()
    pipe.hset(
        key,
        mapping={
            "embedding": blob,
            "matched_query": normalized_query,
            "agent": agent,
            "payload": payload.model_dump_json(),
        },
    )
    pipe.expire(key, ttl_seconds)
    await pipe.execute()


async def clear(redis: Redis, agent: str | None = None) -> int:
    """Delete every L2 doc (optionally restricted to one agent)."""

    pattern = f"{_DOC_PREFIX}:{agent or '*'}:*"
    deleted = 0
    async for key in redis.scan_iter(match=pattern):
        await redis.delete(key)
        deleted += 1
    return deleted

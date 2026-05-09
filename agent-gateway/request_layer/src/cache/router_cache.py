"""Routing-decision cache (opt-in)."""
import hashlib
import json
import logging
from datetime import datetime, timezone
from typing import Iterable

import numpy as np
from redis.asyncio import Redis
from redis.commands.search.field import TextField, VectorField
from redis.commands.search.indexDefinition import IndexDefinition, IndexType
from redis.commands.search.query import Query
from redis.exceptions import ResponseError

from request_layer.src.embedding import embed_one
from request_layer.src.types import RoutingDecision

logger = logging.getLogger(__name__)

_INDEX_NAME = "request_layer:routing:idx"
_DOC_PREFIX = "request_layer:routing:doc"
_MANIFEST_HASH_KEY = "request_layer:routing:manifest_hash"


def _vector_to_bytes(vector: list[float]) -> bytes:
    return np.asarray(vector, dtype=np.float32).tobytes()


def compute_manifest_hash(manifests: Iterable[dict]) -> str:
    """Stable hash of an iterable of AgentCard manifests.

    Used to detect when the registered agent set has changed; on change the
    L3 cache is wiped to avoid serving stale routing decisions.
    """

    serialized = json.dumps(
        sorted(manifests, key=lambda m: m.get("name", "")),
        sort_keys=True,
        separators=(",", ":"),
    )
    return hashlib.sha256(serialized.encode("utf-8")).hexdigest()


async def ensure_index(redis: Redis, dim: int) -> None:
    """Create the cross-agent routing index if it does not exist."""

    try:
        await redis.ft(_INDEX_NAME).info()
        return
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
        TextField("decision_json"),
        TextField("matched_query"),
    )
    definition = IndexDefinition(
        prefix=[f"{_DOC_PREFIX}:"],
        index_type=IndexType.HASH,
    )
    try:
        await redis.ft(_INDEX_NAME).create_index(schema, definition=definition)
        logger.info("created L3 routing index (dim=%s)", dim)
    except ResponseError as exc:
        if "Index already exists" in str(exc):
            return
        raise


async def lookup(
    redis: Redis,
    normalized_query: str,
    threshold: float,
) -> RoutingDecision | None:
    """Return a cached routing decision if one matches above ``threshold``."""

    vector = await embed_one(normalized_query)
    return await lookup_with_vector(redis, vector, threshold)


async def lookup_with_vector(
    redis: Redis,
    vector: list[float],
    threshold: float,
) -> RoutingDecision | None:
    """Vector variant of :func:`lookup`."""

    blob = _vector_to_bytes(vector)
    query = (
        Query("*=>[KNN 1 @embedding $vec AS score]")
        .return_fields("score", "decision_json", "matched_query")
        .dialect(2)
    )
    try:
        result = await redis.ft(_INDEX_NAME).search(
            query, query_params={"vec": blob}
        )
    except ResponseError as exc:
        if "no such index" in str(exc).lower():
            return None
        raise

    if not result.docs:
        return None
    doc = result.docs[0]
    similarity = 1.0 - float(getattr(doc, "score", 1.0))
    if similarity < threshold:
        return None

    payload = getattr(doc, "decision_json", None)
    if not payload:
        return None
    try:
        decision = RoutingDecision.model_validate_json(payload)
    except ValueError:
        await redis.delete(doc.id)
        return None
    decision.matched_query = getattr(doc, "matched_query", None) or None
    return decision


async def store(
    redis: Redis,
    normalized_query: str,
    decision: RoutingDecision,
    ttl_seconds: int,
) -> None:
    """Cache a routing decision keyed by the embedding of ``normalized_query``."""

    vector = await embed_one(normalized_query)
    digest = hashlib.sha256(normalized_query.encode("utf-8")).hexdigest()
    key = f"{_DOC_PREFIX}:{digest}"

    decision = decision.model_copy(
        update={
            "cached_at": datetime.now(timezone.utc),
            "last_validated_at": datetime.now(timezone.utc),
        }
    )

    pipe = redis.pipeline()
    pipe.hset(
        key,
        mapping={
            "embedding": _vector_to_bytes(vector),
            "decision_json": decision.model_dump_json(),
            "matched_query": normalized_query,
        },
    )
    pipe.expire(key, ttl_seconds)
    await pipe.execute()


async def invalidate_all(redis: Redis) -> int:
    """Remove every cached routing decision. Returns count deleted."""

    deleted = 0
    async for key in redis.scan_iter(match=f"{_DOC_PREFIX}:*"):
        await redis.delete(key)
        deleted += 1
    return deleted


async def get_stored_manifest_hash(redis: Redis) -> str | None:
    raw = await redis.get(_MANIFEST_HASH_KEY)
    if isinstance(raw, bytes):
        return raw.decode("utf-8")
    return raw


async def set_stored_manifest_hash(redis: Redis, manifest_hash: str) -> None:
    await redis.set(_MANIFEST_HASH_KEY, manifest_hash)

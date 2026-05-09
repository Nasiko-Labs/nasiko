"""Per-agent cache and rate-limit policy."""
import json
import logging

from redis.asyncio import Redis

from request_layer.src.config import Settings
from request_layer.src.types import AgentManifest, Policy

logger = logging.getLogger(__name__)

_OVERRIDE_KEY = "request_layer:policy:overrides"


def infer_policy(manifest: AgentManifest, settings: Settings) -> Policy:
    """Derive a sensible default policy from an :class:`AgentManifest`.

    The heuristic groups capabilities and tags into buckets representing
    different volatility profiles, then assigns TTL and similarity
    threshold accordingly. Translation is treated as very stable, weather
    as very volatile, etc.
    """

    bag = {c.lower() for c in manifest.capabilities} | {
        t.lower() for t in manifest.tags
    }

    if bag & {"translation", "translate", "static_analysis", "summarization"}:
        ttl, threshold = 86400, 0.92
    elif bag & {"compliance", "policy_review", "code_review"}:
        ttl, threshold = 3600, 0.95
    elif bag & {"weather", "stock_price", "news", "realtime"}:
        ttl, threshold = 300, 0.97
    elif bag & {"search"}:
        ttl, threshold = 1800, 0.96
    else:
        ttl, threshold = settings.request_layer_default_ttl_seconds, settings.request_layer_semantic_threshold

    cost_cap = (
        5.0
        if "expensive" in bag
        else settings.request_layer_default_cost_cap_usd_per_min
    )

    return Policy(
        cache_ttl_seconds=ttl,
        semantic_threshold=threshold,
        rps_limit=settings.request_layer_default_rps,
        cost_cap_usd_per_min=cost_cap,
        notes="inferred from AgentCard capabilities and tags",
    )


async def get_override(redis: Redis, agent: str) -> dict | None:
    """Return the operator override for ``agent`` (if any)."""

    raw = await redis.hget(_OVERRIDE_KEY, agent)
    if raw is None:
        return None
    if isinstance(raw, bytes):
        raw = raw.decode("utf-8")
    try:
        return json.loads(raw)
    except json.JSONDecodeError:
        logger.warning("dropping corrupt override for %s", agent)
        await redis.hdel(_OVERRIDE_KEY, agent)
        return None


async def set_override(redis: Redis, agent: str, partial: dict) -> None:
    """Merge ``partial`` into the persisted override for ``agent``."""

    existing = await get_override(redis, agent) or {}
    existing.update(partial)
    await redis.hset(_OVERRIDE_KEY, agent, json.dumps(existing))


async def resolve(
    redis: Redis,
    inferred: dict[str, Policy],
    agent: str,
) -> Policy:
    """Return the effective policy: inferred default + persisted override."""

    base = inferred.get(agent)
    if base is None:
        # Unknown agent; return a conservative baseline.
        base = Policy(
            cache_ttl_seconds=600,
            semantic_threshold=0.95,
            rps_limit=50,
            cost_cap_usd_per_min=1.0,
            notes="default for unknown agent",
        )

    override = await get_override(redis, agent)
    if not override:
        return base

    merged = base.model_dump()
    for key, value in override.items():
        if key in merged:
            merged[key] = value
    return Policy(**merged)

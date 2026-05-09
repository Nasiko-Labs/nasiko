import copy
import hashlib
import json
import logging
import time
from typing import Optional

import redis.asyncio as aioredis

from src.config import settings

logger = logging.getLogger(__name__)

CACHE_NS = "rm:cache"
CACHE_STATS_NS = "rm:cstats"
REQ_LOG_NS = "rm:reqlog"
REQ_LOG_MAX = 100  # entries kept per agent


def _normalize_body(body: dict) -> str:
    """
    Canonical JSON string for cache keying.
    Strips volatile messageId (changes per UI call, doesn't affect agent answer).
    Sorts all keys for deterministic serialisation.
    """
    normalized = copy.deepcopy(body)
    normalized.pop("id", None)  # JSON-RPC request id is volatile
    params = normalized.get("params", {})
    message = params.get("message", {})
    message.pop("messageId", None)
    for part in message.get("parts", []):
        if "text" in part:
            part["text"] = part["text"].strip().lower()
    return json.dumps(normalized, sort_keys=True, ensure_ascii=False)


def build_cache_key(agent_name: str, body: dict) -> str:
    canonical = _normalize_body(body)
    digest = hashlib.sha256(f"{agent_name}:{canonical}".encode()).hexdigest()
    return f"{CACHE_NS}:{agent_name}:{digest}"


def extract_query_text(body: dict) -> str:
    """Pull the human-readable query out of a JSON-RPC message/send body."""
    try:
        parts = body.get("params", {}).get("message", {}).get("parts", [])
        return " ".join(p["text"] for p in parts if "text" in p).strip()
    except Exception:
        return ""


def _parts_text(parts: list) -> str:
    """Flatten a parts array to a single lowercase string for comparison."""
    return " ".join(
        p.get("text", "") for p in parts if isinstance(p, dict) and "text" in p
    ).strip().lower()


def deduplicate_history(body: dict) -> dict:
    """
    Scan known history fields in the request body and remove earlier duplicate
    (user_query, assistant_response) pairs, keeping only the latest occurrence.

    Supports field names: history, messages, conversation, context_messages.
    A pair is considered duplicate when both the user text and the immediately
    following assistant text are identical (case-insensitive).

    Returns a new body dict (original is not mutated).
    """
    HISTORY_FIELDS = ("history", "messages", "conversation", "context_messages")

    def _find_history_path(d: dict) -> tuple[dict, str] | None:
        """Return (container_dict, field_name) for the first history array found."""
        for field in HISTORY_FIELDS:
            if isinstance(d.get(field), list):
                return d, field
        params = d.get("params", {})
        if isinstance(params, dict):
            for field in HISTORY_FIELDS:
                if isinstance(params.get(field), list):
                    return params, field
        return None

    location = _find_history_path(body)
    if location is None:
        return body

    container, field = location
    history: list = container[field]
    if len(history) < 4:  # need at least 2 pairs to deduplicate
        return body

    # Build (user_text, assistant_text) pairs with their index ranges
    seen: dict[tuple[str, str], int] = {}  # pair -> index of user message
    i = 0
    indices_to_drop: set[int] = set()

    while i < len(history):
        msg = history[i]
        if not isinstance(msg, dict):
            i += 1
            continue
        role = msg.get("role", "")
        if role == "user" and i + 1 < len(history):
            next_msg = history[i + 1]
            if isinstance(next_msg, dict) and next_msg.get("role") in ("assistant", "agent", "model"):
                u_text = _parts_text(msg.get("parts", [])) or msg.get("content", "").strip().lower()
                a_text = _parts_text(next_msg.get("parts", [])) or next_msg.get("content", "").strip().lower()
                pair = (u_text, a_text)
                if pair in seen:
                    indices_to_drop.add(seen[pair])
                    indices_to_drop.add(seen[pair] + 1)
                seen[pair] = i
                i += 2
                continue
        i += 1

    if not indices_to_drop:
        return body

    new_history = [msg for idx, msg in enumerate(history) if idx not in indices_to_drop]
    new_body = copy.deepcopy(body)
    if container is body.get("params", {}):
        new_body["params"][field] = new_history
    else:
        new_body[field] = new_history
    return new_body


class CacheManager:
    def __init__(self, redis_client: aioredis.Redis):
        self._r = redis_client

    async def get(self, key: str, agent_name: str) -> Optional[str]:
        val = await self._r.get(key)
        stats_key = f"{CACHE_STATS_NS}:{agent_name}"
        if val is not None:
            await self._r.hincrby(stats_key, "hits", 1)
            logger.debug(f"Cache HIT: {key}")
            return val.decode("utf-8") if isinstance(val, bytes) else val
        await self._r.hincrby(stats_key, "misses", 1)
        return None

    async def set(self, key: str, agent_name: str, value: str, ttl: int) -> None:
        await self._r.setex(key, ttl, value)

    async def get_ttl_for_agent(self, agent_name: str) -> int:
        val = await self._r.get(f"rm:cache_ttl:{agent_name}")
        if val is not None:
            return int(val)
        return settings.DEFAULT_CACHE_TTL_SECONDS

    async def set_ttl_for_agent(self, agent_name: str, ttl: int) -> None:
        await self._r.set(f"rm:cache_ttl:{agent_name}", ttl)

    async def clear_all(self) -> int:
        keys = await self._r.keys(f"{CACHE_NS}:*")
        if keys:
            return await self._r.delete(*keys)
        return 0

    async def clear_agent(self, agent_name: str) -> int:
        keys = await self._r.keys(f"{CACHE_NS}:{agent_name}:*")
        if keys:
            return await self._r.delete(*keys)
        return 0

    async def get_stats(self) -> dict:
        keys = await self._r.keys(f"{CACHE_STATS_NS}:*")
        stats = {}
        for key in keys:
            key_str = key.decode() if isinstance(key, bytes) else key
            agent_name = key_str.split(":")[-1]
            raw = await self._r.hgetall(key)
            entry = {
                (k.decode() if isinstance(k, bytes) else k): int(v)
                for k, v in raw.items()
            }
            total = entry.get("hits", 0) + entry.get("misses", 0)
            entry["hit_rate"] = round(entry.get("hits", 0) / total, 3) if total else 0.0
            stats[agent_name] = entry
        return stats


class RequestLogger:
    """Stores the last N requests per agent in Redis for analytics."""

    def __init__(self, redis_client: aioredis.Redis):
        self._r = redis_client

    async def log(
        self,
        agent_name: str,
        query: str,
        cache_result: str,
        latency_ms: float,
        queued: bool = False,
        error: bool = False,
        method: str = "message/send",
    ) -> None:
        entry = json.dumps({
            "ts": round(time.time(), 3),
            "agent": agent_name,
            "query": query[:300],  # cap length
            "cache": cache_result,
            "latency_ms": round(latency_ms, 1),
            "queued": queued,
            "error": error,
            "method": method,
        })
        pipe = self._r.pipeline()
        log_key = f"{REQ_LOG_NS}:{agent_name}"
        pipe.lpush(log_key, entry)
        pipe.ltrim(log_key, 0, REQ_LOG_MAX - 1)
        await pipe.execute()

    async def get_recent(self, agent_name: str, limit: int = 20) -> list:
        raw = await self._r.lrange(f"{REQ_LOG_NS}:{agent_name}", 0, limit - 1)
        return [json.loads(r) for r in raw]

    async def get_all_agents(self) -> list[str]:
        keys = await self._r.keys(f"{REQ_LOG_NS}:*")
        return [
            (k.decode() if isinstance(k, bytes) else k).split(":", 2)[-1]
            for k in keys
        ]

    async def get_all_recent(self, limit: int = 50) -> list:
        agents = await self.get_all_agents()
        all_entries = []
        for agent in agents:
            entries = await self.get_recent(agent, limit=limit)
            all_entries.extend(entries)
        all_entries.sort(key=lambda e: e["ts"], reverse=True)
        return all_entries[:limit]

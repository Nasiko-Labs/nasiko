from __future__ import annotations

import base64
import hashlib
import json
import re
from collections.abc import AsyncIterator
from typing import Any

from request_manager import redis_keys
from request_manager.models import AgentLimits, AgentTarget, CacheDecision, CachedResponse


def normalize_text(text: str) -> str:
    return re.sub(r"\s+", " ", text.strip().lower())


class CachePolicy:
    def decide(
        self,
        *,
        agent_id: str,
        target: AgentTarget,
        limits: AgentLimits,
        headers: dict[str, str],
        json_body: dict[str, Any],
    ) -> CacheDecision:
        normalized_headers = {key.lower(): value for key, value in headers.items()}

        if not limits.cache_enabled:
            return CacheDecision(cacheable=False, reason="agent-cache-disabled")

        cache_control = normalized_headers.get("cache-control", "")
        if "no-cache" in cache_control.lower():
            return CacheDecision(cacheable=False, reason="cache-control-no-cache")

        method = json_body.get("method")
        if method != "message/send":
            return CacheDecision(cacheable=False, reason="unsupported-method")

        parts = json_body.get("params", {}).get("message", {}).get("parts")
        if not parts:
            return CacheDecision(cacheable=False, reason="non-text-part")

        texts: list[str] = []
        for part in parts:
            if not isinstance(part, dict):
                return CacheDecision(cacheable=False, reason="non-text-part")

            text = part.get("text")
            if part.get("kind") != "text" or not isinstance(text, str):
                return CacheDecision(cacheable=False, reason="non-text-part")
            texts.append(normalize_text(text))

        fingerprint = {
            "agent_id": agent_id,
            "method": method,
            "scope": normalized_headers.get("x-subject-id", "anonymous"),
            "target_revision": target.target_revision,
            "texts": texts,
        }
        fingerprint_json = json.dumps(
            fingerprint,
            sort_keys=True,
            separators=(",", ":"),
        )
        cache_key = hashlib.sha256(fingerprint_json.encode("utf-8")).hexdigest()

        return CacheDecision(
            cacheable=True,
            reason="cacheable",
            cache_key=cache_key,
            fingerprint=fingerprint,
        )


class RedisResponseCache:
    def __init__(self, redis: Any) -> None:
        self.redis = redis

    async def get(self, cache_key: str) -> CachedResponse | None:
        try:
            value = await self.redis.get(redis_keys.cache_entry(cache_key))
        except Exception:
            return None

        if value is None:
            return None

        try:
            if isinstance(value, bytes):
                value = value.decode("utf-8")
            payload = json.loads(value)
            body = base64.b64decode(payload["body_b64"])
            return CachedResponse(
                status_code=payload["status_code"],
                media_type=payload.get("media_type"),
                headers=payload.get("headers", {}),
                body=body,
            )
        except Exception:
            return None

    async def set(
        self,
        cache_key: str,
        response: CachedResponse,
        *,
        ttl_seconds: int,
    ) -> None:
        payload = {
            "status_code": response.status_code,
            "media_type": response.media_type,
            "headers": response.headers,
            "body_b64": base64.b64encode(response.body).decode("ascii"),
        }

        try:
            await self.redis.set(
                redis_keys.cache_entry(cache_key),
                json.dumps(payload, sort_keys=True, separators=(",", ":")),
                ex=ttl_seconds,
            )
        except Exception:
            return

    async def clear(self, agent_id: str | None = None) -> int:
        if agent_id is not None:
            return 0

        pattern = "request-manager:cache:*"

        try:
            scan_iter = getattr(self.redis, "scan_iter", None)
            if scan_iter is not None:
                removed = 0
                keys = scan_iter(match=pattern)
                if isinstance(keys, AsyncIterator):
                    async for key in keys:
                        removed += await self.redis.delete(key)
                else:
                    for key in keys:
                        removed += await self.redis.delete(key)
                return removed

            values = getattr(self.redis, "values", {})
            keys = [key for key in values if key.startswith("request-manager:cache:")]
            if not keys:
                return 0
            return await self.redis.delete(*keys)
        except Exception:
            return 0

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


def _cache_control_directives(value: str) -> set[str]:
    directives: set[str] = set()
    for directive in value.split(","):
        name = directive.strip().split("=", 1)[0].lower()
        if name:
            directives.add(name)
    return directives


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
        cache_control_directives = _cache_control_directives(cache_control)
        if "no-cache" in cache_control_directives:
            return CacheDecision(cacheable=False, reason="cache-control-no-cache")
        if "no-store" in cache_control_directives:
            return CacheDecision(cacheable=False, reason="cache-control-no-store")

        if json_body.get("jsonrpc") != "2.0":
            return CacheDecision(cacheable=False, reason="unsupported-jsonrpc")
        method = json_body.get("method")
        if method != "message/send":
            return CacheDecision(cacheable=False, reason="unsupported-method")

        scope = normalized_headers.get("x-subject-id", "").strip()
        if not scope:
            return CacheDecision(cacheable=False, reason="missing-subject-scope")

        params = json_body.get("params")
        if not isinstance(params, dict):
            return CacheDecision(cacheable=False, reason="invalid-message-shape")

        message = params.get("message")
        if not isinstance(message, dict):
            return CacheDecision(cacheable=False, reason="invalid-message-shape")

        parts = message.get("parts")
        if not isinstance(parts, list) or not parts:
            return CacheDecision(cacheable=False, reason="invalid-message-shape")

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
            "scope": scope,
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

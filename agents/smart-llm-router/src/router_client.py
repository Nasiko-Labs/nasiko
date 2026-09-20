"""Nasiko /v1/route client."""

from __future__ import annotations

import os
from dataclasses import dataclass
from typing import Any

import httpx


@dataclass
class RouterSuccess:
    provider: str
    model: str
    content: str
    usage: dict[str, Any]
    cache_hit: bool
    tier_used: str
    latency_ms: int
    trace_id: str
    raw: dict[str, Any]


@dataclass
class RouterError:
    status: int
    code: str
    message: str
    provider: str | None
    raw: dict[str, Any] | None


RouterResult = RouterSuccess | RouterError


def router_base_url() -> str:
    return os.getenv("ROUTER_BASE_URL", "http://localhost:8080").rstrip("/")


def router_timeout() -> float:
    try:
        return float(os.getenv("ROUTER_TIMEOUT_SECS", "30"))
    except ValueError:
        return 30.0


async def call_router(
    *,
    tier: str,
    task_type: str,
    complexity: int,
    user_prompt: str,
    session_id: str,
    budget_tokens: int = 2000,
    allow_cache: bool = True,
    client: httpx.AsyncClient | None = None,
) -> RouterResult:
    url = f"{router_base_url()}/v1/route"
    headers = {
        "Content-Type": "application/json",
        "x-nasiko-tier": tier,
        "x-session-id": session_id,
    }
    # Agent identity JWT (injected as OPENAI_API_KEY) lets /v1/route honor
    # agents.tier_cascade — when false, stick to the console-selected BYOK provider.
    agent_jwt = (os.getenv("OPENAI_API_KEY") or "").strip()
    if agent_jwt:
        headers["Authorization"] = f"Bearer {agent_jwt}"
    body = {
        "task_type": task_type,
        "complexity": complexity,
        "messages": [{"role": "user", "content": user_prompt}],
        "budget_tokens": budget_tokens,
        "allow_cache": allow_cache,
    }

    owns = client is None
    http = client or httpx.AsyncClient(timeout=router_timeout())
    try:
        resp = await http.post(url, headers=headers, json=body)
        try:
            data = resp.json()
        except Exception:
            data = None

        if 200 <= resp.status_code < 300 and isinstance(data, dict) and "content" in data:
            usage = data.get("usage") or {}
            return RouterSuccess(
                provider=str(data.get("provider", "")),
                model=str(data.get("model", "")),
                content=str(data.get("content", "")),
                usage=usage if isinstance(usage, dict) else {},
                cache_hit=bool(data.get("cache_hit", False)),
                tier_used=str(data.get("tier_used", tier)),
                latency_ms=int(data.get("latency_ms") or 0),
                trace_id=str(data.get("trace_id") or ""),
                raw=data,
            )

        err = (data or {}).get("error") if isinstance(data, dict) else None
        if isinstance(err, dict):
            return RouterError(
                status=resp.status_code,
                code=str(err.get("code") or f"http_{resp.status_code}"),
                message=str(err.get("message") or resp.text or "router error"),
                provider=err.get("provider"),
                raw=data if isinstance(data, dict) else None,
            )
        return RouterError(
            status=resp.status_code,
            code=f"http_{resp.status_code}",
            message=(resp.text or "router error")[:500],
            provider=None,
            raw=data if isinstance(data, dict) else None,
        )
    except httpx.TimeoutException:
        return RouterError(
            status=0,
            code="timeout",
            message=f"router timed out after {router_timeout()}s",
            provider=None,
            raw=None,
        )
    except httpx.HTTPError as e:
        return RouterError(
            status=0,
            code="transport_error",
            message=str(e),
            provider=None,
            raw=None,
        )
    finally:
        if owns:
            await http.aclose()

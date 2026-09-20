"""Async turn bus — fan-out Smart Router analytics to the local dashboard.

Nasiko console → A2A → agent.run_turn publishes here without blocking the reply.
The /ui dashboard subscribes via SSE (`GET /api/events`).
"""

from __future__ import annotations

import asyncio
import logging
import os
import time
import uuid
from collections import deque
from dataclasses import asdict, dataclass
from threading import Lock
from typing import Any

import httpx


@dataclass
class TurnEvent:
    id: str
    ts: float
    source: str  # "console" (A2A / Nasiko) | "ui" | "eval"
    session_id: str
    prompt: str
    ok: bool
    requested_tier: str
    task_type: str
    complexity: int
    classification_source: str
    provider: str | None
    model: str | None
    tier_used: str | None
    cache_hit: bool | None
    latency_ms: int | None
    prompt_tokens: int | None
    completion_tokens: int | None
    total_tokens: int | None
    cost_usd: float | None
    baseline_usd: float | None
    saved_usd: float | None
    pct_saved: str | None
    content_preview: str | None
    error: str | None
    running_saved_usd: float | None = None
    running_calls: int | None = None
    running_cache_hits: int | None = None

    def to_dict(self) -> dict[str, Any]:
        return asdict(self)


class TurnBus:
    """Ring buffer + fan-out queues for live dashboard subscribers."""

    def __init__(self, history: int = 200) -> None:
        self._lock = Lock()
        self._history: deque[TurnEvent] = deque(maxlen=history)
        self._subscribers: list[asyncio.Queue[TurnEvent | None]] = []
        self._global_saved = 0.0
        self._global_calls = 0
        self._global_cache_hits = 0
        self._tier_counts: dict[str, int] = {"cheap": 0, "balanced": 0, "premium": 0}

    def publish(self, event: TurnEvent) -> None:
        with self._lock:
            self._history.append(event)
            if event.ok:
                self._global_calls += 1
                if event.saved_usd:
                    self._global_saved += max(0.0, event.saved_usd)
                if event.cache_hit:
                    self._global_cache_hits += 1
                tier = event.requested_tier or event.tier_used or ""
                if tier in self._tier_counts:
                    self._tier_counts[tier] += 1
            subs = list(self._subscribers)

        for q in subs:
            try:
                q.put_nowait(event)
            except asyncio.QueueFull:
                pass

    def subscribe(self, maxsize: int = 64) -> asyncio.Queue[TurnEvent | None]:
        q: asyncio.Queue[TurnEvent | None] = asyncio.Queue(maxsize=maxsize)
        with self._lock:
            self._subscribers.append(q)
            snapshot = list(self._history)
        # Replay recent history so a late-opened dashboard catches up.
        for ev in snapshot:
            try:
                q.put_nowait(ev)
            except asyncio.QueueFull:
                break
        return q

    def unsubscribe(self, q: asyncio.Queue[TurnEvent | None]) -> None:
        with self._lock:
            if q in self._subscribers:
                self._subscribers.remove(q)

    def snapshot(self) -> dict[str, Any]:
        with self._lock:
            return {
                "global_saved_usd": self._global_saved,
                "global_calls": self._global_calls,
                "global_cache_hits": self._global_cache_hits,
                "tier_counts": dict(self._tier_counts),
                "recent": [e.to_dict() for e in list(self._history)[-50:]],
                "subscribers": len(self._subscribers),
            }


BUS = TurnBus()


def publish_from_result(
    *,
    prompt: str,
    session_id: str,
    source: str,
    result: Any,
) -> TurnEvent:
    """Build a TurnEvent from a TurnResult and publish locally."""
    from .router_client import RouterError, RouterSuccess

    router = result.router
    savings = result.savings
    totals = result.totals
    err: str | None = None
    provider = model = tier_used = None
    cache_hit = latency_ms = None
    prompt_tokens = completion_tokens = total_tokens = None
    cost_usd = baseline_usd = saved_usd = None
    pct_saved = None
    content_preview = None

    if isinstance(router, RouterSuccess):
        provider = router.provider
        model = router.model
        tier_used = router.tier_used
        cache_hit = router.cache_hit
        latency_ms = router.latency_ms
        usage = router.usage or {}
        prompt_tokens = usage.get("prompt_tokens")
        completion_tokens = usage.get("completion_tokens")
        total_tokens = usage.get("total_tokens")
        cost_usd = usage.get("cost_usd")
        content_preview = (router.content or "")[:240]
    elif isinstance(router, RouterError):
        err = f"{router.code}: {router.message}"

    if savings is not None:
        baseline_usd = float(savings.baseline_usd)
        saved_usd = float(savings.saved_usd_f)
        pct_saved = savings.pct_saved

    event = TurnEvent(
        id=str(uuid.uuid4()),
        ts=time.time(),
        source=source,
        session_id=session_id,
        prompt=prompt[:500],
        ok=bool(result.ok),
        requested_tier=result.requested_tier,
        task_type=result.classification.task_type,
        complexity=result.classification.complexity,
        classification_source=result.classification.source,
        provider=provider,
        model=model,
        tier_used=tier_used,
        cache_hit=cache_hit,
        latency_ms=latency_ms,
        prompt_tokens=int(prompt_tokens) if prompt_tokens is not None else None,
        completion_tokens=int(completion_tokens) if completion_tokens is not None else None,
        total_tokens=int(total_tokens) if total_tokens is not None else None,
        cost_usd=float(cost_usd) if cost_usd is not None else None,
        baseline_usd=baseline_usd,
        saved_usd=saved_usd,
        pct_saved=pct_saved,
        content_preview=content_preview,
        error=err,
        running_saved_usd=totals.running_saved_usd if totals else None,
        running_calls=totals.running_calls if totals else None,
        running_cache_hits=totals.running_cache_hits if totals else None,
    )
    BUS.publish(event)
    return event


def _analytics_ingest_url() -> str:
    return (os.environ.get("ANALYTICS_INGEST_URL") or "").strip()


async def _forward_event(event: TurnEvent) -> None:
    url = _analytics_ingest_url()
    if not url:
        return
    headers: dict[str, str] = {}
    token = (os.environ.get("ANALYTICS_TOKEN") or os.environ.get("DASHBOARD_TOKEN") or "").strip()
    if token:
        headers["x-analytics-token"] = token
    try:
        async with httpx.AsyncClient(timeout=2.0) as client:
            await client.post(url, json=event.to_dict(), headers=headers)
    except Exception as e:
        logging.getLogger(__name__).debug("analytics ingest failed: %s", e)


def publish_event_dict(data: dict[str, Any]) -> TurnEvent | None:
    """Ingest a turn payload from a remote agent into this process's bus."""
    try:
        event = TurnEvent(
            id=str(data.get("id") or uuid.uuid4()),
            ts=float(data.get("ts") or time.time()),
            source=str(data.get("source") or "console"),
            session_id=str(data.get("session_id") or ""),
            prompt=str(data.get("prompt") or "")[:500],
            ok=bool(data.get("ok")),
            requested_tier=str(data.get("requested_tier") or ""),
            task_type=str(data.get("task_type") or "chat"),
            complexity=int(data.get("complexity") or 3),
            classification_source=str(data.get("classification_source") or ""),
            provider=data.get("provider"),
            model=data.get("model"),
            tier_used=data.get("tier_used"),
            cache_hit=data.get("cache_hit"),
            latency_ms=data.get("latency_ms"),
            prompt_tokens=data.get("prompt_tokens"),
            completion_tokens=data.get("completion_tokens"),
            total_tokens=data.get("total_tokens"),
            cost_usd=data.get("cost_usd"),
            baseline_usd=data.get("baseline_usd"),
            saved_usd=data.get("saved_usd"),
            pct_saved=data.get("pct_saved"),
            content_preview=data.get("content_preview"),
            error=data.get("error"),
            running_saved_usd=data.get("running_saved_usd"),
            running_calls=data.get("running_calls"),
            running_cache_hits=data.get("running_cache_hits"),
        )
    except Exception:
        return None
    BUS.publish(event)
    return event


def publish_async(
    *,
    prompt: str,
    session_id: str,
    source: str,
    result: Any,
) -> None:
    """Publish locally + optionally forward to ANALYTICS_INGEST_URL (non-blocking)."""
    event = publish_from_result(
        prompt=prompt, session_id=session_id, source=source, result=result
    )
    if not _analytics_ingest_url():
        return
    try:
        loop = asyncio.get_running_loop()
    except RuntimeError:
        try:
            httpx.post(
                _analytics_ingest_url(),
                json=event.to_dict(),
                timeout=2.0,
            )
        except Exception as e:
            logging.getLogger(__name__).debug("analytics ingest failed: %s", e)
        return
    loop.create_task(_forward_event(event))

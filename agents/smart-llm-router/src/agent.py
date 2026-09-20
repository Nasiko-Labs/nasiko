"""Smart LLM Router — classify → tier → Nasiko /v1/route → savings.

Nasiko console sees only the model reply.
All routing / token / savings analytics are published async to the :8000 dashboard.
"""

from __future__ import annotations

import uuid
from collections.abc import AsyncIterator
from dataclasses import dataclass

import httpx

from classify import Classification, classify, parse_classifier_output
from format_out import format_chat_reply, format_error
from router_client import RouterError, RouterSuccess, call_router
from savings import apply_session, compute_savings
from session_store import SessionTotals
from tier import decide_tier
from turn_bus import publish_async


@dataclass
class TurnResult:
    text: str
    classification: Classification
    requested_tier: str
    router: RouterSuccess | RouterError
    savings: object | None
    totals: SessionTotals | None
    ok: bool
    session_id: str = ""
    prompt: str = ""


class SmartLlmRouterAgent:
    SUPPORTED_CONTENT_TYPES = ["text", "text/plain"]

    def __init__(self, http: httpx.AsyncClient | None = None) -> None:
        self._http = http

    async def run_turn(
        self,
        user_prompt: str,
        session_id: str | None = None,
        *,
        force_classification: Classification | None = None,
        source: str = "console",
    ) -> TurnResult:
        sid = session_id or str(uuid.uuid4())
        prompt = (user_prompt or "").strip()
        if not prompt:
            result = TurnResult(
                text="Please enter a message.",
                classification=Classification(
                    task_type="chat",
                    complexity=1,
                    reasoning="empty",
                    source="fallback",
                ),
                requested_tier="cheap",
                router=RouterError(
                    status=400,
                    code="bad_request",
                    message="missing user_prompt",
                    provider=None,
                    raw=None,
                ),
                savings=None,
                totals=None,
                ok=False,
                session_id=sid,
                prompt="",
            )
            publish_async(prompt="", session_id=sid, source=source, result=result)
            return result

        classification = force_classification or await classify(prompt, self._http)
        tier = decide_tier(classification.task_type, classification.complexity)

        router = await call_router(
            tier=tier,
            task_type=classification.task_type,
            complexity=classification.complexity,
            user_prompt=prompt,
            session_id=sid,
            client=self._http,
        )

        if isinstance(router, RouterError):
            result = TurnResult(
                text=format_error(router),
                classification=classification,
                requested_tier=tier,
                router=router,
                savings=None,
                totals=None,
                ok=False,
                session_id=sid,
                prompt=prompt,
            )
            publish_async(prompt=prompt, session_id=sid, source=source, result=result)
            return result

        savings = compute_savings(router.usage)
        totals = apply_session(sid, savings, router.cache_hit)
        # Console / A2A: chat text ONLY. Metrics go to the dashboard via publish_async.
        result = TurnResult(
            text=format_chat_reply(router, prompt),
            classification=classification,
            requested_tier=tier,
            router=router,
            savings=savings,
            totals=totals,
            ok=True,
            session_id=sid,
            prompt=prompt,
        )
        publish_async(prompt=prompt, session_id=sid, source=source, result=result)
        return result

    async def invoke(self, query: str, context_id: str) -> str:
        result = await self.run_turn(
            query,
            session_id=context_id or str(uuid.uuid4()),
            source="console",
        )
        return result.text

    async def invoke_streaming(self, query: str, context_id: str) -> AsyncIterator[str]:
        text = await self.invoke(query, context_id)
        yield text


__all__ = [
    "SmartLlmRouterAgent",
    "TurnResult",
    "parse_classifier_output",
]

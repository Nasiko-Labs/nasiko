"""
Observability agent executor — fetches compact monitoring data from the
Nasiko router and uses OpenAI to answer natural language questions.
Input is strictly limited: top 5 agents, last 5 events, 4 key fields each.
"""

import json
import logging
import os

import httpx
from a2a.server.agent_execution import AgentExecutor, RequestContext
from a2a.server.events import EventQueue
from a2a.utils import new_agent_text_message
from openai import AsyncOpenAI

logger = logging.getLogger(__name__)

ROUTER_URL = os.getenv("ROUTER_URL", "http://nasiko-router:8000")

SYSTEM_PROMPT = (
    "You are a system observability expert for Nasiko AI Platform. "
    "You have access to real-time monitoring data. Answer the user's question "
    "concisely and specifically using the provided metrics. Focus on actionable insights. "
    "Keep your response under 200 words."
)


class ObservabilityAgentExecutor(AgentExecutor):
    """Reads monitoring APIs and explains system state using OpenAI."""

    def __init__(self, api_key: str, model: str = "gpt-4o-mini"):
        self.openai = AsyncOpenAI(api_key=api_key)
        self.model = model

    async def execute(self, context: RequestContext, event_queue: EventQueue) -> None:
        query = context.get_user_input() or "What is the current system state?"
        try:
            answer = await self._process_request(query)
        except Exception as e:
            logger.error(f"Observability agent error: {e}")
            answer = f"Unable to fetch monitoring data: {e}"
        await event_queue.enqueue_event(new_agent_text_message(answer))

    async def _process_request(self, query: str) -> str:
        async with httpx.AsyncClient(timeout=5.0) as client:
            try:
                overview = (await client.get(f"{ROUTER_URL}/monitoring/overview")).json()
            except Exception:
                overview = {}
            try:
                impact = (await client.get(f"{ROUTER_URL}/monitoring/impact")).json()
            except Exception:
                impact = {}
            try:
                health = (await client.get(f"{ROUTER_URL}/monitoring/agents/health")).json()
            except Exception:
                health = {}
            try:
                events = (await client.get(f"{ROUTER_URL}/monitoring/events?n=5")).json()
            except Exception:
                events = {}

        # Strictly limited context — top 5 agents, last 5 events only
        health_top = dict(list(health.get("agents", {}).items())[:5])
        events_trimmed = events.get("events", [])[:5]

        context = {
            "overview": {
                "status": overview.get("status"),
                "cache_hit_rate": overview.get("cache_hit_rate"),
                "avg_latency_ms": overview.get("avg_latency_ms"),
                "queue_depth_total": overview.get("queue_depth_total"),
            },
            "impact": {
                "llm_calls_saved": impact.get("llm_calls_saved"),
                "cache_coverage_percent": impact.get("cache_coverage_percent"),
                "compute_saved_estimate_ms": impact.get("compute_saved_estimate_ms"),
            },
            "health": health_top,
            "recent_events": events_trimmed,
        }

        response = await self.openai.chat.completions.create(
            model=self.model,
            messages=[
                {"role": "system", "content": SYSTEM_PROMPT},
                {
                    "role": "user",
                    "content": f"System state:\n{json.dumps(context, indent=2)}\n\nQuestion: {query}",
                },
            ],
        )
        return response.choices[0].message.content

    async def cancel(self, context: RequestContext, event_queue: EventQueue) -> None:
        pass

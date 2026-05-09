"""
Traffic agent executor — simulates realistic LLM processing latency
so the dashboard caching benefit is visible within 5 seconds.
"""

import asyncio
import random
from datetime import datetime, timezone

from a2a.server.agent_execution import AgentExecutor, RequestContext
from a2a.server.events import EventQueue
from a2a.utils import new_agent_text_message


class TrafficAgentExecutor(AgentExecutor):
    """Simulates variable-latency business query analysis (0.8–2.5 s)."""

    async def execute(self, context: RequestContext, event_queue: EventQueue) -> None:
        query = context.get_user_input() or ""
        delay = random.uniform(0.8, 2.5)
        await asyncio.sleep(delay)

        word_count = len(query.split())
        ts = datetime.now(timezone.utc).strftime("%H:%M:%S UTC")
        response = (
            f"**Analysis Complete** ({delay:.1f}s processing time)\n\n"
            f"Query: *{query}*\n\n"
            f"This {word_count}-word query was processed using pattern matching and "
            f"contextual reasoning. Cache this result to serve future identical "
            f"queries in under 10ms.\n\n"
            f"_Processed at {ts}_"
        )
        await event_queue.enqueue_event(new_agent_text_message(response))

    async def cancel(self, context: RequestContext, event_queue: EventQueue) -> None:
        pass

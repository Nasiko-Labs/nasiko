# A real CrewAI agent served over A2A, as `A2AServerConfig` + a2a-sdk serve it. Each message
# it receives is a task for the agent, so it makes one LLM call. Like a crew made with
# `crewai create`, it is run from its own folder with the Python environment inside it, and
# reads its keys from that folder's `.env` (CrewAI loads it at import). Usage: crewai_agent.py <port>
import asyncio
import os
import sys

os.environ["CREWAI_DISABLE_TELEMETRY"] = "true"
os.environ["OTEL_SDK_DISABLED"] = "true"

import uvicorn
from a2a.server.agent_execution import AgentExecutor
from a2a.server.apps import A2AStarletteApplication
from a2a.server.request_handlers import DefaultRequestHandler
from a2a.server.tasks import InMemoryTaskStore
from a2a.utils import new_agent_text_message
from crewai import Agent
from crewai.a2a import A2AServerConfig

PORT = int(sys.argv[1])

researcher = Agent(
    role="Researcher",
    goal="Find facts",
    backstory="Careful analyst",
    llm="gpt-4o-mini",
    max_iter=2,
    a2a=A2AServerConfig(name="crew-researcher"),
)


class Research(AgentExecutor):
    async def execute(self, context, event_queue):
        result = await asyncio.to_thread(researcher.kickoff, context.get_user_input())
        await event_queue.enqueue_event(new_agent_text_message(result.raw))

    async def cancel(self, context, event_queue):
        raise NotImplementedError("a task can't be cancelled")


card = researcher.to_agent_card(f"http://127.0.0.1:{PORT}/")
app = A2AStarletteApplication(card, DefaultRequestHandler(Research(), InMemoryTaskStore()))
uvicorn.run(app.build(), host="127.0.0.1", port=PORT, log_level="warning")

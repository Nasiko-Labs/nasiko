# A real CrewAI agent served over A2A, as `A2AServerConfig` + a2a-sdk serve it.
# It never runs a task, so no LLM key is needed. Usage: crewai_agent.py <port>
import os
import sys

os.environ.setdefault("OPENAI_API_KEY", "not-used")
os.environ["CREWAI_DISABLE_TELEMETRY"] = "true"

import uvicorn
from a2a.server.agent_execution import AgentExecutor
from a2a.server.apps import A2AStarletteApplication
from a2a.server.request_handlers import DefaultRequestHandler
from a2a.server.tasks import InMemoryTaskStore
from crewai import Agent
from crewai.a2a import A2AServerConfig

PORT = int(sys.argv[1])

researcher = Agent(
    role="Researcher",
    goal="Find facts",
    backstory="Careful analyst",
    llm="gpt-4o-mini",
    a2a=A2AServerConfig(name="crew-researcher"),
)


class Idle(AgentExecutor):
    async def execute(self, context, event_queue): ...
    async def cancel(self, context, event_queue): ...


card = researcher.to_agent_card(f"http://127.0.0.1:{PORT}/")
app = A2AStarletteApplication(card, DefaultRequestHandler(Idle(), InMemoryTaskStore()))
uvicorn.run(app.build(), host="127.0.0.1", port=PORT, log_level="warning")

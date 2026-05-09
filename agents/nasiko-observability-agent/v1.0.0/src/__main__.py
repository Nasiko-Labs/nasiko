import logging
import os

import click
import uvicorn
from a2a.server.apps import A2AStarletteApplication
from a2a.server.request_handlers import DefaultRequestHandler
from a2a.server.tasks import InMemoryTaskStore
from a2a.types import AgentCapabilities, AgentCard, AgentSkill
from observability_agent_executor import ObservabilityAgentExecutor
from starlette.applications import Starlette

logging.basicConfig(level=logging.INFO)


@click.command()
@click.option("--host", default="0.0.0.0")
@click.option("--port", default=5000, type=int)
def main(host: str, port: int):
    api_key = os.getenv("OPENAI_API_KEY")
    if not api_key:
        raise ValueError("OPENAI_API_KEY is required for the observability agent")

    model = os.getenv("OBSERVABILITY_MODEL", "gpt-4o-mini")

    skill = AgentSkill(
        id="explain-system-state",
        name="System State Explanation",
        description=(
            "Reads live Nasiko monitoring APIs and explains system behavior in natural language. "
            "Ask about performance, cache impact, or system health."
        ),
        tags=["observability", "monitoring", "ai", "analysis", "demo"],
        examples=[
            "Why was the last request slow?",
            "What has improved since caching was enabled?",
            "Is the system under pressure right now?",
            "How much compute has been saved today?",
        ],
    )

    agent_card = AgentCard(
        name="Nasiko Observability Agent",
        description="AI agent that explains Nasiko system state in natural language",
        url=f"http://{host}:{port}/",
        version="1.0.0",
        default_input_modes=["text"],
        default_output_modes=["text"],
        capabilities=AgentCapabilities(streaming=True),
        skills=[skill],
    )

    executor = ObservabilityAgentExecutor(api_key=api_key, model=model)
    handler = DefaultRequestHandler(agent_executor=executor, task_store=InMemoryTaskStore())
    a2a_app = A2AStarletteApplication(agent_card=agent_card, http_handler=handler)
    app = Starlette(routes=a2a_app.routes())
    uvicorn.run(app, host=host, port=port)


if __name__ == "__main__":
    main()

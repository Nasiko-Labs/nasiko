import logging

import click
import uvicorn
from a2a.server.apps import A2AStarletteApplication
from a2a.server.request_handlers import DefaultRequestHandler
from a2a.server.tasks import InMemoryTaskStore
from a2a.types import AgentCapabilities, AgentCard, AgentSkill
from starlette.applications import Starlette
from traffic_agent_executor import TrafficAgentExecutor

logging.basicConfig(level=logging.INFO)


@click.command()
@click.option("--host", default="0.0.0.0")
@click.option("--port", default=5000, type=int)
def main(host: str, port: int):
    skill = AgentSkill(
        id="analyze-query",
        name="Business Query Analysis",
        description=(
            "Analyzes business and data queries, returning structured insights. "
            "Repeat the same query to observe cache hits in the Nasiko dashboard."
        ),
        tags=["analysis", "demo", "caching-showcase", "business"],
        examples=[
            "Analyze revenue trend for Q3 2025",
            "What are the top customer complaints?",
            "Summarize this week's performance",
            "Show me the sales breakdown by region",
        ],
    )

    agent_card = AgentCard(
        name="Nasiko Traffic Agent",
        description="Demo agent with variable latency for caching showcase",
        url=f"http://{host}:{port}/",
        version="1.0.0",
        default_input_modes=["text"],
        default_output_modes=["text"],
        capabilities=AgentCapabilities(streaming=True),
        skills=[skill],
    )

    executor = TrafficAgentExecutor()
    handler = DefaultRequestHandler(agent_executor=executor, task_store=InMemoryTaskStore())
    a2a_app = A2AStarletteApplication(agent_card=agent_card, http_handler=handler)
    app = Starlette(routes=a2a_app.routes())
    uvicorn.run(app, host=host, port=port)


if __name__ == "__main__":
    main()

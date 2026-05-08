import logging
import os

import click
import uvicorn
from a2a.server.apps import A2AStarletteApplication
from a2a.server.request_handlers import DefaultRequestHandler
from a2a.server.tasks import InMemoryTaskStore
from a2a.types import (
    AgentCapabilities,
    AgentCard,
    AgentSkill,
)
from dotenv import load_dotenv
from gateway_agent_executor import GatewayAgentExecutor  # type: ignore[import-not-found]
from starlette.applications import Starlette

load_dotenv()

logging.basicConfig()


@click.command()
@click.option("--host", default="localhost")
@click.option("--port", default=5000)
def main(host: str, port: int) -> None:
    skill = AgentSkill(
        id="gateway_demo",
        name="Gateway Demo",
        description=(
            "Demonstration LLM call routed through the Nasiko platform "
            "LLM gateway. No provider API key required in agent source."
        ),
        tags=["demo", "gateway", "llm"],
        examples=["ping", "Say hello", "What is 2+2?"],
    )

    agent_card = AgentCard(
        name="a2a-gateway-demo",
        description=(
            "Demonstration agent using the platform LLM gateway — "
            "no provider keys in source."
        ),
        url=f"http://{host}:{port}/",
        version="1.0.0",
        default_input_modes=["text"],
        default_output_modes=["text"],
        capabilities=AgentCapabilities(streaming=False),
        skills=[skill],
    )

    agent_executor = GatewayAgentExecutor(card=agent_card)

    request_handler = DefaultRequestHandler(
        agent_executor=agent_executor,
        task_store=InMemoryTaskStore(),
    )

    a2a_app = A2AStarletteApplication(
        agent_card=agent_card, http_handler=request_handler
    )
    app = Starlette(routes=a2a_app.routes())
    uvicorn.run(app, host=host, port=port)


if __name__ == "__main__":
    main()

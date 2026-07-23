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
from .openai_agent import create_agent
from .openai_agent_executor import OpenAIAgentExecutor
from starlette.applications import Starlette

load_dotenv()

logging.basicConfig()


@click.command()
@click.option("--host", "host", default="localhost")
@click.option("--port", "port", default=5000)
def main(host: str, port: int):
    base_url = None
    model = "gpt-4o"

    if os.getenv("OPENROUTER_API_KEY"):
        api_key = os.getenv("OPENROUTER_API_KEY")
        base_url = "https://openrouter.ai/api/v1"
        model = os.getenv("OPENROUTER_MODEL", "nvidia/nemotron-3-super-120b-a12b:free")
    elif os.getenv("MINIMAX_API_KEY"):
        api_key = os.getenv("MINIMAX_API_KEY")
        base_url = os.getenv("MINIMAX_BASE_URL", "https://api.minimax.io/v1")
        model = os.getenv("MINIMAX_MODEL", "MiniMax-M2.7")
    else:
        api_key = os.getenv("OPENAI_API_KEY")

    if not api_key:
        raise ValueError(
            "One of OPENROUTER_API_KEY, OPENAI_API_KEY, or MINIMAX_API_KEY must be set"
        )

    skill = AgentSkill(
        id="search_web",
        name="Search Web",
        description="Search the internet and read web pages to answer queries",
        tags=["search", "web", "research", "url"],
        examples=[
            "Search the web for the latest news on AI",
            "Who won the Superbowl in 2024?",
        ],
    )

    # AgentCard for OpenAI-based agent
    agent_card = AgentCard(
        name="Web Search Agent",
        description="An agent that can autonomously search the internet and read web pages to answer queries",
        url=f"http://{host}:{port}/",
        version="1.0.0",
        default_input_modes=["text"],
        default_output_modes=["text"],
        capabilities=AgentCapabilities(streaming=True),
        skills=[skill],
    )

    # Create OpenAI agent
    agent_data = create_agent()

    agent_executor = OpenAIAgentExecutor(
        card=agent_card,
        tools=agent_data["tools"],
        api_key=api_key,
        system_prompt=agent_data["system_prompt"],
        base_url=base_url,
        model=model,
    )

    request_handler = DefaultRequestHandler(
        agent_executor=agent_executor, task_store=InMemoryTaskStore()
    )

    a2a_app = A2AStarletteApplication(
        agent_card=agent_card, http_handler=request_handler
    )
    routes = a2a_app.routes()

    app = Starlette(routes=routes)

    uvicorn.run(app, host=host, port=port)


if __name__ == "__main__":
    main()

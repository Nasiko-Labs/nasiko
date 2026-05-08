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
from openai_agent import create_agent  # type: ignore[import-not-found]
from openai_agent_executor import (
    OpenAIAgentExecutor,  # type: ignore[import-untyped]
)
from starlette.applications import Starlette

load_dotenv()

logging.basicConfig()


@click.command()
@click.option("--host", "host", default="localhost")
@click.option("--port", "port", default=5000)
def main(host: str, port: int):
    base_url = None
    model = os.getenv("LLM_GATEWAY_MODEL", "platform-default")

    if os.getenv("LLM_GATEWAY_URL") and os.getenv("LLM_VIRTUAL_KEY"):
        api_key = os.getenv("LLM_VIRTUAL_KEY")
        base_url = os.getenv("LLM_GATEWAY_URL")
    elif os.getenv("OPENROUTER_API_KEY"):
        api_key = os.getenv("OPENROUTER_API_KEY")
        base_url = os.getenv("OPENROUTER_BASE_URL", "https://openrouter.ai/api/v1")
        model = os.getenv("OPENROUTER_MODEL", "openai/gpt-4o-mini")
    elif os.getenv("MINIMAX_API_KEY"):
        api_key = os.getenv("MINIMAX_API_KEY")
        base_url = os.getenv("MINIMAX_BASE_URL", "https://api.minimax.io/v1")
        model = os.getenv("MINIMAX_MODEL", "MiniMax-M2.7")
    else:
        api_key = os.getenv("OPENAI_API_KEY")

    if not api_key:
        raise ValueError(
            "Set LLM_GATEWAY_URL + LLM_VIRTUAL_KEY (recommended), or one of OPENROUTER_API_KEY / OPENAI_API_KEY / MINIMAX_API_KEY"
        )

    skill = AgentSkill(
        id="translator_agent",
        name="Translator Agent",
        description="Translate text and web content between different languages",
        tags=["translation", "language", "text", "url"],
        examples=[
            'Translate "Hello world" to Spanish',
            "What does this French website say in English?",
            "Detect the language of this text",
            "Translate the content of this webpage to German",
        ],
    )

    # AgentCard for OpenAI-based agent
    agent_card = AgentCard(
        name="Translator Agent",
        description="An agent that can translate text and web content between different languages",
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

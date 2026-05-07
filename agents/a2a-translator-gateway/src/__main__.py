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
    # =========================================================================
    # LLM Gateway Pattern
    # =========================================================================
    # This agent uses the Nasiko LLM Gateway (LiteLLM) instead of hardcoded
    # provider API keys. The gateway URL and virtual key are injected as
    # environment variables at deploy time by the orchestrator.
    #
    # Benefits:
    #   - No provider keys in source code
    #   - Switch providers (OpenAI ↔ OpenRouter ↔ MiniMax) without code changes
    #   - Centralized spend tracking and rate limiting
    #   - All LLM calls traced via OpenTelemetry → Phoenix
    #
    # Fallback: if gateway env vars are not set, falls back to direct keys
    # for backward compatibility.
    # =========================================================================

    gateway_url = os.getenv("LITELLM_GATEWAY_URL")
    gateway_key = os.getenv("LITELLM_API_KEY")

    if gateway_url and gateway_key:
        # Use the LLM Gateway (recommended)
        api_key = gateway_key
        base_url = f"{gateway_url}/v1"  # LiteLLM serves OpenAI-compatible API at /v1
        model = os.getenv("LITELLM_MODEL", "gpt-4o-mini")
        print(f"🌐 Using LLM Gateway at {gateway_url}")
        print(f"   Model: {model}")
        print(f"   Key: {api_key[:12]}...")
    elif os.getenv("OPENROUTER_API_KEY"):
        # Fallback: direct OpenRouter
        api_key = os.getenv("OPENROUTER_API_KEY")
        base_url = "https://openrouter.ai/api/v1"
        model = os.getenv("OPENROUTER_MODEL", "nvidia/nemotron-3-super-120b-a12b:free")
        print("⚠️  Using direct OpenRouter key (consider using the LLM Gateway instead)")
    elif os.getenv("MINIMAX_API_KEY"):
        # Fallback: direct MiniMax
        api_key = os.getenv("MINIMAX_API_KEY")
        base_url = os.getenv("MINIMAX_BASE_URL", "https://api.minimax.io/v1")
        model = os.getenv("MINIMAX_MODEL", "MiniMax-M2.7")
        print("⚠️  Using direct MiniMax key (consider using the LLM Gateway instead)")
    elif os.getenv("OPENAI_API_KEY"):
        # Fallback: direct OpenAI
        api_key = os.getenv("OPENAI_API_KEY")
        base_url = None
        model = "gpt-4o"
        print("⚠️  Using direct OpenAI key (consider using the LLM Gateway instead)")
    else:
        raise ValueError(
            "No LLM configuration found. Set LITELLM_GATEWAY_URL + LITELLM_API_KEY "
            "(recommended) or one of OPENROUTER_API_KEY, OPENAI_API_KEY, MINIMAX_API_KEY"
        )

    skill = AgentSkill(
        id="translator_agent",
        name="Translator Agent (Gateway)",
        description="Translate text and web content between different languages using the LLM Gateway",
        tags=["translation", "language", "text", "url", "gateway"],
        examples=[
            'Translate "Hello world" to Spanish',
            "What does this French website say in English?",
            "Detect the language of this text",
            "Translate the content of this webpage to German",
        ],
    )

    # AgentCard for gateway-powered agent
    agent_card = AgentCard(
        name="Translator Agent (Gateway)",
        description="An agent that translates text and web content using the Nasiko LLM Gateway — no hardcoded provider keys",
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

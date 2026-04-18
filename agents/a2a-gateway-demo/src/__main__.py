"""
Gateway Demo Agent
==================
Demonstrates the Nasiko LLM gateway pattern: the agent carries NO provider API
key in its source.  The orchestrator injects LLM_GATEWAY_URL and LLM_VIRTUAL_KEY
at deploy time so the agent is completely provider-agnostic.

Switching the underlying model (OpenAI → OpenRouter → MiniMax) is a gateway
config change only — no code change needed here.
"""
import logging
import os

import click
import uvicorn
from a2a.server.apps import A2AStarletteApplication
from a2a.server.request_handlers import DefaultRequestHandler
from a2a.server.tasks import InMemoryTaskStore
from a2a.types import AgentCapabilities, AgentCard, AgentSkill
from dotenv import load_dotenv
from openai import AsyncOpenAI
from starlette.applications import Starlette

from gateway_executor import GatewayAgentExecutor  # type: ignore[import-not-found]

load_dotenv()
logging.basicConfig()


@click.command()
@click.option("--host", default="0.0.0.0")
@click.option("--port", default=5001, type=int)
def main(host: str, port: int) -> None:
    gateway_url = os.environ.get("LLM_GATEWAY_URL", "http://llm-gateway:4000")
    virtual_key = os.environ.get("LLM_VIRTUAL_KEY", "")
    model = os.environ.get("LLM_MODEL", "gpt-4o-mini")

    if not virtual_key:
        raise ValueError(
            "LLM_VIRTUAL_KEY is not set. "
            "The Nasiko orchestrator should inject this at deploy time. "
            "Do NOT add a raw provider API key — use the LLM gateway instead."
        )

    # The OpenAI SDK is OpenAI-API-compatible; LiteLLM exposes the same interface.
    client = AsyncOpenAI(api_key=virtual_key, base_url=f"{gateway_url}/v1")

    skill = AgentSkill(
        id="answer-questions",
        name="Answer Questions",
        description="Answer general knowledge questions via the platform LLM gateway",
        tags=["qa", "general", "assistant"],
        examples=[
            "What is the capital of France?",
            "Explain how TCP/IP works in simple terms",
            "Write a haiku about software engineering",
        ],
    )

    agent_card = AgentCard(
        name="Gateway Demo Agent",
        description=(
            "A demonstration agent that uses the Nasiko LLM gateway. "
            "No provider API key is embedded in this agent."
        ),
        url=f"http://{host}:{port}/",
        version="1.0.0",
        default_input_modes=["text"],
        default_output_modes=["text"],
        capabilities=AgentCapabilities(streaming=True),
        skills=[skill],
    )

    executor = GatewayAgentExecutor(
        card=agent_card,
        client=client,
        model=model,
    )

    request_handler = DefaultRequestHandler(
        agent_executor=executor, task_store=InMemoryTaskStore()
    )
    a2a_app = A2AStarletteApplication(agent_card=agent_card, http_handler=request_handler)
    app = Starlette(routes=a2a_app.routes())
    uvicorn.run(app, host=host, port=port)


if __name__ == "__main__":
    main()

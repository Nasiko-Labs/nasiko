import logging
import os

import click
import httpx
import uvicorn
from a2a.server.apps import A2AStarletteApplication
from a2a.server.request_handlers import DefaultRequestHandler
from a2a.server.tasks import (
    BasePushNotificationSender,
    InMemoryPushNotificationConfigStore,
    InMemoryTaskStore,
)
from a2a.types import AgentCapabilities, AgentCard, AgentSkill
from dotenv import load_dotenv
from telemetry import init_telemetry
from starlette.middleware.cors import CORSMiddleware

from agent import DeepAnalystAgent
from agent_executor import DeepAnalystAgentExecutor

load_dotenv()
logging.basicConfig(level=logging.INFO)
init_telemetry("deep-analyst")
logger = logging.getLogger(__name__)


@click.command()
@click.option("--host", default="localhost")
@click.option("--port", default=8000)
def main(host, port):
    """Starts the Deep Analyst Agent server."""
    capabilities = AgentCapabilities(streaming=True, push_notifications=True)
    skill = AgentSkill(
        id="deep_analysis",
        name="Deep Analysis",
        description="Multi-step reasoning with web search and financial data tools for thorough analysis.",
        tags=["analysis", "reasoning", "research", "finance"],
        examples=[
            "What's the current USD to EUR rate and how has it trended?",
            "Analyze the impact of recent AI developments on the tech market",
        ],
    )
    agent_card = AgentCard(
        name="Deep Analyst Agent",
        description="Stateful multi-step reasoning agent with tool access for deep analysis. Uses web search and exchange rate data.",
        url=os.getenv("HOST_OVERRIDE", f"http://{host}:{port}/"),
        version="1.0.0",
        default_input_modes=DeepAnalystAgent.SUPPORTED_CONTENT_TYPES,
        default_output_modes=DeepAnalystAgent.SUPPORTED_CONTENT_TYPES,
        capabilities=capabilities,
        skills=[skill],
    )

    httpx_client = httpx.AsyncClient()
    push_config_store = InMemoryPushNotificationConfigStore()
    push_sender = BasePushNotificationSender(
        httpx_client=httpx_client, config_store=push_config_store
    )
    request_handler = DefaultRequestHandler(
        agent_executor=DeepAnalystAgentExecutor(),
        task_store=InMemoryTaskStore(),
        push_config_store=push_config_store,
        push_sender=push_sender,
    )
    server = A2AStarletteApplication(agent_card=agent_card, http_handler=request_handler)
    app = server.build()
    app.add_middleware(
        CORSMiddleware,
        allow_origins=["*"],
        allow_credentials=True,
        allow_methods=["*"],
        allow_headers=["*"],
    )
    uvicorn.run(app, host=host, port=port)


if __name__ == "__main__":
    main()

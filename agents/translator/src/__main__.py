import logging
import os

import click
import uvicorn
from a2a.server.request_handlers import DefaultRequestHandler
from a2a.server.routes import create_agent_card_routes, create_jsonrpc_routes
from a2a.server.tasks import InMemoryTaskStore
from a2a.types import AgentCapabilities, AgentCard, AgentInterface, AgentSkill
from dotenv import load_dotenv
from starlette.applications import Starlette
from telemetry import init_telemetry

from agent_executor import TranslatorExecutor

load_dotenv()
logging.basicConfig(level=logging.INFO)
init_telemetry("translator")

logger = logging.getLogger(__name__)


@click.command()
@click.option("--host", default="0.0.0.0")
@click.option("--port", default=int(os.environ.get("PORT", "8000")), type=int)
def main(host: str, port: int):
    if not os.getenv("OPENAI_API_KEY"):
        logger.error("OPENAI_API_KEY is not set")
        raise SystemExit(1)

    agent_url = os.getenv("HOST_OVERRIDE", f"http://{host}:{port}/")

    agent_card = AgentCard(
        name="Translator Agent",
        description="Translates text and web content between languages using AI",
        supported_interfaces=[
            AgentInterface(
                protocol_binding="JSONRPC",
                url=agent_url,
            )
        ],
        version="1.0.0",
        default_input_modes=["text/plain"],
        default_output_modes=["text/plain"],
        capabilities=AgentCapabilities(streaming=True),
        skills=[
            AgentSkill(
                id="translate",
                name="Translation",
                description="Translate text or web page content between any languages",
                tags=["translation", "language", "text", "url"],
                examples=[
                    "Translate 'Hello world' to Spanish",
                    "What does this French website say in English?",
                    "Detect the language of this text",
                ],
            )
        ],
    )

    handler = DefaultRequestHandler(
        agent_executor=TranslatorExecutor(),
        task_store=InMemoryTaskStore(),
        agent_card=agent_card,
    )

    routes = create_agent_card_routes(agent_card) + create_jsonrpc_routes(handler, rpc_url="/")
    app = Starlette(routes=routes)

    logger.info("Translator Agent listening on %s:%s", host, port)
    uvicorn.run(app, host=host, port=port)


if __name__ == "__main__":
    main()
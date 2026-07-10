import logging
import os

import click
import uvicorn
from a2a.server.request_handlers import DefaultRequestHandler
from a2a.server.routes import create_agent_card_routes, create_jsonrpc_routes
from a2a.server.tasks import InMemoryTaskStore
from a2a.types import AgentCapabilities, AgentCard, AgentInterface, AgentSkill
from dotenv import load_dotenv

from agent_executor import SummarizerExecutor

load_dotenv()
logging.basicConfig(level=logging.INFO)

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
        name="Summarizer Agent",
        description="Summarizes text into concise bullet points or paragraphs using AI",
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
                id="summarize",
                name="Summarization",
                description="Summarize any text into a concise form",
                tags=["summarize", "summary", "text", "tldr"],
                examples=[
                    "Summarize this article: ...",
                    "Give me a TL;DR of the following",
                    "What are the key points of this text?",
                ],
            )
        ],
    )

    handler = DefaultRequestHandler(
        agent_executor=SummarizerExecutor(),
        task_store=InMemoryTaskStore(),
        agent_card=agent_card,
    )

    routes = create_agent_card_routes(agent_card) + create_jsonrpc_routes(handler, rpc_url="/")
    from starlette.applications import Starlette
    app = Starlette(routes=routes)

    logger.info("Summarizer Agent listening on %s:%s", host, port)
    uvicorn.run(app, host=host, port=port)


if __name__ == "__main__":
    main()
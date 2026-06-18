import logging
import os

import click
import uvicorn
from a2a.server.apps import A2AStarletteApplication
from a2a.server.request_handlers import DefaultRequestHandler
from a2a.server.tasks import InMemoryTaskStore
from a2a.types import AgentCapabilities, AgentCard, AgentSkill
from dotenv import load_dotenv
from telemetry import init_telemetry
from starlette.middleware.cors import CORSMiddleware

from agent import ResearchAgent
from agent_executor import ResearchAgentExecutor

load_dotenv()
logging.basicConfig(level=logging.INFO)
init_telemetry("google-adk")
logger = logging.getLogger(__name__)

CORS_ORIGINS = [
    "http://localhost:4000",
    "http://127.0.0.1:4000",
    "http://localhost:3000",
    "http://127.0.0.1:3000",
]


@click.command()
@click.option("--host", default="localhost")
@click.option("--port", default=10006)
def main(host, port):
    """Starts the Google ADK Research Agent server."""
    if not os.getenv("GOOGLE_API_KEY"):
        logger.error("GOOGLE_API_KEY environment variable not set.")
        raise SystemExit(1)

    capabilities = AgentCapabilities(streaming=True)
    skill = AgentSkill(
        id="wikipedia_research",
        name="Wikipedia Research",
        description="Answers questions by searching Wikipedia for relevant information.",
        tags=["research", "wikipedia", "qa"],
        examples=["What is quantum computing?", "Tell me about the Eiffel Tower"],
    )
    agent_url = os.getenv("HOST_OVERRIDE", f"http://{host}:{port}/")
    agent_card = AgentCard(
        name="Google ADK Research Agent",
        description="Answers questions using Wikipedia as a knowledge source.",
        url=agent_url,
        version="1.0.0",
        default_input_modes=ResearchAgent.SUPPORTED_CONTENT_TYPES,
        default_output_modes=ResearchAgent.SUPPORTED_CONTENT_TYPES,
        capabilities=capabilities,
        skills=[skill],
    )
    request_handler = DefaultRequestHandler(
        agent_executor=ResearchAgentExecutor(),
        task_store=InMemoryTaskStore(),
    )
    server = A2AStarletteApplication(agent_card=agent_card, http_handler=request_handler)
    app = server.build()
    app.add_middleware(
        CORSMiddleware,
        allow_origins=CORS_ORIGINS,
        allow_credentials=True,
        allow_methods=["*"],
        allow_headers=["*"],
    )
    uvicorn.run(app, host=host, port=port)


if __name__ == "__main__":
    main()

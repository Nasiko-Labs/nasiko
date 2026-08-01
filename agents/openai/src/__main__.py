import logging
import os

from dotenv import load_dotenv

# Instrumentation must initialize before a2a-sdk (and anything it imports,
# e.g. Starlette) is imported below: OTel's Starlette instrumentor patches by
# rebinding `starlette.applications.Starlette` to an instrumented subclass, so
# any module that already did `from starlette.applications import Starlette`
# keeps its original, un-instrumented reference forever — no incoming
# traceparent gets extracted, and every request starts an orphan root trace
# instead of joining the platform's session trace.
from telemetry import init_telemetry

load_dotenv(override=True)
logging.basicConfig(level=logging.INFO)
init_telemetry()

import click
import uvicorn
from a2a.server.request_handlers import DefaultRequestHandler
from a2a.server.routes import create_agent_card_routes, create_jsonrpc_routes
from a2a.server.tasks import InMemoryTaskStore
from a2a.types import AgentCapabilities, AgentCard, AgentInterface, AgentSkill
from starlette.applications import Starlette
from starlette.middleware.cors import CORSMiddleware

from agents import set_tracing_disabled

from agent import ResearchAgent
from agent_executor import ResearchAgentExecutor

set_tracing_disabled(True)
logger = logging.getLogger(__name__)

CORS_ORIGINS = [
    "http://localhost:4000",
    "http://127.0.0.1:4000",
    "http://localhost:3000",
    "http://127.0.0.1:3000",
]


@click.command()
@click.option("--host", default="localhost")
@click.option("--port", default=8000)
def main(host, port):
    """Starts the Research Agent server."""
    skill = AgentSkill(
        id="research",
        name="Web & Wikipedia Research",
        description="Searches the web and Wikipedia to gather comprehensive facts on any topic.",
        tags=["research", "web-search", "wikipedia", "information-retrieval"],
        examples=["What is quantum computing?", "Research the latest in AI agent frameworks"],
    )
    agent_url = os.getenv("HOST_OVERRIDE", f"http://{host}:{port}/")
    agent_card = AgentCard(
        name="Research Agent",
        description="Searches the web and Wikipedia to gather comprehensive facts. Produces thorough research summaries.",
        supported_interfaces=[
            AgentInterface(protocol_binding="JSONRPC", url=agent_url),
        ],
        version="1.0.0",
        default_input_modes=ResearchAgent.SUPPORTED_CONTENT_TYPES,
        default_output_modes=ResearchAgent.SUPPORTED_CONTENT_TYPES,
        capabilities=AgentCapabilities(streaming=True),
        skills=[skill],
    )
    request_handler = DefaultRequestHandler(
        agent_executor=ResearchAgentExecutor(),
        task_store=InMemoryTaskStore(),
        agent_card=agent_card,
    )

    routes = []
    routes.extend(create_agent_card_routes(agent_card))
    routes.extend(create_jsonrpc_routes(request_handler, rpc_url="/"))

    app = Starlette(routes=routes)
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

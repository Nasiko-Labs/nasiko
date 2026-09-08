import logging
import os

from dotenv import load_dotenv

load_dotenv()
logging.basicConfig(level=logging.INFO)

# Instrumentation must initialize before a2a-sdk (and anything it imports,
# e.g. Starlette) is imported below: OTel's Starlette instrumentor patches by
# rebinding `starlette.applications.Starlette` to an instrumented subclass, so
# any module that already did `from starlette.applications import Starlette`
# keeps its original, un-instrumented reference forever — no incoming
# traceparent gets extracted, and every request starts an orphan root trace
# instead of joining the platform's session trace.
from telemetry import init_telemetry

init_telemetry()

import click
import uvicorn
from a2a.server.request_handlers import DefaultRequestHandler
from a2a.server.routes import create_agent_card_routes, create_jsonrpc_routes
from a2a.server.tasks import InMemoryTaskStore
from a2a.types import AgentCapabilities, AgentCard, AgentInterface, AgentSkill
from starlette.applications import Starlette
from starlette.middleware.cors import CORSMiddleware

from agent import SynthesizerAgent
from agent_executor import SynthesizerAgentExecutor

logger = logging.getLogger(__name__)


@click.command()
@click.option("--host", default="0.0.0.0")
@click.option("--port", default=int(os.environ.get("PORT", "8000")), type=int)
def main(host, port):
    """Starts the Synthesizer Agent server."""
    if not os.getenv("ANTHROPIC_API_KEY"):
        logger.error("ANTHROPIC_API_KEY is not set")
        raise SystemExit(1)

    capabilities = AgentCapabilities(streaming=True)
    skill = AgentSkill(
        id="synthesize",
        name="Synthesize & Report",
        description="Synthesizes research and data from multiple sources into polished, well-structured reports.",
        tags=["synthesis", "writing", "reports", "analysis", "claude"],
        examples=[
            "Synthesize these research findings into a report",
            "Write an executive summary from this data",
            "Compare these sources and identify the strongest conclusions",
        ],
    )
    agent_url = os.getenv("HOST_OVERRIDE", f"http://{host}:{port}/")
    agent_card = AgentCard(
        name="synthesizer-agent",
        description="Transforms raw research, notes, and analysis into polished reports with summaries, key findings, and conclusions using Claude.",
        supported_interfaces=[
            AgentInterface(
                protocol_binding="JSONRPC",
                protocol_version="1.0",
                url=agent_url,
            )
        ],
        version="1.0.1",
        default_input_modes=SynthesizerAgent.SUPPORTED_CONTENT_TYPES,
        default_output_modes=SynthesizerAgent.SUPPORTED_CONTENT_TYPES,
        capabilities=capabilities,
        skills=[skill],
    )
    request_handler = DefaultRequestHandler(
        agent_executor=SynthesizerAgentExecutor(),
        task_store=InMemoryTaskStore(),
        agent_card=agent_card,
    )
    routes = create_agent_card_routes(agent_card) + create_jsonrpc_routes(
        request_handler,
        rpc_url="/",
    )
    app = Starlette(routes=routes)
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

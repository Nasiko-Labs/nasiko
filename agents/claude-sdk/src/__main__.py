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

from agent import SynthesizerAgent
from agent_executor import SynthesizerAgentExecutor

load_dotenv()
logging.basicConfig(level=logging.INFO)
init_telemetry("synthesizer")
logger = logging.getLogger(__name__)


@click.command()
@click.option("--host", default="localhost")
@click.option("--port", default=8000)
def main(host, port):
    """Starts the Synthesizer Agent server."""
    capabilities = AgentCapabilities(streaming=True)
    skill = AgentSkill(
        id="synthesize",
        name="Synthesize & Report",
        description="Takes research data from multiple sources and produces polished, well-structured reports.",
        tags=["synthesis", "writing", "reports", "analysis"],
        examples=[
            "Synthesize these research findings into a report",
            "Write an executive summary from this data",
        ],
    )
    agent_url = os.getenv("HOST_OVERRIDE", f"http://{host}:{port}/")
    agent_card = AgentCard(
        name="Synthesizer Agent",
        description="Takes raw research and data from other agents, produces clear structured reports with summaries, key findings, and conclusions.",
        url=agent_url,
        version="1.0.0",
        default_input_modes=SynthesizerAgent.SUPPORTED_CONTENT_TYPES,
        default_output_modes=SynthesizerAgent.SUPPORTED_CONTENT_TYPES,
        capabilities=capabilities,
        skills=[skill],
    )
    request_handler = DefaultRequestHandler(
        agent_executor=SynthesizerAgentExecutor(),
        task_store=InMemoryTaskStore(),
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

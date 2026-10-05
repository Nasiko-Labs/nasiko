import logging
import os

from dotenv import load_dotenv

# Instrumentation must initialize before a2a-sdk / Starlette are imported
# (see agents/openai/src/__main__.py for why).
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

from agent_executor import CopilotAgentExecutor
from copilot import Copilot
from rest import build_routes


@click.command()
@click.option("--host", default="localhost")
@click.option("--port", default=8000)
def main(host, port):
    """Starts the OSS Contribution Copilot (A2A on /, REST on /a2a/*)."""
    skill = AgentSkill(
        id="contribute",
        name="Governed OSS contribution",
        description="Reads an allow-listed GitHub repo, proposes one contribution, prepares the files "
                    "and opens a pull request from your fork — pausing at three human approval gates.",
        tags=["github", "open-source", "pull-request", "human-in-the-loop"],
        examples=["contribute to Nasiko-Labs/nasiko", "status t_0d35861c6ea8", "approvals"],
    )
    agent_url = os.getenv("HOST_OVERRIDE", f"http://{host}:{port}/")
    agent_card = AgentCard(
        name="OSS Contribution Copilot",
        description="Ships open-source pull requests under human approval gates, a repo allow-list "
                    "and a per-contribution cost budget.",
        supported_interfaces=[AgentInterface(protocol_binding="JSONRPC", url=agent_url)],
        version="1.0.0",
        default_input_modes=["text", "text/plain"],
        default_output_modes=["text", "text/plain"],
        capabilities=AgentCapabilities(streaming=False),
        skills=[skill],
    )
    copilot = Copilot()
    request_handler = DefaultRequestHandler(
        agent_executor=CopilotAgentExecutor(
            copilot, allow_a2a_approvals=os.getenv("ALLOW_A2A_APPROVALS") == "1"
        ),
        task_store=InMemoryTaskStore(),
        agent_card=agent_card,
    )
    routes = [*build_routes(copilot)]
    routes.extend(create_agent_card_routes(agent_card))
    routes.extend(create_jsonrpc_routes(request_handler, rpc_url="/"))
    uvicorn.run(Starlette(routes=routes), host=host, port=port)


if __name__ == "__main__":
    main()

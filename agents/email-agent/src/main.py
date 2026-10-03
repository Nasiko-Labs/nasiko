"""
Email Agent — Nasiko Agent
Entry point: A2A JSON-RPC server exposing an email-focused, HITL-capable
LLM+MCP assistant (see agent_executor.py for the InputRequired/AuthRequired
contract this agent implements, including the selectable-options extension).
"""

import json
import os
from pathlib import Path

from telemetry import TraceparentMiddleware, init_telemetry

init_telemetry("email-agent")

import uvicorn
from a2a.server.request_handlers import DefaultRequestHandler
from a2a.server.routes import create_agent_card_routes, create_jsonrpc_routes
from a2a.server.tasks import InMemoryTaskStore
from a2a.types import AgentCapabilities, AgentCard, AgentInterface, AgentSkill
from starlette.applications import Starlette
from starlette.responses import JSONResponse
from starlette.routing import Route

from agent_executor import EmailAgentExecutor, agent_token_ctx

_CARD_PATH = Path(__file__).parent.parent / "AgentCard.json"


def load_agent_card(url: str) -> AgentCard:
    """Build the SDK card from AgentCard.json — the file stays the single
    source of card data (it's what `nasiko validate` and publish read); only
    the serving URL is runtime information."""
    card = json.loads(_CARD_PATH.read_text())
    capabilities = card.get("capabilities", {})
    return AgentCard(
        name=card["name"],
        description=card["description"],
        version=card["version"],
        supported_interfaces=[AgentInterface(protocol_binding="JSONRPC", url=url)],
        default_input_modes=card.get("defaultInputModes", ["text/plain"]),
        default_output_modes=card.get("defaultOutputModes", ["text/plain"]),
        capabilities=AgentCapabilities(
            streaming=capabilities.get("streaming", True),
            push_notifications=capabilities.get("pushNotifications", False),
        ),
        skills=[
            AgentSkill(
                id=skill["id"],
                name=skill["name"],
                description=skill["description"],
                tags=skill.get("tags", []),
                examples=skill.get("examples", []),
            )
            for skill in card.get("skills", [])
        ],
    )


async def health(_request):
    return JSONResponse({"status": "ok", "agent": "email-agent"})


def main() -> None:
    host = "0.0.0.0"
    port = int(os.getenv("PORT", "8000"))
    agent_card = load_agent_card(os.getenv("HOST_OVERRIDE", f"http://{host}:{port}/"))

    executor = EmailAgentExecutor()
    handler = DefaultRequestHandler(
        agent_executor=executor,
        task_store=InMemoryTaskStore(),
        agent_card=agent_card,
    )
    routes = [
        Route("/health", health),
    ]
    routes += create_agent_card_routes(agent_card)
    routes += create_jsonrpc_routes(handler, rpc_url="/")
    app = Starlette(routes=routes)
    app = TraceparentMiddleware(app)
    uvicorn.run(app, host=host, port=port)


if __name__ == "__main__":
    main()

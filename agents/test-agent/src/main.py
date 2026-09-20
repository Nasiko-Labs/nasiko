"""
test — Nasiko Agent
Entry point: A2A JSON-RPC server exposing the echo agent (no LLM dependency).
"""

import json
import os
from pathlib import Path

from telemetry import init_telemetry

init_telemetry("test-agent")

import uvicorn
from a2a.helpers import (
    new_task_from_user_message,
    new_text_artifact_update_event,
    new_text_status_update_event,
)
from a2a.server.agent_execution import AgentExecutor, RequestContext
from a2a.server.events import EventQueue
from a2a.server.request_handlers import DefaultRequestHandler
from a2a.server.routes import create_agent_card_routes, create_jsonrpc_routes
from a2a.server.tasks import InMemoryTaskStore
from a2a.types import (
    AgentCapabilities,
    AgentCard,
    AgentInterface,
    AgentSkill,
    TaskState,
)
from starlette.applications import Starlette
from starlette.responses import JSONResponse
from starlette.routing import Route

from agent import run

_CARD_PATH = Path(__file__).parent.parent / "AgentCard.json"


class EchoExecutor(AgentExecutor):
    """Runs `agent.run` for each incoming A2A message."""

    async def execute(self, context: RequestContext, event_queue: EventQueue) -> None:
        query = context.get_user_input()
        task = context.current_task or new_task_from_user_message(context.message)
        await event_queue.enqueue_event(task)

        result = run(query)

        await event_queue.enqueue_event(
            new_text_artifact_update_event(
                task_id=task.id,
                context_id=task.context_id,
                name="echo",
                text=result,
            )
        )
        await event_queue.enqueue_event(
            new_text_status_update_event(
                task_id=task.id,
                context_id=task.context_id,
                state=TaskState.TASK_STATE_COMPLETED,
                text=result,
            )
        )

    async def cancel(self, context: RequestContext, event_queue: EventQueue) -> None:
        pass


def load_agent_card(url: str) -> AgentCard:
    """Build the SDK card from AgentCard.json — the file stays the single
    source of card data (it's what `nasiko validate` and publish read);
    only the serving URL is runtime information."""
    card = json.loads(_CARD_PATH.read_text())
    return AgentCard(
        name=card["name"],
        description=card["description"],
        version=card["version"],
        supported_interfaces=[AgentInterface(protocol_binding="JSONRPC", url=url)],
        default_input_modes=["text/plain"],
        default_output_modes=["text/plain"],
        capabilities=AgentCapabilities(streaming=True),
        skills=[
            AgentSkill(
                id=skill["id"],
                name=skill["name"],
                description=skill["description"],
                tags=skill.get("tags", []),
            )
            for skill in card.get("skills", [])
        ],
    )


async def health(_request):
    return JSONResponse({"status": "ok", "agent": "test"})


def main() -> None:
    host = "0.0.0.0"
    port = int(os.getenv("PORT", "8000"))
    agent_card = load_agent_card(os.getenv("HOST_OVERRIDE", f"http://{host}:{port}/"))

    handler = DefaultRequestHandler(
        agent_executor=EchoExecutor(),
        task_store=InMemoryTaskStore(),
        agent_card=agent_card,
    )
    routes = [Route("/health", health)]
    routes += create_agent_card_routes(agent_card)
    routes += create_jsonrpc_routes(handler, rpc_url="/")
    uvicorn.run(Starlette(routes=routes), host=host, port=port)


if __name__ == "__main__":
    main()

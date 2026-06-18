"""Echo agent — mirrors user input. No LLM, no HTTP calls."""
import logging
import os

import click
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

logging.basicConfig(level=logging.INFO)


class EchoExecutor(AgentExecutor):
    async def execute(self, context: RequestContext, event_queue: EventQueue) -> None:
        query = context.get_user_input()
        reply = f"Echo: {query}"

        task = context.current_task or new_task_from_user_message(context.message)
        await event_queue.enqueue_event(task)

        await event_queue.enqueue_event(
            new_text_status_update_event(
                task_id=task.id,
                context_id=task.context_id,
                state=TaskState.TASK_STATE_WORKING,
                text="Echoing...",
            )
        )

        await event_queue.enqueue_event(
            new_text_artifact_update_event(
                task_id=task.id,
                context_id=task.context_id,
                name="echo-result",
                text=reply,
            )
        )

        await event_queue.enqueue_event(
            new_text_status_update_event(
                task_id=task.id,
                context_id=task.context_id,
                state=TaskState.TASK_STATE_COMPLETED,
                text=reply,
            )
        )

    async def cancel(self, context: RequestContext, event_queue: EventQueue) -> None:
        pass


@click.command()
@click.option("--host", default="0.0.0.0")
@click.option("--port", default=int(os.environ.get("PORT", "8000")), type=int)
def main(host, port):
    agent_card = AgentCard(
        name="Echo Agent",
        description="Mirrors back user input. Useful for testing A2A connectivity.",
        supported_interfaces=[
            AgentInterface(protocol_binding="JSONRPC", url=f"http://{host}:{port}/"),
        ],
        version="1.0.0",
        default_input_modes=["text/plain"],
        default_output_modes=["text/plain"],
        capabilities=AgentCapabilities(streaming=True),
        skills=[
            AgentSkill(
                id="echo",
                name="Echo",
                description="Echoes back the user's input verbatim, prefixed with 'Echo: '",
                tags=["utility", "test"],
                examples=["Hello world", "Testing 123"],
            )
        ],
    )

    handler = DefaultRequestHandler(
        agent_executor=EchoExecutor(),
        task_store=InMemoryTaskStore(),
        agent_card=agent_card,
    )

    routes = []
    routes.extend(create_agent_card_routes(agent_card))
    routes.extend(create_jsonrpc_routes(handler, rpc_url="/"))

    app = Starlette(routes=routes)
    uvicorn.run(app, host=host, port=port)


if __name__ == "__main__":
    main()

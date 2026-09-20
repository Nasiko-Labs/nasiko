"""Currency converter agent — pure logic, no LLM, no HTTP calls."""
import logging
import os
import re

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

RATES = {
    "USD": 1.0, "EUR": 0.92, "GBP": 0.79, "JPY": 154.5, "INR": 83.4,
    "CAD": 1.36, "AUD": 1.53, "CHF": 0.88, "CNY": 7.24, "BRL": 4.97,
}

PATTERN = re.compile(r"(?i)(\d+(?:\.\d+)?)\s*([a-zA-Z]{3})\s+(?:to|in|into)\s+([a-zA-Z]{3})")


def convert(text: str) -> str:
    m = PATTERN.search(text)
    if not m:
        supported = ", ".join(sorted(RATES.keys()))
        return f"Please say something like '100 USD to EUR'. Supported: {supported}"

    amount = float(m.group(1))
    from_cur = m.group(2).upper()
    to_cur = m.group(3).upper()

    if from_cur not in RATES:
        return f"Unsupported currency: {from_cur}"
    if to_cur not in RATES:
        return f"Unsupported currency: {to_cur}"

    usd = amount / RATES[from_cur]
    result = usd * RATES[to_cur]
    rate = RATES[to_cur] / RATES[from_cur]
    return f"{amount:.2f} {from_cur} = {result:.2f} {to_cur} (Rate: 1 {from_cur} = {rate:.4f} {to_cur})"


class CurrencyExecutor(AgentExecutor):
    async def execute(self, context: RequestContext, event_queue: EventQueue) -> None:
        query = context.get_user_input()
        reply = convert(query)

        task = context.current_task or new_task_from_user_message(context.message)
        await event_queue.enqueue_event(task)

        await event_queue.enqueue_event(
            new_text_status_update_event(
                task_id=task.id,
                context_id=task.context_id,
                state=TaskState.TASK_STATE_WORKING,
                text="Converting...",
            )
        )

        await event_queue.enqueue_event(
            new_text_artifact_update_event(
                task_id=task.id,
                context_id=task.context_id,
                name="conversion-result",
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
        name="Currency Converter",
        description="Converts between currencies using fixed rates. No LLM needed.",
        supported_interfaces=[
            AgentInterface(protocol_binding="JSONRPC", url=f"http://{host}:{port}/"),
        ],
        version="1.0.0",
        default_input_modes=["text/plain"],
        default_output_modes=["text/plain"],
        capabilities=AgentCapabilities(streaming=True),
        skills=[
            AgentSkill(
                id="convert-currency",
                name="Convert Currency",
                description="Converts an amount from one currency to another.",
                tags=["finance", "conversion", "utility"],
                examples=["100 USD to EUR", "50 GBP to INR", "1000 JPY to USD"],
            )
        ],
    )

    handler = DefaultRequestHandler(
        agent_executor=CurrencyExecutor(),
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

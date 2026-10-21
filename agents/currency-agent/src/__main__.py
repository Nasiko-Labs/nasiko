"""Currency converter agent.

OpenAI parses natural-language conversion requests. Fixed USD-based rates then
compute the result so the math stays deterministic.
"""
import json
import logging
import os

from dotenv import load_dotenv

load_dotenv()
logging.basicConfig(level=logging.INFO)
logger = logging.getLogger(__name__)

# Instrumentation must initialize before a2a-sdk (and anything it imports,
# e.g. Starlette) is imported below: OTel's Starlette instrumentor patches by
# rebinding `starlette.applications.Starlette` to an instrumented subclass, so
# any module that already did `from starlette.applications import Starlette`
# keeps its original, un-instrumented reference forever — no incoming
# traceparent gets extracted, and every request starts an orphan root trace
# instead of joining the platform's session trace.
from telemetry import init_telemetry

init_telemetry(os.environ.get("OTEL_SERVICE_NAME", "currency-agent"))

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
from openai import AsyncOpenAI
from starlette.applications import Starlette

RATES = {
    "USD": 1.0,
    "EUR": 0.92,
    "GBP": 0.79,
    "JPY": 154.5,
    "INR": 83.4,
    "CAD": 1.36,
    "AUD": 1.53,
    "CHF": 0.88,
    "CNY": 7.24,
    "BRL": 4.97,
}

SUPPORTED = ", ".join(sorted(RATES))

_SYSTEM_PROMPT = f"""\
You extract currency conversions from user text.
Reply with JSON only, no markdown:
{{"amount": 100, "from": "USD", "to": "EUR"}}
Use ISO 4217 codes. Supported codes: {SUPPORTED}.
If the request is not a conversion, reply:
{{"error": "short explanation"}}
"""


def convert(amount: float, from_cur: str, to_cur: str) -> str:
    from_cur = from_cur.upper()
    to_cur = to_cur.upper()
    if from_cur not in RATES:
        return f"Unsupported currency: {from_cur}. Supported: {SUPPORTED}"
    if to_cur not in RATES:
        return f"Unsupported currency: {to_cur}. Supported: {SUPPORTED}"

    usd = amount / RATES[from_cur]
    result = usd * RATES[to_cur]
    rate = RATES[to_cur] / RATES[from_cur]
    return (
        f"{amount:.2f} {from_cur} = {result:.2f} {to_cur} "
        f"(Rate: 1 {from_cur} = {rate:.4f} {to_cur})"
    )


class CurrencyExecutor(AgentExecutor):
    def __init__(self, llm: AsyncOpenAI, model: str):
        self.llm = llm
        self.model = model

    async def execute(self, context: RequestContext, event_queue: EventQueue) -> None:
        query = context.get_user_input()
        task = context.current_task or new_task_from_user_message(context.message)
        await event_queue.enqueue_event(task)

        await event_queue.enqueue_event(
            new_text_status_update_event(
                task_id=task.id,
                context_id=task.context_id,
                state=TaskState.TASK_STATE_WORKING,
                text="Parsing conversion request...",
            )
        )

        try:
            reply = await self._convert_query(query)
        except Exception as exc:
            logger.exception("Currency conversion failed")
            await event_queue.enqueue_event(
                new_text_status_update_event(
                    task_id=task.id,
                    context_id=task.context_id,
                    state=TaskState.TASK_STATE_FAILED,
                    text=f"Error: {exc}",
                )
            )
            return

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

    async def _convert_query(self, query: str) -> str:
        parsed = await self._parse_query(query)
        if error := parsed.get("error"):
            return f"{error} Try something like '100 USD to EUR'. Supported: {SUPPORTED}"

        try:
            amount = float(parsed["amount"])
            from_cur = str(parsed["from"])
            to_cur = str(parsed["to"])
        except (KeyError, TypeError, ValueError):
            return f"Could not parse a conversion. Try '100 USD to EUR'. Supported: {SUPPORTED}"

        return convert(amount, from_cur, to_cur)

    async def _parse_query(self, query: str) -> dict:
        resp = await self.llm.chat.completions.create(
            model=self.model,
            temperature=0,
            response_format={"type": "json_object"},
            messages=[
                {"role": "system", "content": _SYSTEM_PROMPT},
                {"role": "user", "content": query},
            ],
        )
        text = resp.choices[0].message.content or "{}"
        try:
            data = json.loads(text)
        except json.JSONDecodeError:
            return {"error": "The model did not return a valid conversion."}
        return data if isinstance(data, dict) else {"error": "Unexpected model response."}

    async def cancel(self, context: RequestContext, event_queue: EventQueue) -> None:
        pass


@click.command()
@click.option("--host", default="0.0.0.0")
@click.option("--port", default=int(os.environ.get("PORT", "8000")), type=int)
def main(host, port):
    if not os.getenv("OPENAI_API_KEY"):
        logger.error("OPENAI_API_KEY is not set")
        raise SystemExit(1)

    llm = AsyncOpenAI(
        api_key=os.environ.get("OPENAI_API_KEY"),
        base_url=os.environ.get("OPENAI_BASE_URL") or None,
    )
    model = os.environ.get("OPENAI_MODEL", "gpt-4o-mini")
    agent_url = os.getenv("HOST_OVERRIDE", f"http://{host}:{port}/")

    agent_card = AgentCard(
        name="currency-agent",
        description=(
            "Converts amounts between currencies. OpenAI parses the request; "
            "fixed USD-based rates compute the result."
        ),
        supported_interfaces=[
            AgentInterface(
                protocol_binding="JSONRPC",
                protocol_version="1.0",
                url=agent_url,
            )
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
                tags=["finance", "conversion", "utility", "openai"],
                examples=["100 USD to EUR", "50 GBP to INR", "How much is 1000 yen in dollars?"],
            )
        ],
    )

    handler = DefaultRequestHandler(
        agent_executor=CurrencyExecutor(llm, model),
        task_store=InMemoryTaskStore(),
        agent_card=agent_card,
    )

    routes = []
    routes.extend(create_agent_card_routes(agent_card))
    routes.extend(create_jsonrpc_routes(handler, rpc_url="/"))

    app = Starlette(routes=routes)
    logger.info("Currency Agent listening on %s:%s", host, port)
    uvicorn.run(app, host=host, port=port)


if __name__ == "__main__":
    main()

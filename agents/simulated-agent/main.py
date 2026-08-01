"""Simulated agent — stands in for a real LLM-backed agent during load testing.

No LLM, no outbound HTTP calls: replies with random lorem-ipsum-style text,
but paced with randomized per-chunk latency (a "thinking" delay before the
first token, then a delay between each streamed chunk) so it looks like a
real agent's response-time distribution rather than an instant echo. This
lets a benchmark isolate control-plane/proxy overhead from LLM latency while
still exercising the full A2A streaming code path.

Tunable via env vars — see `_env_float`/`_env_int` calls below — so the same
binary can simulate a fast agent or a slow one without a rebuild.
"""
import logging
import os
import random
import asyncio

logging.basicConfig(level=logging.INFO)

# Instrumentation must initialize before a2a-sdk (and anything it imports,
# e.g. Starlette) is imported below: OTel's Starlette instrumentor patches by
# rebinding `starlette.applications.Starlette` to an instrumented subclass, so
# any module that already did `from starlette.applications import Starlette`
# keeps its original, un-instrumented reference forever — no incoming
# traceparent gets extracted, and every request starts an orphan root trace
# instead of joining the platform's session trace.
try:  # OTel bootstrap — telemetry.py must ship alongside main.py
    from telemetry import init_telemetry

    init_telemetry()
except ImportError:
    logging.getLogger(__name__).warning("telemetry.py not found — OTel telemetry disabled")

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

_LOREM_WORDS = (
    "lorem ipsum dolor sit amet consectetur adipiscing elit sed do eiusmod "
    "tempor incididunt ut labore et dolore magna aliqua ut enim ad minim "
    "veniam quis nostrud exercitation ullamco laboris nisi ut aliquip ex ea "
    "commodo consequat duis aute irure dolor in reprehenderit voluptate velit "
    "esse cillum dolore eu fugiat nulla pariatur excepteur sint occaecat "
    "cupidatat non proident sunt in culpa qui officia deserunt mollit anim "
    "id est laborum"
).split()


def _env_float(name: str, default: float) -> float:
    try:
        return float(os.environ.get(name, default))
    except ValueError:
        return default


def _env_int(name: str, default: int) -> int:
    try:
        return int(os.environ.get(name, default))
    except ValueError:
        return default


# "Thinking" delay before the first chunk — models time-to-first-token.
THINK_MIN_MS = _env_float("SIM_THINK_MIN_MS", 200)
THINK_MAX_MS = _env_float("SIM_THINK_MAX_MS", 1500)
# Delay between streamed chunks — models per-token/per-chunk generation time.
CHUNK_MIN_MS = _env_float("SIM_CHUNK_MIN_MS", 30)
CHUNK_MAX_MS = _env_float("SIM_CHUNK_MAX_MS", 200)
# Response length, in words, drawn uniformly from this range.
WORDS_MIN = _env_int("SIM_WORDS_MIN", 20)
WORDS_MAX = _env_int("SIM_WORDS_MAX", 120)
# Words streamed per chunk (a stand-in for tokens-per-flush).
WORDS_PER_CHUNK = _env_int("SIM_WORDS_PER_CHUNK", 4)


def _random_reply_words() -> list[str]:
    n = random.randint(WORDS_MIN, WORDS_MAX)
    return [random.choice(_LOREM_WORDS) for _ in range(n)]


async def _sleep_ms(min_ms: float, max_ms: float) -> None:
    await asyncio.sleep(random.uniform(min_ms, max_ms) / 1000)


class SimulatedExecutor(AgentExecutor):
    async def execute(self, context: RequestContext, event_queue: EventQueue) -> None:
        task = context.current_task or new_task_from_user_message(context.message)
        await event_queue.enqueue_event(task)

        await event_queue.enqueue_event(
            new_text_status_update_event(
                task_id=task.id,
                context_id=task.context_id,
                state=TaskState.TASK_STATE_WORKING,
                text="thinking...",
            )
        )
        await _sleep_ms(THINK_MIN_MS, THINK_MAX_MS)

        words = _random_reply_words()
        chunks = [
            " ".join(words[i : i + WORDS_PER_CHUNK])
            for i in range(0, len(words), WORDS_PER_CHUNK)
        ]
        for chunk in chunks:
            await event_queue.enqueue_event(
                new_text_artifact_update_event(
                    task_id=task.id,
                    context_id=task.context_id,
                    name="echo-result",
                    text=chunk + " ",
                )
            )
            await _sleep_ms(CHUNK_MIN_MS, CHUNK_MAX_MS)

        await event_queue.enqueue_event(
            new_text_status_update_event(
                task_id=task.id,
                context_id=task.context_id,
                state=TaskState.TASK_STATE_COMPLETED,
                text=" ".join(words),
            )
        )

    async def cancel(self, context: RequestContext, event_queue: EventQueue) -> None:
        pass


@click.command()
@click.option("--host", default="0.0.0.0")
@click.option("--port", default=int(os.environ.get("PORT", "8000")), type=int)
def main(host, port):
    agent_card = AgentCard(
        name="Simulated Agent",
        description=(
            "Replies with randomized lorem-ipsum text at configurable, "
            "variable latency. Stands in for a real LLM-backed agent during "
            "control-plane load testing — no LLM, no outbound HTTP calls."
        ),
        supported_interfaces=[
            AgentInterface(protocol_binding="JSONRPC", url=f"http://{host}:{port}/"),
        ],
        version="1.0.0",
        default_input_modes=["text/plain"],
        default_output_modes=["text/plain"],
        capabilities=AgentCapabilities(streaming=True),
        skills=[
            AgentSkill(
                id="simulate",
                name="Simulate",
                description="Streams randomized lorem-ipsum text back at variable latency",
                tags=["utility", "benchmark", "load-test"],
                examples=["Hello world", "Testing 123"],
            )
        ],
    )

    handler = DefaultRequestHandler(
        agent_executor=SimulatedExecutor(),
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

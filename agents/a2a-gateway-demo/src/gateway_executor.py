import logging

import httpx
from a2a.server.agent_execution import AgentExecutor
from a2a.server.agent_execution.context import RequestContext
from a2a.server.events.event_queue import EventQueue
from a2a.server.tasks import TaskUpdater
from a2a.types import AgentCard, TaskState, TextPart, UnsupportedOperationError
from a2a.utils.errors import ServerError
from openai import AsyncOpenAI

# ─────────────────────────────────────────────────────────────────────────────
# W3C traceparent propagation
# We attach the current OpenTelemetry context to every outbound request so the
# gateway span stitches into the agent's trace in Phoenix. Without this, the
# gateway span shows up as an orphan root.
# ─────────────────────────────────────────────────────────────────────────────
try:
    from opentelemetry.propagate import inject as _otel_inject

    def _build_http_client() -> httpx.AsyncClient:
        def _propagate(request: httpx.Request) -> None:
            headers: dict[str, str] = {}
            _otel_inject(headers)
            for k, v in headers.items():
                request.headers.setdefault(k, v)

        return httpx.AsyncClient(event_hooks={"request": [_propagate]})
except ImportError:  # pragma: no cover — OTEL not installed
    def _build_http_client() -> httpx.AsyncClient:
        return httpx.AsyncClient()


logger = logging.getLogger(__name__)

SYSTEM_PROMPT = (
    "You are a helpful assistant deployed on the Nasiko AI platform. "
    "Answer questions clearly and concisely."
)


class GatewayAgentExecutor(AgentExecutor):
    """AgentExecutor that calls the LLM via the Nasiko gateway (no direct provider key)."""

    def __init__(self, card: AgentCard, client: AsyncOpenAI, model: str = "gpt-4o-mini"):
        self._card = card
        self.client = client
        self.model = model

    async def execute(self, context: RequestContext, event_queue: EventQueue) -> None:
        updater = TaskUpdater(event_queue, context.task_id, context.context_id)
        if not context.current_task:
            await updater.submit()
        await updater.start_work()

        user_text = "".join(
            part.root.text
            for part in context.message.parts
            if isinstance(part.root, TextPart)
        )

        await updater.update_status(
            TaskState.working,
            message=updater.new_agent_message([TextPart(text="Calling LLM via gateway…")]),
        )

        try:
            response = await self.client.chat.completions.create(
                model=self.model,
                messages=[
                    {"role": "system", "content": SYSTEM_PROMPT},
                    {"role": "user", "content": user_text},
                ],
                temperature=0.7,
                max_tokens=1024,
            )
            answer = response.choices[0].message.content or ""
            await updater.add_artifact([TextPart(text=answer)])
            await updater.complete()
        except Exception as exc:
            logger.error(f"Gateway call failed: {exc}")
            await updater.add_artifact(
                [TextPart(text=f"Error calling LLM gateway: {exc}")]
            )
            await updater.complete()

    async def cancel(self, context: RequestContext, event_queue: EventQueue) -> None:
        raise ServerError(error=UnsupportedOperationError())

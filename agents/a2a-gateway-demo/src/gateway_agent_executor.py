import logging
import os

from a2a.server.agent_execution import AgentExecutor
from a2a.server.agent_execution.context import RequestContext
from a2a.server.events.event_queue import EventQueue
from a2a.server.tasks import TaskUpdater
from a2a.types import (
    AgentCard,
    TaskState,
    TextPart,
    UnsupportedOperationError,
)
from a2a.utils.errors import ServerError
from openai import AsyncOpenAI

logger = logging.getLogger(__name__)


class GatewayAgentExecutor(AgentExecutor):
    """AgentExecutor that routes all LLM calls through the platform gateway.

    ZERO provider keys in source. Both credentials are injected at deploy
    time by the Nasiko orchestrator (_deploy_agent_container):
      - OPENAI_BASE_URL  → points at llm-gateway:4000
      - OPENAI_API_KEY   → per-agent virtual key minted by LiteLLM
    """

    def __init__(self, card: AgentCard) -> None:
        self._card = card
        gateway_url = os.environ.get("OPENAI_BASE_URL") or os.environ.get(
            "LLM_GATEWAY_URL"
        )
        virtual_key = os.environ.get("OPENAI_API_KEY")

        if not gateway_url or not virtual_key:
            raise ValueError(
                "GatewayAgentExecutor requires OPENAI_BASE_URL (or LLM_GATEWAY_URL) "
                "and OPENAI_API_KEY to be set by the Nasiko orchestrator. "
                "Do not hardcode provider keys — use the platform LLM gateway."
            )

        self.client = AsyncOpenAI(base_url=gateway_url, api_key=virtual_key)

    async def execute(
        self,
        context: RequestContext,
        event_queue: EventQueue,
    ) -> None:
        updater = TaskUpdater(event_queue, context.task_id, context.context_id)
        if not context.current_task:
            await updater.submit()
        await updater.start_work()

        message_text = ""
        for part in context.message.parts:
            if isinstance(part.root, TextPart):
                message_text += part.root.text

        try:
            response = await self.client.chat.completions.create(
                model="default-model",
                messages=[
                    {
                        "role": "system",
                        "content": (
                            "You are a helpful assistant running through the "
                            "Nasiko platform LLM gateway. Answer concisely."
                        ),
                    },
                    {"role": "user", "content": message_text},
                ],
                max_tokens=512,
            )
            reply = response.choices[0].message.content or ""
        except Exception as exc:
            logger.error("Gateway LLM call failed: %s", exc)
            reply = f"Gateway error: {exc}"

        await updater.add_artifact([TextPart(text=reply)])
        await updater.complete()
        logger.debug("[GatewayDemo] execute complete")

    async def cancel(
        self, context: RequestContext, event_queue: EventQueue
    ) -> None:
        raise ServerError(error=UnsupportedOperationError())

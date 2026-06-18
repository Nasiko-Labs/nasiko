import logging
import uuid

from a2a.server.agent_execution import AgentExecutor, RequestContext
from a2a.server.events import EventQueue
from a2a.types import (
    Artifact,
    InternalError,
    Part,
    TaskState,
    TaskStatus,
    TaskStatusUpdateEvent,
    TaskArtifactUpdateEvent,
    TextPart,
    UnsupportedOperationError,
)
from a2a.utils.errors import ServerError

from agent import ResearchAgent

logger = logging.getLogger(__name__)


class ResearchAgentExecutor(AgentExecutor):
    def __init__(self):
        self.agent = ResearchAgent()

    async def execute(self, context: RequestContext, event_queue: EventQueue) -> None:
        query = context.get_user_input()

        await event_queue.enqueue_event(
            TaskStatusUpdateEvent(
                task_id=context.task_id,
                context_id=context.context_id,
                status=TaskStatus(state=TaskState.working),
                final=False,
            )
        )

        artifact_id = str(uuid.uuid4())
        first_chunk = True

        try:
            async for chunk in self.agent.invoke_streaming(query, context.context_id):
                await event_queue.enqueue_event(
                    TaskArtifactUpdateEvent(
                        task_id=context.task_id,
                        context_id=context.context_id,
                        artifact=Artifact(
                            artifact_id=artifact_id,
                            parts=[Part(root=TextPart(text=chunk))],
                        ),
                        append=not first_chunk,
                    )
                )
                first_chunk = False
        except Exception as e:
            logger.error(f"Streaming error: {e}")
            raise ServerError(error=InternalError()) from e

        await event_queue.enqueue_event(
            TaskStatusUpdateEvent(
                task_id=context.task_id,
                context_id=context.context_id,
                status=TaskStatus(state=TaskState.completed),
                final=True,
            )
        )

    async def cancel(self, context: RequestContext, event_queue: EventQueue) -> None:
        raise ServerError(error=UnsupportedOperationError())

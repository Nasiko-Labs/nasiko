import logging
import uuid

from a2a.helpers import new_task_from_user_message
from a2a.server.agent_execution import AgentExecutor, RequestContext
from a2a.server.events import EventQueue
from a2a.types import (
    Artifact,
    Part,
    TaskState,
    TaskStatus,
    TaskStatusUpdateEvent,
    TaskArtifactUpdateEvent,
)

from agent import ResearchAgent

logger = logging.getLogger(__name__)


class ResearchAgentExecutor(AgentExecutor):
    def __init__(self):
        self.agent = ResearchAgent()

    async def execute(self, context: RequestContext, event_queue: EventQueue) -> None:
        query = context.get_user_input()

        task = context.current_task or new_task_from_user_message(context.message)
        await event_queue.enqueue_event(task)

        await event_queue.enqueue_event(
            TaskStatusUpdateEvent(
                task_id=task.id,
                context_id=task.context_id,
                status=TaskStatus(state=TaskState.TASK_STATE_WORKING),
            )
        )

        artifact_id = str(uuid.uuid4())
        chunks = []

        try:
            async for chunk in self.agent.invoke_streaming(query, context.context_id):
                chunks.append(chunk)
                await event_queue.enqueue_event(
                    TaskArtifactUpdateEvent(
                        task_id=task.id,
                        context_id=task.context_id,
                        artifact=Artifact(
                            artifact_id=artifact_id,
                            parts=[Part(text=chunk)],
                        ),
                        append=len(chunks) > 1,
                        last_chunk=False,
                    )
                )
        except Exception as e:
            logger.error(f"Streaming error: {e}")
            await event_queue.enqueue_event(
                TaskStatusUpdateEvent(
                    task_id=task.id,
                    context_id=task.context_id,
                    status=TaskStatus(state=TaskState.TASK_STATE_FAILED),
                )
            )
            return

        # Mark final chunk
        if chunks:
            await event_queue.enqueue_event(
                TaskArtifactUpdateEvent(
                    task_id=task.id,
                    context_id=task.context_id,
                    artifact=Artifact(artifact_id=artifact_id, parts=[]),
                    append=True,
                    last_chunk=True,
                )
            )

        await event_queue.enqueue_event(
            TaskStatusUpdateEvent(
                task_id=task.id,
                context_id=task.context_id,
                status=TaskStatus(state=TaskState.TASK_STATE_COMPLETED),
            )
        )

    async def cancel(self, context: RequestContext, event_queue: EventQueue) -> None:
        pass

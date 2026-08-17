import logging

from a2a.helpers import (
    new_task_from_user_message,
    new_text_artifact_update_event,
    new_text_status_update_event,
)
from a2a.server.agent_execution import AgentExecutor, RequestContext
from a2a.server.events import EventQueue
from a2a.types import TaskState

from agent import SynthesizerAgent

logger = logging.getLogger(__name__)


class SynthesizerAgentExecutor(AgentExecutor):
    def __init__(self):
        self.agent = SynthesizerAgent()

    async def execute(self, context: RequestContext, event_queue: EventQueue) -> None:
        query = context.get_user_input()
        task = context.current_task or new_task_from_user_message(context.message)
        await event_queue.enqueue_event(task)

        try:
            async for item in self.agent.stream(query, task.context_id):
                if not item["is_task_complete"] and not item["require_user_input"]:
                    await event_queue.enqueue_event(
                        new_text_status_update_event(
                            task_id=task.id,
                            context_id=task.context_id,
                            state=TaskState.TASK_STATE_WORKING,
                            text=item["content"],
                        )
                    )
                elif item["require_user_input"]:
                    await event_queue.enqueue_event(
                        new_text_status_update_event(
                            task_id=task.id,
                            context_id=task.context_id,
                            state=TaskState.TASK_STATE_INPUT_REQUIRED,
                            text=item["content"],
                        )
                    )
                    return
                else:
                    await event_queue.enqueue_event(
                        new_text_artifact_update_event(
                            task_id=task.id,
                            context_id=task.context_id,
                            name="synthesis_result",
                            text=item["content"],
                        )
                    )
                    await event_queue.enqueue_event(
                        new_text_status_update_event(
                            task_id=task.id,
                            context_id=task.context_id,
                            state=TaskState.TASK_STATE_COMPLETED,
                            text=item["content"],
                        )
                    )
                    return
        except Exception as e:
            logger.exception("Synthesis failed")
            await event_queue.enqueue_event(
                new_text_status_update_event(
                    task_id=task.id,
                    context_id=task.context_id,
                    state=TaskState.TASK_STATE_FAILED,
                    text=f"Error: {e}",
                )
            )

    async def cancel(self, context: RequestContext, event_queue: EventQueue) -> None:
        pass

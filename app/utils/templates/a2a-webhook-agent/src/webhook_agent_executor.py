import logging
from typing import Any

from a2a.server.agent_execution import AgentExecutor
from a2a.server.agent_execution.context import RequestContext
from a2a.server.events.event_queue import EventQueue
from a2a.server.tasks import TaskUpdater
from a2a.types import (
    AgentCard,
    TextPart,
    UnsupportedOperationError,
)
from a2a.utils.errors import ServerError
from webhook_agent import WebhookAgent


logger = logging.getLogger(__name__)
logger.setLevel(logging.DEBUG)


class WebhookAgentExecutor(AgentExecutor):
    """An AgentExecutor that forwards messages to webhook endpoints."""

    def __init__(
        self,
        card: AgentCard,
        webhook_agent: WebhookAgent,
    ):
        self._card = card
        self.webhook_agent = webhook_agent
        logger.info(f"WebhookAgentExecutor initialized")

    async def _process_request(
        self,
        message_text: str,
        request_id: str,
        task_updater: TaskUpdater,
    ) -> None:
        """
        Process the incoming message by forwarding it to the webhook
        """
        try:
            logger.info(f"Processing message: {message_text}")
            logger.info(f"Using request ID as session ID: {request_id}")

            # Send message to webhook using request ID as session ID
            webhook_response = await self.webhook_agent.send_message(
                session_id=request_id, 
                message=message_text
            )
            
            logger.info(f"Received webhook response: {webhook_response[:200]}...")

            # Create response artifact
            response_part = TextPart(text=webhook_response)
            
            # Add the artifact and complete the task
            await task_updater.add_artifact([response_part])
            await task_updater.complete()

        except Exception as e:
            logger.error(f"Error processing request: {str(e)}")
            error_message = f"Failed to process webhook request: {str(e)}"
            
            # Add error artifact and complete
            await task_updater.add_artifact([TextPart(text=error_message)])
            await task_updater.complete()

    async def execute(
        self,
        context: RequestContext,
        event_queue: EventQueue,
    ):
        """Execute the webhook agent request"""
        # Run the agent until complete
        updater = TaskUpdater(event_queue, context.task_id, context.context_id)
        
        # Immediately notify that the task is submitted.
        if not context.current_task:
            await updater.submit()
        await updater.start_work()

        # Extract text from message parts
        message_text = ''
        for part in context.message.parts:
            if isinstance(part.root, TextPart):
                message_text += part.root.text

        # Use A2A context_id as session ID for consistency within A2A ecosystem
        # This ensures uniform session tracking across all A2A agents
        session_id = context.context_id
        logger.info(f"Using A2A context_id as session ID: {session_id}")
        await self._process_request(message_text, session_id, updater)
        logger.debug('[Webhook Agent] execute exiting')

    async def cancel(self, context: RequestContext, event_queue: EventQueue):
        """Cancel the webhook agent request"""
        # Ideally: kill any ongoing tasks.
        raise ServerError(error=UnsupportedOperationError())
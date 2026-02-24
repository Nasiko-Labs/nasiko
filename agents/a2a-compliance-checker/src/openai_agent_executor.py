import json
import logging

from typing import Any

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
logger.setLevel(logging.DEBUG)


class OpenAIAgentExecutor(AgentExecutor):
    """An AgentExecutor that runs an OpenAI-based Compliance Agent."""

    def __init__(
        self,
        card: AgentCard,
        tools: dict[str, Any],
        api_key: str,
        system_prompt: str,
    ):
        self._card = card
        self.tools = tools
        self.client = AsyncOpenAI(
            api_key=api_key,
        )
        self.model = 'gpt-4o'
        self.system_prompt = system_prompt

    async def _process_request(
        self,
        message_text: str,
        context: RequestContext,
        task_updater: TaskUpdater,
    ) -> None:
        messages = [
            {'role': 'system', 'content': self.system_prompt},
            {'role': 'user', 'content': message_text},
        ]

        # Convert tools to OpenAI format
        openai_tools = []
        for tool_name, tool_instance in self.tools.items():
            if hasattr(tool_instance, tool_name):
                func = getattr(tool_instance, tool_name)
                # Extract function schema from the method
                schema = self._extract_function_schema(func)
                openai_tools.append({'type': 'function', 'function': schema})

        max_iterations = 10
        iteration = 0

        while iteration < max_iterations:
            iteration += 1

            try:
                # Make API call to OpenAI
                response = await self.client.chat.completions.create(
                    model=self.model,
                    messages=messages,
                    tools=openai_tools if openai_tools else None,
                    tool_choice='auto' if openai_tools else None,
                    temperature=0.1,
                    max_tokens=4000,
                )

                message = response.choices[0].message

                # Add assistant's response to messages
                messages.append(
                    {
                        'role': 'assistant',
                        'content': message.content,
                        'tool_calls': message.tool_calls,
                    }
                )

                # Check if there are tool calls to execute
                if message.tool_calls:
                    # Execute tool calls
                    for tool_call in message.tool_calls:
                        function_name = tool_call.function.name
                        function_args = json.loads(tool_call.function.arguments)

                        logger.debug(
                            f'Calling function: {function_name} with args: {function_args}'
                        )

                        # Execute the function
                        if function_name in self.tools:
                            tool_instance = self.tools[function_name]
                            # Get the method from the instance
                            if hasattr(tool_instance, function_name):
                                method = getattr(tool_instance, function_name)
                                result = method(**function_args)
                            else:
                                result = {
                                    'error': f'Method {function_name} not found on tool instance'
                                }
                        else:
                            result = {
                                'error': f'Function {function_name} not found'
                            }

                        # Serialize result properly - handle Pydantic models
                        if hasattr(result, 'model_dump'):
                            # It's a Pydantic model, use model_dump() to convert to dict
                            result_json = json.dumps(result.model_dump())
                        elif isinstance(result, dict):
                            # It's a regular dict
                            result_json = json.dumps(result)
                        else:
                            # Convert to string as fallback
                            result_json = str(result)

                        # Add tool result to messages
                        messages.append(
                            {
                                'role': 'tool',
                                'tool_call_id': tool_call.id,
                                'content': result_json,
                            }
                        )

                    # Send update to show we're processing
                    await task_updater.update_status(
                        TaskState.working,
                        message=task_updater.new_agent_message(
                            [TextPart(text='Processing tool calls...')]
                        ),
                    )

                    # Continue the loop to get the final response
                    continue
                # No more tool calls, this is the final response
                if message.content:
                    parts = [TextPart(text=message.content)]
                    logger.debug(f'Yielding final response: {parts}')
                    await task_updater.add_artifact(parts)
                    await task_updater.complete()
                break

            except Exception as e:
                logger.error(f'Error in OpenAI API call: {e}')
                error_parts = [
                    TextPart(
                        text=f'Sorry, an error occurred while processing the request: {e!s}'
                    )
                ]
                await task_updater.add_artifact(error_parts)
                await task_updater.complete()
                break

        if iteration >= max_iterations:
            error_parts = [
                TextPart(
                    text='Sorry, the request has exceeded the maximum number of iterations.'
                )
            ]
            await task_updater.add_artifact(error_parts)
            await task_updater.complete()

    def _extract_function_schema(self, func):
        """Extract OpenAI function schema from a Python function"""
        import inspect

        # Get function signature
        sig = inspect.signature(func)

        # Get docstring
        docstring = inspect.getdoc(func) or ''

        # Extract description and parameter info from docstring
        lines = docstring.split('\n')
        description = lines[0] if lines else func.__name__

        # Build parameters schema
        properties = {}
        required = []

        for param_name, param in sig.parameters.items():
            param_type = 'string'  # Default type
            param_description = f'Parameter {param_name}'

            # Try to infer type from annotation
            if param.annotation != inspect.Parameter.empty:
                if param.annotation == int:
                    param_type = 'integer'
                elif param.annotation == float:
                    param_type = 'number'
                elif param.annotation == bool:
                    param_type = 'boolean'
                elif param.annotation == list:
                    param_type = 'array'
                elif param.annotation == dict:
                    param_type = 'object'

            # Check if parameter has default value
            if param.default == inspect.Parameter.empty:
                required.append(param_name)

            properties[param_name] = {
                'type': param_type,
                'description': param_description,
            }

        return {
            'name': func.__name__,
            'description': description,
            'parameters': {
                'type': 'object',
                'properties': properties,
                'required': required,
            },
        }

    async def execute(
        self,
        context: RequestContext,
        event_queue: EventQueue,
    ):
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

        await self._process_request(message_text, context, updater)
        logger.debug('[Compliance Agent] execute exiting')

    async def cancel(self, context: RequestContext, event_queue: EventQueue):
        # Ideally: kill any ongoing tasks.
        raise ServerError(error=UnsupportedOperationError())
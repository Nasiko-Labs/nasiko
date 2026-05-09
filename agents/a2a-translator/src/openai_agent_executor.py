import json
import logging
import inspect
import re
import time

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

# Regex for simple "translate X to Y" patterns — matched → fast path
_FAST_PATH_RE = re.compile(
    r'(?:translate|convert)\s+["\']?(.+?)["\']?\s+(?:to|into)\s+(\w+)',
    re.IGNORECASE,
)


def _timing_footer(elapsed_s: float, fast_path: bool) -> str:
    label = "fast-path" if fast_path else "full-agent"
    return f"\n\n---\n*Translator ({label}): {elapsed_s * 1000:.0f} ms*"


class OpenAIAgentExecutor(AgentExecutor):
    """An AgentExecutor that runs an OpenAI-based Agent."""

    def __init__(
        self,
        card: AgentCard,
        tools: dict[str, Any],
        api_key: str,
        system_prompt: str,
        base_url: str | None = None,
        model: str = "gpt-4o",
    ):
        self._card = card
        self.tools = tools
        self.client = AsyncOpenAI(
            api_key=api_key,
            base_url=base_url,
            timeout=15.0,   # hard 15-second ceiling on every API call
        )
        self.model = model
        self.system_prompt = system_prompt

    # ── Fast path ─────────────────────────────────────────────────────────────

    async def _fast_translate(
        self,
        text: str,
        target_lang: str,
        task_updater: TaskUpdater,
    ) -> bool:
        """
        Direct single-shot translation for simple patterns.
        Returns True if handled, False to fall through to full agent.
        """
        t0 = time.perf_counter()
        try:
            response = await self.client.chat.completions.create(
                model=self.model,
                messages=[
                    {
                        "role": "system",
                        "content": (
                            f"You are a professional translator. "
                            f"Translate the user's text to {target_lang}. "
                            "Reply with only the translated text, nothing else."
                        ),
                    },
                    {"role": "user", "content": text},
                ],
                temperature=0.1,
                max_tokens=1024,
            )
            content = response.choices[0].message.content or ""
            elapsed = time.perf_counter() - t0
            content += _timing_footer(elapsed, fast_path=True)
            await task_updater.add_artifact([TextPart(text=content)])
            await task_updater.complete()
            logger.debug(f"Fast-path translation done in {elapsed*1000:.0f}ms")
            return True
        except Exception as e:
            logger.warning(f"Fast-path failed, falling back to full agent: {e}")
            return False

    # ── Full agent loop ───────────────────────────────────────────────────────

    async def _process_request(
        self,
        message_text: str,
        context: RequestContext,
        task_updater: TaskUpdater,
    ) -> None:
        t0 = time.perf_counter()

        # Fast path: simple "translate X to Y" pattern
        m = _FAST_PATH_RE.search(message_text)
        if m:
            text_to_translate = m.group(1).strip()
            target_language   = m.group(2).strip()
            handled = await self._fast_translate(text_to_translate, target_language, task_updater)
            if handled:
                return

        # Full agentic loop
        messages = [
            {"role": "system", "content": self.system_prompt},
            {"role": "user", "content": message_text},
        ]

        openai_tools = []
        for tool_name, tool_instance in self.tools.items():
            if hasattr(tool_instance, tool_name):
                func = getattr(tool_instance, tool_name)
                schema = self._extract_function_schema(func)
                openai_tools.append({"type": "function", "function": schema})

        max_iterations = 10
        iteration = 0

        while iteration < max_iterations:
            iteration += 1

            try:
                response = await self.client.chat.completions.create(
                    model=self.model,
                    messages=messages,
                    tools=openai_tools if openai_tools else None,
                    tool_choice="auto" if openai_tools else None,
                    temperature=0.1,
                    max_tokens=4000,
                )

                message = response.choices[0].message

                messages.append(
                    {
                        "role": "assistant",
                        "content": message.content,
                        "tool_calls": message.tool_calls,
                    }
                )

                if message.tool_calls:
                    for tool_call in message.tool_calls:
                        function_name = tool_call.function.name
                        function_args = json.loads(tool_call.function.arguments)

                        logger.debug(
                            f"Calling function: {function_name} with args: {function_args}"
                        )

                        if function_name in self.tools:
                            tool_instance = self.tools[function_name]
                            if hasattr(tool_instance, function_name):
                                method = getattr(tool_instance, function_name)
                                result = method(**function_args)
                                if inspect.iscoroutine(result):
                                    result = await result
                            else:
                                result = {
                                    "error": f"Method {function_name} not found on tool instance"
                                }
                        else:
                            result = {"error": f"Function {function_name} not found"}

                        if hasattr(result, "model_dump"):
                            result_json = json.dumps(result.model_dump())
                        elif isinstance(result, dict):
                            result_json = json.dumps(result)
                        else:
                            result_json = str(result)

                        messages.append(
                            {
                                "role": "tool",
                                "tool_call_id": tool_call.id,
                                "content": result_json,
                            }
                        )

                    await task_updater.update_status(
                        TaskState.working,
                        message=task_updater.new_agent_message(
                            [TextPart(text="Processing tool calls...")]
                        ),
                    )
                    continue

                if message.content:
                    elapsed = time.perf_counter() - t0
                    content = message.content + _timing_footer(elapsed, fast_path=False)
                    parts = [TextPart(text=content)]
                    logger.debug(f"Full-agent response in {elapsed*1000:.0f}ms")
                    await task_updater.add_artifact(parts)
                    await task_updater.complete()
                break

            except Exception as e:
                logger.error(f"Error in OpenAI API call: {e}")
                elapsed = time.perf_counter() - t0
                error_parts = [
                    TextPart(
                        text=f"Sorry, an error occurred while processing the request: {e!s}"
                        + _timing_footer(elapsed, fast_path=False)
                    )
                ]
                await task_updater.add_artifact(error_parts)
                await task_updater.complete()
                break

        if iteration >= max_iterations:
            elapsed = time.perf_counter() - t0
            error_parts = [
                TextPart(
                    text="Sorry, the request has exceeded the maximum number of iterations."
                    + _timing_footer(elapsed, fast_path=False)
                )
            ]
            await task_updater.add_artifact(error_parts)
            await task_updater.complete()

    def _extract_function_schema(self, func):
        """Extract OpenAI function schema from a Python function"""
        import inspect

        sig = inspect.signature(func)
        docstring = inspect.getdoc(func) or ""
        lines = docstring.split("\n")
        description = lines[0] if lines else func.__name__

        properties = {}
        required = []

        for param_name, param in sig.parameters.items():
            param_type = "string"
            param_description = f"Parameter {param_name}"

            if param.annotation != inspect.Parameter.empty:
                if param.annotation == int:
                    param_type = "integer"
                elif param.annotation == float:
                    param_type = "number"
                elif param.annotation == bool:
                    param_type = "boolean"
                elif param.annotation == list:
                    param_type = "array"
                elif param.annotation == dict:
                    param_type = "object"

            if param.default == inspect.Parameter.empty:
                required.append(param_name)

            properties[param_name] = {
                "type": param_type,
                "description": param_description,
            }

        return {
            "name": func.__name__,
            "description": description,
            "parameters": {
                "type": "object",
                "properties": properties,
                "required": required,
            },
        }

    async def execute(
        self,
        context: RequestContext,
        event_queue: EventQueue,
    ):
        updater = TaskUpdater(event_queue, context.task_id, context.context_id)
        if not context.current_task:
            await updater.submit()
        await updater.start_work()

        message_text = ""
        for part in context.message.parts:
            if isinstance(part.root, TextPart):
                message_text += part.root.text

        await self._process_request(message_text, context, updater)
        logger.debug("[Translator Agent] execute exiting")

    async def cancel(self, context: RequestContext, event_queue: EventQueue):
        raise ServerError(error=UnsupportedOperationError())

"""Translator agent executor — OpenAI tool-calling loop with 1.x A2A event queue."""

import inspect
import json
import logging
import os

from a2a.helpers import (
    new_task_from_user_message,
    new_text_artifact_update_event,
    new_text_status_update_event,
)
from a2a.server.agent_execution import AgentExecutor, RequestContext
from a2a.server.events import EventQueue
from a2a.types import TaskState
from openai import AsyncOpenAI

from toolset import TranslatorToolset

logger = logging.getLogger(__name__)

_TOOLS = [
    {
        "type": "function",
        "function": {
            "name": "translate_text",
            "description": "Translate plain text from one language to another.",
            "parameters": {
                "type": "object",
                "properties": {
                    "text": {"type": "string", "description": "Text to translate"},
                    "target_language": {
                        "type": "string",
                        "description": "Target language BCP-47 code (e.g. 'es', 'fr', 'de')",
                    },
                    "source_language": {
                        "type": "string",
                        "description": "Source language code — omit for auto-detect",
                    },
                },
                "required": ["text", "target_language"],
            },
        },
    },
    {
        "type": "function",
        "function": {
            "name": "translate_url",
            "description": "Fetch a web page and translate its text content.",
            "parameters": {
                "type": "object",
                "properties": {
                    "url": {"type": "string", "description": "Web page URL"},
                    "target_language": {
                        "type": "string",
                        "description": "Target language BCP-47 code",
                    },
                    "source_language": {
                        "type": "string",
                        "description": "Source language code — omit for auto-detect",
                    },
                },
                "required": ["url", "target_language"],
            },
        },
    },
    {
        "type": "function",
        "function": {
            "name": "detect_language",
            "description": "Detect the language of text or a web page.",
            "parameters": {
                "type": "object",
                "properties": {
                    "text": {"type": "string", "description": "Text to analyse"},
                    "url": {"type": "string", "description": "Web page URL to analyse"},
                },
            },
        },
    },
]

_SYSTEM_PROMPT = """\
You are a Translation agent. ALWAYS use your tools — never translate from memory.
Call a tool before answering. Include original text, translation, and both language codes."""


class TranslatorExecutor(AgentExecutor):
    def __init__(self):
        self._client = AsyncOpenAI(
            api_key=os.getenv("OPENAI_API_KEY"),
            base_url=os.getenv("OPENAI_BASE_URL") or None,
        )
        self._model = os.getenv("OPENAI_MODEL", "gpt-4o-mini")
        self._toolset = TranslatorToolset()

    async def execute(self, context: RequestContext, event_queue: EventQueue) -> None:
        query = context.get_user_input()
        task = context.current_task or new_task_from_user_message(context.message)
        await event_queue.enqueue_event(task)

        await event_queue.enqueue_event(
            new_text_status_update_event(
                task_id=task.id,
                context_id=task.context_id,
                state=TaskState.TASK_STATE_WORKING,
                text="Translating...",
            )
        )

        result = await self._run(query, task.id, task.context_id, event_queue)

        await event_queue.enqueue_event(
            new_text_artifact_update_event(
                task_id=task.id,
                context_id=task.context_id,
                name="translation",
                text=result,
            )
        )
        await event_queue.enqueue_event(
            new_text_status_update_event(
                task_id=task.id,
                context_id=task.context_id,
                state=TaskState.TASK_STATE_COMPLETED,
                text=result,
            )
        )

    async def _run(
        self,
        query: str,
        task_id: str,
        context_id: str,
        event_queue: EventQueue,
    ) -> str:
        messages: list = [
            {"role": "system", "content": _SYSTEM_PROMPT},
            {"role": "user", "content": query},
        ]

        for _ in range(6):
            resp = await self._client.chat.completions.create(
                model=self._model,
                messages=messages,
                tools=_TOOLS,
                temperature=0.1,
            )
            choice = resp.choices[0].message
            messages.append(choice)

            if not choice.tool_calls:
                return choice.content or ""

            for tc in choice.tool_calls:
                name = tc.function.name
                args = json.loads(tc.function.arguments)

                await event_queue.enqueue_event(
                    new_text_status_update_event(
                        task_id=task_id,
                        context_id=context_id,
                        state=TaskState.TASK_STATE_WORKING,
                        text=f"{name}...",
                    )
                )

                tool_result = await self._call_tool(name, args)
                messages.append({
                    "role": "tool",
                    "tool_call_id": tc.id,
                    "content": tool_result,
                })

        return "Could not complete the translation request."

    async def _call_tool(self, name: str, args: dict) -> str:
        try:
            fn = getattr(self._toolset, name, None)
            if fn is None:
                return f"Unknown tool: {name}"
            result = fn(**args)
            if inspect.iscoroutine(result):
                result = await result
            return str(result)
        except Exception as exc:
            logger.warning("Tool %s failed: %s", name, exc)
            return f"Tool error: {exc}"

    async def cancel(self, context: RequestContext, event_queue: EventQueue) -> None:
        pass
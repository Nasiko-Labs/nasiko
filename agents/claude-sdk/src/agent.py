import os
from collections.abc import AsyncIterable
from typing import Any

import anthropic
# [nasiko:imports]


class SynthesizerAgent:
    """Takes research/data from other agents and produces polished, structured output."""

    SUPPORTED_CONTENT_TYPES = ["text", "text/plain"]

    def __init__(self):
        self.client = anthropic.AsyncAnthropic(
            base_url=os.getenv("ANTHROPIC_BASE_URL", "https://api.deepseek.com/anthropic"),
            api_key=os.getenv("ANTHROPIC_API_KEY", os.getenv("DEEPSEEK_API_KEY")),
        )
        self.model = os.getenv("MODEL", "deepseek-v4-flash")

    async def stream(self, query: str, context_id: str) -> AsyncIterable[dict[str, Any]]:
        yield {
            "is_task_complete": False,
            "require_user_input": False,
            "content": "Synthesizing...",
        }

        full_response = ""
        async with self.client.messages.stream(
            model=self.model,
            max_tokens=2048,
            system=(
                "You are a synthesis expert. You take raw research, data, and analysis "
                "from multiple sources and produce clear, well-structured reports. "
                "Use markdown formatting. Include a summary, key findings, and conclusions. "
                "If the input is a question, produce a comprehensive answer drawing on "
                "all available information."
            ),
            messages=[{"role": "user", "content": query}],
        ) as stream:
            async for text in stream.text_stream:
                full_response += text
                yield {
                    "is_task_complete": False,
                    "require_user_input": False,
                    "content": text,
                }

        yield {
            "is_task_complete": True,
            "require_user_input": False,
            "content": full_response,
        }

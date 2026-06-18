import os
from collections.abc import AsyncIterable
from typing import Any, Literal

import httpx
from langchain_core.messages import AIMessage, ToolMessage
from langchain_core.tools import tool
# [nasiko:imports]
from langchain_openai import ChatOpenAI
from langgraph.checkpoint.memory import MemorySaver
from langgraph.prebuilt import create_react_agent
from pydantic import BaseModel

memory = MemorySaver()


@tool
def web_search(query: str) -> str:
    """Search the web for current information."""
    resp = httpx.get(
        "https://api.duckduckgo.com/",
        params={"q": query, "format": "json", "no_html": "1", "skip_disambig": "1"},
        timeout=10.0,
        follow_redirects=True,
    )
    if resp.status_code != 200:
        return f"Search failed for '{query}'."
    data = resp.json()
    parts: list[str] = []
    if data.get("AbstractText"):
        parts.append(data["AbstractText"])
    for topic in data.get("RelatedTopics", [])[:5]:
        if isinstance(topic, dict) and topic.get("Text"):
            parts.append(f"• {topic['Text']}")
    return "\n".join(parts) if parts else f"No results for '{query}'."


@tool
def get_exchange_rate(currency_from: str = "USD", currency_to: str = "EUR", currency_date: str = "latest") -> dict:
    """Get current or historical exchange rates between currencies.

    Args:
        currency_from: Source currency code (e.g., "USD").
        currency_to: Target currency code (e.g., "EUR").
        currency_date: Date for rate or "latest".
    """
    try:
        response = httpx.get(
            f"https://api.frankfurter.app/{currency_date}",
            params={"from": currency_from, "to": currency_to},
        )
        response.raise_for_status()
        return response.json()
    except httpx.HTTPError as e:
        return {"error": f"API request failed: {e}"}


class ResponseFormat(BaseModel):
    """Structured response from the agent."""
    status: Literal["input_required", "completed", "error"] = "input_required"
    message: str


class DeepAnalystAgent:
    """Stateful multi-step reasoning agent with tool access for deep analysis."""

    SYSTEM_INSTRUCTION = (
        "You are a deep analyst. You perform multi-step reasoning to answer complex questions. "
        "Use web_search to gather information and get_exchange_rate for financial data. "
        "Think step by step, cross-reference sources, and provide thorough analysis with evidence. "
        "Always explain your reasoning process."
    )

    FORMAT_INSTRUCTION = (
        "Set status to input_required if you need more information. "
        "Set status to error if something went wrong. "
        "Set status to completed when the analysis is done."
    )

    SUPPORTED_CONTENT_TYPES = ["text", "text/plain"]

    def __init__(self):
        self.model = ChatOpenAI(
            model=os.getenv("MODEL", "deepseek-v4-flash"),
            openai_api_key=os.getenv("DEEPSEEK_API_KEY", os.getenv("OPENAI_API_KEY")),
            openai_api_base=os.getenv("OPENAI_BASE_URL", "https://api.deepseek.com/v1"),
            temperature=0,
        )
        self.tools = [
            web_search,
            get_exchange_rate,
            # [nasiko:tools]
        ]
        self.graph = create_react_agent(
            self.model,
            tools=self.tools,
            checkpointer=memory,
            prompt=self.SYSTEM_INSTRUCTION,
            response_format=(self.FORMAT_INSTRUCTION, ResponseFormat),
        )

    async def stream(self, query, context_id) -> AsyncIterable[dict[str, Any]]:
        inputs = {"messages": [("user", query)]}
        config = {"configurable": {"thread_id": context_id}}

        for item in self.graph.stream(inputs, config, stream_mode="values"):
            message = item["messages"][-1]
            if isinstance(message, AIMessage) and message.tool_calls:
                tool_names = [tc["name"] for tc in message.tool_calls]
                yield {
                    "is_task_complete": False,
                    "require_user_input": False,
                    "content": f"Using tools: {', '.join(tool_names)}...",
                }
            elif isinstance(message, ToolMessage):
                yield {
                    "is_task_complete": False,
                    "require_user_input": False,
                    "content": "Processing data...",
                }

        yield self._get_response(config)

    def _get_response(self, config) -> dict[str, Any]:
        current_state = self.graph.get_state(config)
        structured_response = current_state.values.get("structured_response")
        if structured_response and isinstance(structured_response, ResponseFormat):
            return {
                "is_task_complete": structured_response.status == "completed",
                "require_user_input": structured_response.status == "input_required",
                "content": structured_response.message,
            }
        return {
            "is_task_complete": False,
            "require_user_input": True,
            "content": "Unable to process request. Please try again.",
        }

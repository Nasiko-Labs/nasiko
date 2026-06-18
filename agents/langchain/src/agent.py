import os
from collections.abc import AsyncIterable
from typing import Any

import httpx
from langchain.agents import AgentExecutor, create_tool_calling_agent
from langchain_core.prompts import ChatPromptTemplate
from langchain_core.tools import tool
from langchain_openai import ChatOpenAI


@tool
def search_wikipedia(query: str) -> str:
    """Search Wikipedia for a summary of a topic."""
    resp = httpx.get(
        f"https://en.wikipedia.org/api/rest_v1/page/summary/{query.replace(' ', '_')}",
        follow_redirects=True,
        timeout=10.0,
    )
    if resp.status_code != 200:
        return f"No Wikipedia article found for '{query}'."
    return resp.json().get("extract", "No extract available.")


PROMPT = ChatPromptTemplate.from_messages(
    [
        (
            "system",
            "You are a research assistant. Use the search_wikipedia tool to answer questions accurately and concisely.",
        ),
        ("human", "{input}"),
        ("placeholder", "{agent_scratchpad}"),
    ]
)


class ResearchAgent:
    SUPPORTED_CONTENT_TYPES = ["text", "text/plain"]

    def __init__(self):
        llm = ChatOpenAI(
            model="gpt-4o",
            api_key=os.getenv("OPENAI_API_KEY"),
            temperature=0,
        )
        agent = create_tool_calling_agent(llm, [search_wikipedia], PROMPT)
        self._executor = AgentExecutor(agent=agent, tools=[search_wikipedia], verbose=False)

    async def stream(self, query: str, context_id: str) -> AsyncIterable[dict[str, Any]]:
        yield {
            "is_task_complete": False,
            "require_user_input": False,
            "content": "Researching...",
        }
        async for event in self._executor.astream_events({"input": query}, version="v2"):
            if event["event"] == "on_tool_start":
                yield {
                    "is_task_complete": False,
                    "require_user_input": False,
                    "content": "Searching Wikipedia...",
                }
            elif event["event"] == "on_chain_end" and event.get("name") == "AgentExecutor":
                output = event["data"]["output"].get("output", "No answer found.")
                yield {
                    "is_task_complete": True,
                    "require_user_input": False,
                    "content": output,
                }
                return
        yield {
            "is_task_complete": True,
            "require_user_input": False,
            "content": "No answer found.",
        }

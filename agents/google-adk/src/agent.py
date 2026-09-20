import os
from collections.abc import AsyncIterable
from typing import Any

import httpx
from google.adk.agents import Agent
from google.adk.runners import Runner
from google.adk.sessions import InMemorySessionService
from google.genai import types as genai_types


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


APP_NAME = "nasiko_research"


class ResearchAgent:
    SUPPORTED_CONTENT_TYPES = ["text", "text/plain"]

    def __init__(self):
        self._agent = Agent(
            model="gemini-2.0-flash",
            name="research_agent",
            instruction=(
                "You are a research assistant. Use the search_wikipedia tool to "
                "answer questions accurately and concisely."
            ),
            tools=[search_wikipedia],
        )
        self._session_service = InMemorySessionService()
        self._runner = Runner(
            agent=self._agent,
            app_name=APP_NAME,
            session_service=self._session_service,
        )

    async def stream(self, query: str, context_id: str) -> AsyncIterable[dict[str, Any]]:
        yield {
            "is_task_complete": False,
            "require_user_input": False,
            "content": "Researching...",
        }
        await self._session_service.create_session(
            app_name=APP_NAME,
            user_id="user",
            session_id=context_id,
        )
        final_text = ""
        async for event in self._runner.run_async(
            user_id="user",
            session_id=context_id,
            new_message=genai_types.Content(
                role="user",
                parts=[genai_types.Part(text=query)],
            ),
        ):
            if event.is_final_response() and event.content and event.content.parts:
                final_text = event.content.parts[0].text
        yield {
            "is_task_complete": True,
            "require_user_input": False,
            "content": final_text or "No answer found.",
        }

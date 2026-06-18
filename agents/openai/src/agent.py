import os
from collections.abc import AsyncIterator

import httpx
from agents import Agent, Runner, function_tool
from agents.models.openai_chatcompletions import OpenAIChatCompletionsModel
from openai import AsyncOpenAI
# [nasiko:imports]


HEADERS = {"User-Agent": "NasikoResearchAgent/1.0 (https://nasiko.com)"}


@function_tool
def web_search(query: str) -> str:
    """Search the web for information using DuckDuckGo."""
    resp = httpx.get(
        "https://api.duckduckgo.com/",
        params={"q": query, "format": "json", "no_html": "1", "skip_disambig": "1"},
        headers=HEADERS,
        timeout=5.0,
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
    return "\n".join(parts) if parts else f"No results found for '{query}'."


@function_tool
def search_wikipedia(query: str) -> str:
    """Search Wikipedia for a summary of a topic."""
    resp = httpx.get(
        "https://en.wikipedia.org/w/api.php",
        params={"action": "query", "list": "search", "srsearch": query, "format": "json", "srlimit": "3"},
        headers=HEADERS,
        follow_redirects=True,
        timeout=5.0,
    )
    if resp.status_code != 200:
        return f"Wikipedia search failed for '{query}'."
    data = resp.json()
    results = data.get("query", {}).get("search", [])
    if not results:
        return f"No Wikipedia results for '{query}'."
    parts = []
    for r in results:
        snippet = r.get("snippet", "").replace('<span class="searchmatch">', "").replace("</span>", "")
        parts.append(f"• {r['title']}: {snippet}")
    return "\n".join(parts)


class ResearchAgent:
    SUPPORTED_CONTENT_TYPES = ["text", "text/plain"]

    def __init__(self):
        self._client = AsyncOpenAI(
            api_key=os.getenv("OPENAI_API_KEY"),
            base_url=os.getenv("OPENAI_BASE_URL"),
        )
        self._model_name = os.getenv("MODEL", "deepseek-v4-flash")
        model = OpenAIChatCompletionsModel(
            model=self._model_name,
            openai_client=self._client,
        )
        self._agent = Agent(
            name="Research Agent",
            instructions=(
                "You are a research assistant. Use web_search or search_wikipedia to gather facts. "
                "Make at most 2 tool calls total, then answer with what you have. Be concise."
            ),
            tools=[
                web_search,
                search_wikipedia,
                # [nasiko:tools]
            ],
            model=model,
        )

    async def invoke(self, query: str, context_id: str) -> str:
        result = await Runner.run(self._agent, query, max_turns=3)
        return result.final_output

    async def invoke_streaming(self, query: str, context_id: str) -> AsyncIterator[str]:
        result = await Runner.run(self._agent, query, max_turns=3)
        research_context = result.final_output

        # Phase 2: stream the final answer token-by-token directly via chat completions
        stream = await self._client.chat.completions.create(
            model=self._model_name,
            messages=[
                {"role": "system", "content": "Present the following research clearly and concisely."},
                {"role": "user", "content": research_context},
            ],
            stream=True,
        )
        async for chunk in stream:
            delta = chunk.choices[0].delta if chunk.choices else None
            if delta and delta.content:
                yield delta.content

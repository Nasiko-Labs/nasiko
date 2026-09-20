import os
import json
from collections.abc import AsyncIterator
from datetime import date, timedelta
from plan_selector import select_plan
from demo_state import demo_state
import httpx
from agents import Agent, Runner, function_tool
from agents.models.openai_chatcompletions import OpenAIChatCompletionsModel
from openai import AsyncOpenAI
# [nasiko:imports]


HEADERS = {"User-Agent": "AffordAgent/1.0 (https://nasiko.com)"}

# TODO: replace with real Anakin endpoint + auth once confirmed
ANAKIN_API_URL = os.getenv("ANAKIN_API_URL", "")
ANAKIN_API_KEY = os.getenv("ANAKIN_API_KEY", "")


@function_tool(strict_mode=False)
def get_product_price(item_name: str) -> str:
    """Look up live price/product info for an item via Anakin's web-data API."""
    if not ANAKIN_API_URL:
        # Fallback mock — keeps the agent demoable even if Anakin isn't wired yet
        return json.dumps({
            "item": item_name,
            "price": 45000,
            "currency": "INR",
            "source": "mock (Anakin not configured)",
        })
    resp = httpx.get(
        ANAKIN_API_URL,
        params={"q": item_name},
        headers={**HEADERS, "Authorization": f"Bearer {ANAKIN_API_KEY}"},
        timeout=8.0,
        follow_redirects=True,
    )
    if resp.status_code != 200:
        return json.dumps({"item": item_name, "error": f"Anakin lookup failed ({resp.status_code})"})
    return resp.text


# --- Deterministic affordability core -----------------------------------
# TODO: swap this out for the real ported logic from Buy or Wait
# (forecast_engine.py / plan_selector.py) if you have it handy.

def compute_affordability(price: float) -> dict:
    request = {
        "request_id": "live-demo",
        "request_date": date.today().isoformat(),
        "desired_completion_date": (date.today() + timedelta(days=45)).isoformat(),
        "requested_amount": price,
        "allows_partial_payment": True,
    }
    return select_plan(request, demo_state(), payment_options=[])

@function_tool(strict_mode=False)
def check_affordability(price: float) -> str:
    """Run the deterministic affordability engine against the user's spending profile."""
    result = compute_affordability(price)
    return json.dumps(result)

class AffordAgent:
    SUPPORTED_CONTENT_TYPES = ["text", "text/plain"]

    def __init__(self):
        self._client = AsyncOpenAI(
            api_key=os.getenv("OPENAI_API_KEY"),
            base_url=os.getenv("OPENAI_BASE_URL"),
        )
        self._model_name = os.getenv("MODEL", "gpt-4o-mini")
        model = OpenAIChatCompletionsModel(
            model=self._model_name,
            openai_client=self._client,
        )
        self._agent = Agent(
            name="Afford Agent",
            instructions=(
                "You help users decide if they can afford a purchase right now. "
                "First call get_product_price to find the item's live price. "
                "Then call check_affordability with that price to get a structured verdict. "
                "Then explain the structured result clearly and concisely, in plain language — "
                "no jargon, no hedging."
                ),
            tools=[
                get_product_price,
                check_affordability,
                # [nasiko:tools]
            ],
            model=model,
        )

    async def invoke(self, query: str, context_id: str) -> str:
        result = await Runner.run(self._agent, query, max_turns=6)
        return result.final_output

    async def invoke_streaming(self, query: str, context_id: str) -> AsyncIterator[str]:
        result = await Runner.run(self._agent, query, max_turns=6)
        answer_context = result.final_output

        stream = await self._client.chat.completions.create(
            model=self._model_name,
            messages=[
                {"role": "system", "content": "Present the following affordability verdict clearly and concisely."},
                {"role": "user", "content": answer_context},
            ],
            stream=True,
        )
        async for chunk in stream:
            delta = chunk.choices[0].delta if chunk.choices else None
            if delta and delta.content:
                yield delta.content
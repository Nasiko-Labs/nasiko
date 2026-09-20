import os
import json
import re
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

ANAKIN_API_URL = os.getenv("ANAKIN_API_URL", "https://api.anakin.io/v1/search")
ANAKIN_API_KEY = os.getenv("ANAKIN_API_KEY", "ask_0c1dd9118d683bcf1ca964c9ee1b5b250924de296980148e8fc511f38c261e72")
_PRICE_RE = re.compile(r"(?:₹|Rs\.?\s?)\s?\d[\d,]*(?:\.\d+)?")
_NOISE_RE = re.compile(r"(?i)\bemi\b|\d\s*x\s*\d+\s*months?|no[- ]cost")
_price_cache: dict = {}


def _price_lines(text: str, max_lines: int = 6) -> list:
       lines, seen = [], set()
       for line in text.splitlines():
           line = line.strip()
           m = _PRICE_RE.search(line)
           if len(line) < 12 or not m or (_NOISE_RE.search(line) and len(line) < 60):
               continue
           snippet = line if len(line) <= 300 else line[max(0, m.start() - 200): m.end() + 80]
           if snippet not in seen:
               seen.add(snippet)
               lines.append(snippet)
           if len(lines) >= max_lines:
               break
       return lines


@function_tool(strict_mode=False)
def get_product_price(item_name: str) -> str:
    """Look up current INR price listings for an item via Anakin web search."""
    key = item_name.strip().lower()
    if key in _price_cache:
        return _price_cache[key]
    if not ANAKIN_API_KEY:
        return json.dumps({"item": item_name, "price": 45000, "currency": "INR",
                           "source": "mock (Anakin not configured)"})
    try:
        resp = httpx.post(
            ANAKIN_API_URL,
            headers={**HEADERS, "X-API-Key": ANAKIN_API_KEY, "Content-Type": "application/json"},
            json={"prompt": f"{item_name} price in India INR", "limit": 5},
            timeout=20.0,
        )
        resp.raise_for_status()
        listings = [
            {"title": r.get("title", "")[:120], "url": r.get("url", ""),
             "price_lines": _price_lines(r.get("snippet", ""))}
            for r in resp.json().get("results", [])
        ]
        listings = [x for x in listings if x["price_lines"]]
        if not listings:
            raise ValueError("no prices found")
    except Exception as e:
        return json.dumps({"item": item_name, "price": 45000, "currency": "INR",
                           "source": f"mock (Anakin failed: {type(e).__name__})"})
    out = json.dumps({"item": item_name, "currency": "INR", "source": "anakin search", "listings": listings})
    _price_cache[key] = out
    return out

# --- Deterministic affordability core -----------------------------------
   # Deterministic core, ported from Buy or Wait (plan_selector.py / forecast_engine.py).

def compute_affordability(price: float, current_balance: float = None, minimum_balance_to_keep: float = None) -> dict:
    state = demo_state()
    is_demo = current_balance is None and minimum_balance_to_keep is None
    if current_balance is not None:
        state["current_balance"] = current_balance
    if minimum_balance_to_keep is not None:
        state["minimum_balance_to_keep"] = minimum_balance_to_keep

    request = {
        "request_id": "live-demo",
        "request_date": date.today().isoformat(),
        "desired_completion_date": (date.today() + timedelta(days=45)).isoformat(),
        "requested_amount": price,
        "allows_partial_payment": True,
    }
    result = select_plan(request, state, payment_options=[])
    result["payments"] = [
           {"date": d, "amount": float(a)}
           for d, a in (p.split(":", 1) for p in str(result.get("payment_plan", "")).split("|") if ":" in p)
       ]
    if not result["payments"]:
        available = state["current_balance"] - state["minimum_balance_to_keep"]
        if price <= available:
            result["payments"] = [{"date": date.today().isoformat(), "amount": float(price)}]
    result["profile_assumptions"] = {
           "current_balance": state["current_balance"],
           "minimum_balance_to_keep": state["minimum_balance_to_keep"],
           "note": "fixed demo profile, not real account data" if is_demo else "user-provided balance",
       }
    return result

@function_tool(strict_mode=False)
def check_affordability(price: float, current_balance: float = None, minimum_balance_to_keep: float = None) -> str:
    """Run the deterministic affordability engine. If current_balance or minimum_balance_to_keep are not
    provided, a fixed demo profile is used instead."""
    if not price or price <= 0:
        return json.dumps({"error": "price must be a positive amount in INR"})
    result = compute_affordability(price, current_balance, minimum_balance_to_keep)
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
                   "You help users decide if they can afford a purchase right now. A fixed demo spending profile is "
                   "used unless the user gives their own current balance and/or minimum balance to keep. "
                   "If the user states a price, use it and skip the lookup; otherwise call get_product_price and pick the "
                   "most representative current price for the exact item asked (ignore EMI amounts, MRPs and filter values). "
                   "State the seller or site and the exact variant your price refers to, and note that prices vary by seller. "
                   "If the price source starts with 'mock', say the price is a placeholder estimate. "
                   "If the user's message gives a current balance and/or minimum balance to keep, pass them to "
                   "check_affordability as current_balance and minimum_balance_to_keep; if not provided, omit them "
                   "and the tool falls back to its demo profile. "
                   "Then call check_affordability exactly once with that price in rupees. "
                   "Use only numbers and dates returned by the tools; never calculate or invent amounts or dates. "
                   "Reply in short markdown with: **Verdict** (affordable now / affordable with a payment plan / not affordable), "
                   "**Price used** (with its source), "
                   "**Payment plan**: if payments has one entry equal to the full price, say it can be paid in full "
                   "today on that date; if it has multiple entries, list each date and amount; if payments is empty, "
                   "say no safe plan was found and give earliest_date_for_full_payment if present. "
                   "After the bullets, add one short sentence explaining the plan in plain language — connect "
                   "current_balance and minimum_balance_to_keep to why the first payment amount is what it is. "
                   "Skip this sentence if affordable now with a single lump-sum payment. "
                   "**Assumptions** (current_balance and minimum_balance_to_keep from profile_assumptions, noting in "
                   "profile_assumptions.note whether it's a demo or user-provided balance), and "
                   "Mention spending_changes_needed only if it is not 'none'. "
                   "End with one line saying this is guidance, not a guarantee. Keep it under 130 words."
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
        yield result.final_output or "Sorry, I could not produce an answer."
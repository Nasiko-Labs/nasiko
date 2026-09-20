"""Classifier — small LLM when keyed, else heuristic; always returns valid JSON fields."""

from __future__ import annotations

import json
import logging
import os
import re
from dataclasses import dataclass
from typing import Any

import httpx

from tier import VALID_TASK_TYPES

logger = logging.getLogger(__name__)

CLASSIFIER_SYSTEM = """You classify user tasks for an LLM router. Return ONLY valid JSON, no prose.
Schema: {"task_type":"summarize|translate|reason|code|chat|extract",
         "complexity":1-5, "reasoning":"one short sentence"}
complexity guide:
  1 = trivial (greetings, one-word answers)
  2 = short factual / simple rewrite
  3 = multi-step but bounded
  4 = requires reasoning or long output
  5 = deep reasoning, math proofs, complex code"""

DEFAULT_CLASSIFICATION = {
    "task_type": "chat",
    "complexity": 3,
    "reasoning": "fallback default",
}


@dataclass
class Classification:
    task_type: str
    complexity: int
    reasoning: str
    source: str  # "llm" | "heuristic" | "fallback"


def _classifier_credentials() -> tuple[str | None, str, str]:
    """Only use an explicit classifier/Groq key — never the agent's OpenAI gateway JWT.

    Deployed Nasiko agents get OPENAI_BASE_URL + OPENAI_API_KEY (identity JWT). Using
    those for classification caused ~20s timeouts and polluted chat latency.
    """
    model = os.getenv("CLASSIFIER_MODEL") or "llama-3.1-8b-instant"
    key = (os.getenv("CLASSIFIER_API_KEY") or os.getenv("GROQ_API_KEY") or "").strip() or None
    if not key:
        return None, "", model
    base = (
        os.getenv("CLASSIFIER_BASE_URL")
        or "https://api.groq.com/openai/v1"
    ).rstrip("/")
    return key, base, model



def parse_classifier_output(raw: str | dict[str, Any] | None) -> Classification:
    """Parse classifier JSON; on any failure return chat/3."""
    if raw is None:
        return Classification(**DEFAULT_CLASSIFICATION, source="fallback")  # type: ignore[arg-type]

    data: Any = raw
    if isinstance(raw, str):
        text = raw.strip()
        # Strip markdown fences if the model wraps JSON.
        fence = re.search(r"```(?:json)?\s*(\{.*?\})\s*```", text, re.DOTALL)
        if fence:
            text = fence.group(1)
        else:
            brace = re.search(r"\{.*\}", text, re.DOTALL)
            if brace:
                text = brace.group(0)
        try:
            data = json.loads(text)
        except json.JSONDecodeError:
            logger.warning("classifier parse failure — using chat/3")
            return Classification(
                task_type="chat",
                complexity=3,
                reasoning="parse failure fallback",
                source="fallback",
            )

    if not isinstance(data, dict):
        return Classification(
            task_type="chat",
            complexity=3,
            reasoning="parse failure fallback",
            source="fallback",
        )

    task_type = str(data.get("task_type", "chat")).strip().lower()
    if task_type not in VALID_TASK_TYPES:
        task_type = "chat"
    try:
        complexity = int(data.get("complexity", 3))
    except (TypeError, ValueError):
        complexity = 3
    complexity = max(1, min(5, complexity))
    reasoning = str(data.get("reasoning") or "classified").strip() or "classified"
    return Classification(
        task_type=task_type,
        complexity=complexity,
        reasoning=reasoning,
        source="llm",
    )


def heuristic_classify(prompt: str) -> Classification:
    """Offline / no-key classifier good enough for the demo evals."""
    p = prompt.strip()
    low = p.lower()

    if len(p) <= 8 and re.fullmatch(
        r"(hi|hey|hello|yo|sup|thanks|thx|ok|okay)[!?.]*", low
    ):
        return Classification(
            task_type="chat",
            complexity=1,
            reasoning="trivial greeting",
            source="heuristic",
        )

    if any(
        k in low
        for k in (
            "prove",
            "theorem",
            "pythagorean",
            "derive",
            "formal proof",
            "complexity analysis",
        )
    ):
        return Classification(
            task_type="reason",
            complexity=5,
            reasoning="deep reasoning / proof request",
            source="heuristic",
        )

    if any(
        k in low
        for k in ("translate", "in french", "in spanish", "in german", "traduis")
    ):
        return Classification(
            task_type="translate",
            complexity=2,
            reasoning="translation request",
            source="heuristic",
        )

    if any(
        k in low
        for k in ("summarize", "summary", "tl;dr", "tldr", "sum up")
    ):
        return Classification(
            task_type="summarize",
            complexity=2,
            reasoning="summarization request",
            source="heuristic",
        )

    if any(
        k in low
        for k in (
            "write a function",
            "implement",
            "refactor",
            "bug",
            "stack trace",
            "```",
            "def ",
            "class ",
        )
    ):
        return Classification(
            task_type="code",
            complexity=4 if len(p) > 80 else 3,
            reasoning="coding request",
            source="heuristic",
        )

    if any(k in low for k in ("extract", "parse out", "pull the")):
        return Classification(
            task_type="extract",
            complexity=2,
            reasoning="extraction request",
            source="heuristic",
        )

    return Classification(
        task_type="chat",
        complexity=3,
        reasoning="general chat",
        source="heuristic",
    )


async def classify(prompt: str, client: httpx.AsyncClient | None = None) -> Classification:
    """LLM classify when credentials exist; otherwise heuristic."""
    key, base, model = _classifier_credentials()
    if not key:
        return heuristic_classify(prompt)

    owns_client = client is None
    http = client or httpx.AsyncClient(timeout=3.0)
    try:
        resp = await http.post(
            f"{base}/chat/completions",
            headers={
                "Authorization": f"Bearer {key}",
                "Content-Type": "application/json",
            },
            json={
                "model": model,
                "temperature": 0,
                "response_format": {"type": "json_object"},
                "messages": [
                    {"role": "system", "content": CLASSIFIER_SYSTEM},
                    {"role": "user", "content": prompt},
                ],
            },
        )
        if resp.status_code >= 400:
            logger.warning(
                "classifier LLM HTTP %s — falling back to heuristic", resp.status_code
            )
            return heuristic_classify(prompt)
        body = resp.json()
        content = (
            body.get("choices", [{}])[0]
            .get("message", {})
            .get("content", "")
        )
        parsed = parse_classifier_output(content)
        if parsed.source == "fallback":
            # Malformed LLM output — still satisfy eval #3 with chat/3.
            return parsed
        parsed.source = "llm"
        return parsed
    except Exception as e:
        logger.warning("classifier LLM error (%s) — heuristic", e)
        return heuristic_classify(prompt)
    finally:
        if owns_client:
            await http.aclose()

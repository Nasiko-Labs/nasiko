import os
from dataclasses import dataclass

from openai import OpenAI


@dataclass
class DocumentParser:
    """Simple holder for the active document text under review."""

    document_text: str = ""


class LLMAdapter:
    """Tiny adapter exposing chat(system_prompt, user_prompt, session_id)."""

    def __init__(self):
        api_key = os.getenv("OPENAI_API_KEY") or os.getenv("MINIMAX_API_KEY")
        if not api_key:
            raise ValueError(
                "Either OPENAI_API_KEY or MINIMAX_API_KEY environment variable must be set"
            )

        model = "gpt-4o"
        base_url = None
        if os.getenv("MINIMAX_API_KEY") and not os.getenv("OPENAI_API_KEY"):
            model = os.getenv("MINIMAX_MODEL", "MiniMax-M2.7")
            base_url = os.getenv("MINIMAX_BASE_URL", "https://api.minimax.io/v1")

        self.model = model
        self.client = OpenAI(api_key=api_key, base_url=base_url)

    def chat(self, system_prompt: str, user_prompt: str, session_id: str) -> str:
        response = self.client.chat.completions.create(
            model=self.model,
            messages=[
                {"role": "system", "content": system_prompt},
                {"role": "user", "content": user_prompt},
            ],
        )
        return response.choices[0].message.content or ""


class BaseAgent:
    """Compatibility base class expected by PolicyAgent."""

    def __init__(self, mongo_url: str, db_name: str):
        self.mongo_url = mongo_url
        self.db_name = db_name
        self.document_parser = DocumentParser()
        self.agent = LLMAdapter()

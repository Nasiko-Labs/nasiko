import logging
import os

from langchain_openai import ChatOpenAI

logger = logging.getLogger(__name__)


class LLMClient:
    def __init__(self, model: str = "gpt-4o", temperature: float = 0.7):
        logger.info(
            f"Initializing LLMClient with model={model}, temperature={temperature}"
        )
        self.llm = ChatOpenAI(
            model=model, temperature=temperature, api_key=os.getenv("OPENAI_API_KEY")
        )

    def invoke(self, system_prompt: str, user_prompt: str) -> str:
        logger.debug(
            f"Invoking LLM with system_prompt length={len(system_prompt)}, user_prompt length={len(user_prompt)}"
        )
        response = self.llm.invoke([("system", system_prompt), ("human", user_prompt)])
        logger.info("LLM invocation complete")
        return response.content

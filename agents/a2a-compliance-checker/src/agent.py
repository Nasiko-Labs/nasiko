"""
Core agent logic for translation.
"""
import logging
from typing import List, Dict, Any, Optional

from langchain_openai import ChatOpenAI
from langchain_core.prompts import ChatPromptTemplate, MessagesPlaceholder
from langchain.agents import AgentExecutor, create_tool_calling_agent


from tools import extract_web_text

logger = logging.getLogger(__name__)

class Agent:
    def __init__(self):
        # Initialize your agent
        self.name = "Translation Agent"
        
        # Initialize Tools
        self.tools = [extract_web_text]
        
        # Initialize LangChain components
        self.llm = ChatOpenAI(model="gpt-4o", temperature=0)
        
        # System prompt tailored for text-to-text translation
        self.system_prompt = """You are a helpful assistant whose primary objective is to help the user with language translation.

RULES:
- If the user provides a URL, use the 'extract_web_text' tool to get the content, then translate the extracted text.
- Detect the source language and the target language from the user's request.
- If the user specifies a target language, translate the text to that language.
- If the user provides text without specifying a target language, default to translating it to English (if it's not English) or ask for clarification if ambiguous.
- Translate the text fully and accurately.
- Preserve the original meaning and tone.
- Do NOT add explanations or commentary unless the translation requires context notes (which should be minimal).

RESPONSE FORMAT:
- Provide only the translated text.
"""
        
        self.prompt = ChatPromptTemplate.from_messages([
            ("system", self.system_prompt),
            ("user", "{input}"),
            MessagesPlaceholder(variable_name="agent_scratchpad"),
        ])
        
        # Create Tool Calling Agent
        agent = create_tool_calling_agent(self.llm, self.tools, self.prompt)
        self.agent_executor = AgentExecutor(agent=agent, tools=self.tools, verbose=True)
        
    def process_message(self, message_text: str) -> str:
        """
        Process the incoming message using LangChain.
        """
        logger.info(f"Processing message: {message_text[:50]}...")
        result = self.agent_executor.invoke({"input": message_text})
        return result["output"]

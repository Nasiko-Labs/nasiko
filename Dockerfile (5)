"""
LangChain Agent -- Uses Nasiko LLM Gateway (no hardcoded API keys)

This agent demonstrates the recommended pattern for Nasiko agents:
- Uses the platform-provided LLM gateway URL (http://llm-gateway:4000)
- Uses a platform-provided virtual key (no real provider keys in source)
- Switching the underlying provider (OpenAI -> Anthropic) requires only
  a gateway config change, not an agent code change.

Environment variables injected by the Nasiko platform at deploy time:
    OPENAI_API_BASE=http://llm-gateway:4000
    OPENAI_API_KEY=nasiko-virtual-proxy-key

DO NOT hardcode model provider API keys in your agent source code.
Use the gateway instead.
"""

import os

from langchain_openai import ChatOpenAI
from langchain.agents import AgentExecutor, create_tool_calling_agent
from langchain.tools import tool
from langchain_core.prompts import ChatPromptTemplate


# -- Tools ------------------------------------------------------------------

@tool
def greet(name: str) -> str:
    """Greet someone by name."""
    return f"Hello, {name}! Welcome to Nasiko."


@tool
def calculate(expression: str) -> str:
    """Evaluate a simple math expression. Only supports +, -, *, /."""
    try:
        # Safe eval: only allow digits and basic operators
        allowed = set("0123456789+-*/.(). ")
        if not all(c in allowed for c in expression):
            return "Error: Only basic math expressions are supported"
        return str(eval(expression))  # noqa: S307
    except Exception as e:
        return f"Error: {e}"


# -- LLM via Gateway -------------------------------------------------------
# The platform injects these env vars at deploy time.
# In local dev, set them manually or use docker-compose.

llm = ChatOpenAI(
    base_url=os.environ.get("OPENAI_API_BASE", "http://llm-gateway:4000"),
    api_key=os.environ.get("OPENAI_API_KEY", "nasiko-virtual-proxy-key"),
    model="platform-default-model",
    temperature=0,
)

prompt = ChatPromptTemplate.from_messages([
    ("system", "You are a helpful assistant with access to tools."),
    ("human", "{input}"),
    ("placeholder", "{agent_scratchpad}"),
])

tools = [greet, calculate]
agent = create_tool_calling_agent(llm, tools, prompt)
executor = AgentExecutor(agent=agent, tools=tools, verbose=True)


if __name__ == "__main__":
    result = executor.invoke({"input": "Greet Alice and calculate 42 * 17"})
    print(result["output"])

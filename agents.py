"""
Fake AI agent registry and processing engine.

This module supplies deterministic, async-compatible stand-ins for real AI
agents. The orchestrator can demonstrate caching, rate limiting, queueing, and
observability without external model dependencies. Future integrations can
replace FakeAgent instances with LLM/tool-backed agents behind the same
registry contract.
"""

import asyncio
from typing import Any, Dict
from dataclasses import dataclass
from enum import Enum


class AgentType(str, Enum):
    """Supported agent types."""
    TRANSLATOR = "translator"
    CODER = "coder"
    SEARCH = "search"
    MATH = "math"


@dataclass
class AgentMetadata:
    """
    Agent metadata and capability information exposed by agent endpoints.
    
    EXTENSION POINT (Phase 3):
    - Add capability descriptors
    - Add supported languages/frameworks
    - Add cost metrics
    - Add SLA requirements
    """
    name: str
    description: str
    version: str = "1.0.0"
    capability_tags: list = None

    def __post_init__(self):
        """Initialize default values."""
        if self.capability_tags is None:
            self.capability_tags = []


class FakeAgent:
    """
    Fake AI agent with simulated async processing.
    
    EXTENSION POINT:
    - Replace with actual LangChain agents
    - Add real LLM integration
    - Add streaming responses
    - Add token counting
    """

    def __init__(self, agent_type: AgentType, metadata: AgentMetadata):
        """
        Initialize fake agent.
        
        Args:
            agent_type: Type of agent
            metadata: Agent metadata
        """
        self.agent_type = agent_type
        self.metadata = metadata
        self.request_count = 0

    async def process(self, query: str) -> str:
        """
        Process query through agent.
        
        Args:
            query: Input query
            
        Returns:
            Simulated agent response
        """
        self.request_count += 1
        
        # Keep behavior async so route, queue, and metrics paths resemble real
        # LLM calls even though the responses are deterministic.
        
        if self.agent_type == AgentType.TRANSLATOR:
            return await self._translator_process(query)
        elif self.agent_type == AgentType.CODER:
            return await self._coder_process(query)
        elif self.agent_type == AgentType.SEARCH:
            return await self._search_process(query)
        elif self.agent_type == AgentType.MATH:
            return await self._math_process(query)
        else:
            return "Unknown agent type"

    async def _translator_process(self, query: str) -> str:
        """Simulate translation processing."""
        # Simulate variable latency (1-2 seconds)
        await asyncio.sleep(0.5)
        
        translations = {
            "hello": "Bonjour (FR) / Hola (ES) / Hallo (DE)",
            "thank you": "Merci (FR) / Gracias (ES) / Danke (DE)",
            "goodbye": "Au revoir (FR) / Adiós (ES) / Auf Wiedersehen (DE)",
        }
        
        query_lower = query.lower()
        if query_lower in translations:
            return f"[TRANSLATOR] {translations[query_lower]}"
        return f"[TRANSLATOR] Translated: '{query}' → [Translation would go here]"

    async def _coder_process(self, query: str) -> str:
        """Simulate code generation processing."""
        # Simulate variable latency (1.5-2.5 seconds)
        await asyncio.sleep(0.7)
        
        code_templates = {
            "function": "def my_function(x):\n    return x * 2",
            "loop": "for i in range(10):\n    print(i)",
            "class": "class MyClass:\n    def __init__(self):\n        pass",
        }
        
        for keyword, code in code_templates.items():
            if keyword in query.lower():
                return f"[CODER] Generated code:\n```python\n{code}\n```"
        
        return (
            "[CODER] Generated code:\n"
            "```python\n"
            "def solution(input_data):\n"
            "    # Your solution here\n"
            "    return result\n"
            "```"
        )

    async def _search_process(self, query: str) -> str:
        """Simulate search processing."""
        # Simulate variable latency (1-2 seconds)
        await asyncio.sleep(0.6)
        
        return f"[SEARCH] Found {len(query) % 5 + 1} results for '{query}':\n" \
               f"  1. Result 1 - High relevance\n" \
               f"  2. Result 2 - Medium relevance\n" \
               f"  3. Result 3 - Medium relevance"

    async def _math_process(self, query: str) -> str:
        """Simulate mathematical computation without executing user input."""
        # Simulate variable latency (1.5-2 seconds)
        await asyncio.sleep(0.5)

        # Keep this deliberately safe and lightweight for the hackathon demo.
        # Phase 3+: Replace with a real math parser/tool, never dynamic execution.
        return f"[MATH] Calculated result for: '{query}'"


class AgentRegistry:
    """
    Centralized lookup for available demo agents.
    
    EXTENSION POINT:
    - Load agents from external config/database
    - Implement agent versioning and rollout
    - Add agent health checks and auto-scaling
    - Add agent capability-based routing
    """

    def __init__(self):
        """Initialize agent registry with fake agents."""
        self.agents: Dict[str, FakeAgent] = {}
        self._initialize_agents()

    def _initialize_agents(self) -> None:
        """Initialize default fake agents."""
        # Translator Agent
        self.agents[AgentType.TRANSLATOR.value] = FakeAgent(
            AgentType.TRANSLATOR,
            AgentMetadata(
                name="Language Translator",
                description="Translates text between multiple languages",
                capability_tags=["translation", "multilingual"]
            )
        )

        # Coder Agent
        self.agents[AgentType.CODER.value] = FakeAgent(
            AgentType.CODER,
            AgentMetadata(
                name="Code Generator",
                description="Generates and explains code snippets",
                capability_tags=["code-generation", "python", "javascript"]
            )
        )

        # Search Agent
        self.agents[AgentType.SEARCH.value] = FakeAgent(
            AgentType.SEARCH,
            AgentMetadata(
                name="Information Searcher",
                description="Searches and retrieves relevant information",
                capability_tags=["search", "retrieval", "qa"]
            )
        )

        # Math Agent
        self.agents[AgentType.MATH.value] = FakeAgent(
            AgentType.MATH,
            AgentMetadata(
                name="Mathematical Calculator",
                description="Performs mathematical computations and analysis",
                capability_tags=["math", "calculation", "analysis"]
            )
        )

    def get_agent(self, agent_name: str) -> FakeAgent:
        """
        Get agent by name.
        
        Args:
            agent_name: Name of the agent
            
        Returns:
            FakeAgent instance
            
        Raises:
            KeyError: If agent does not exist
        """
        agent_name_lower = agent_name.lower()
        
        if agent_name_lower not in self.agents:
            available = ", ".join(self.agents.keys())
            raise KeyError(
                f"Agent '{agent_name}' not found. Available agents: {available}"
            )
        
        return self.agents[agent_name_lower]

    def list_agents(self) -> Dict[str, Dict[str, Any]]:
        """
        List all available agents with metadata.
        
        Returns:
            Dictionary mapping agent names to their metadata
        """
        result = {}
        for name, agent in self.agents.items():
            result[name] = {
                "name": agent.metadata.name,
                "description": agent.metadata.description,
                "version": agent.metadata.version,
                "capabilities": agent.metadata.capability_tags,
                "request_count": agent.request_count
            }
        return result

    def get_agent_stats(self, agent_name: str) -> Dict[str, Any]:
        """
        Get statistics for specific agent.
        
        Args:
            agent_name: Name of the agent
            
        Returns:
            Dictionary with agent stats
        """
        agent = self.get_agent(agent_name)
        return {
            "name": agent_name,
            "request_count": agent.request_count,
            "description": agent.metadata.description,
            "capabilities": agent.metadata.capability_tags
        }


# Global agent registry instance
agent_registry = AgentRegistry()
